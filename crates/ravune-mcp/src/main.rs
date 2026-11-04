//! `ravune-mcp`: an MCP server on stdio that drives a running Ravune. MCP clients (Claude
//! Code: `claude mcp add ravune -- <path>/ravune-mcp`) start it; it reads the port and the
//! secret Ravune wrote into its data folder (`agent.json`, there while Preferences → Agents
//! is on) and forwards each tool call to Ravune as one JSON line.

use std::io::{BufRead, Write};

use ravune_mcp::{INSTRUCTIONS, PROTOCOLS, Ravune, tools};
use serde_json::{Value, json};

#[derive(Default)]
struct Bridge {
    ravune: Option<Ravune>,
    /// Ravune's data folder when it isn't the usual one (`--data`, which Ravune's chat passes:
    /// clients start bridges with an environment of their own, `RAVUNE_DATA` may not reach it).
    data: Option<std::path::PathBuf>,
}

impl Bridge {
    /// Forwards a tool call, reconnecting once if Ravune restarted.
    fn tool(&mut self, name: &str, args: Value) -> Result<Value, String> {
        for attempt in 0..2 {
            if self.ravune.is_none() {
                self.ravune = Some(Ravune::connect(self.data.as_deref())?);
            }
            match self.ravune.as_mut().map(|r| r.call(name, args.clone())) {
                Some(Err(e)) if attempt == 0 && (e.contains("closed") || e.contains("os error")) => self.ravune = None,
                Some(result) => return result,
                None => unreachable!("connected above"),
            }
        }
        Err("Ravune isn't answering".into())
    }

    /// One JSON-RPC message in, the reply out (none for notifications).
    fn handle(&mut self, msg: &Value) -> Option<Value> {
        let id = msg.get("id")?.clone();
        let result = match msg["method"].as_str().unwrap_or("") {
            "initialize" => {
                let asked = msg["params"]["protocolVersion"].as_str().unwrap_or("");
                let version = PROTOCOLS.iter().find(|v| **v == asked).unwrap_or(&PROTOCOLS[0]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "ravune", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or("");
                let args = msg["params"].get("arguments").cloned().unwrap_or(json!({}));
                let (text, error) = match self.tool(name, args) {
                    Ok(v) => (serde_json::to_string_pretty(&v).unwrap_or_default(), false),
                    Err(e) => (e, true),
                };
                Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": error }))
            }
            other => Err(json!({ "code": -32601, "message": format!("no method '{other}'") })),
        };
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e }),
        })
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = args.iter().position(|a| a == "--data").and_then(|i| args.get(i + 1)).map(std::path::PathBuf::from);
    let mut bridge = Bridge { data, ..Default::default() };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => bridge.handle(&msg),
            Err(e) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": e.to_string() } })),
        };
        if let Some(reply) = reply
            && writeln!(stdout, "{reply}").and_then(|_| stdout.flush()).is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;
    use std::net::TcpListener;

    /// A fake Ravune: checks the token, then echoes each method and its params back.
    #[test]
    fn speaks_mcp_and_forwards_tool_calls() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        let dir = std::env::temp_dir().join(format!("ravune-mcp-test-{port}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agent.json"), json!({ "port": port, "token": "s3cret" }).to_string()).unwrap();
        // SAFETY: this test is the only one touching the environment
        unsafe { std::env::set_var("RAVUNE_DATA", &dir) };
        std::thread::spawn(move || {
            let (stream, _) = server.accept().unwrap();
            let mut out = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines() {
                let msg: Value = serde_json::from_str(&line.unwrap()).unwrap();
                let reply = if msg["method"] == "hello" {
                    assert_eq!(msg["params"]["token"], "s3cret");
                    json!({ "id": msg["id"], "result": {} })
                } else if msg["method"] == "undo" {
                    json!({ "id": msg["id"], "error": "nothing to undo" })
                } else {
                    json!({ "id": msg["id"], "result": { "method": msg["method"], "params": msg["params"] } })
                };
                writeln!(out, "{reply}").unwrap();
            }
        });
        let mut b = Bridge::default();
        let init = b.handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } })).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(b.handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none(), "no reply to a notification");
        let list = b.handle(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).unwrap();
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 8);
        let call = b.handle(&json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "run_operator", "arguments": { "id": "pattern.add" } } })).unwrap();
        assert_eq!(call["result"]["isError"], false);
        let text: Value = serde_json::from_str(call["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!((text["method"].as_str(), text["params"]["id"].as_str()), (Some("run_operator"), Some("pattern.add")));
        let failed = b.handle(&json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "undo" } })).unwrap();
        assert_eq!((failed["result"]["isError"].as_bool(), failed["result"]["content"][0]["text"].as_str()), (Some(true), Some("nothing to undo")));
        let unknown = b.handle(&json!({ "jsonrpc": "2.0", "id": 5, "method": "resources/list" })).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
    }
}
