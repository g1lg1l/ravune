//! The MCP side of Ravune's agents, shared by the `ravune-mcp` bridge and Ravune's own AI chat
//! (which drives local models with the same tools): the tool list, what the AI is told about
//! Ravune, and the line to a running Ravune's agent server.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

pub const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

pub const INSTRUCTIONS: &str = "Ravune is a DAW. Every edit is an operator: find them with search_operators and run them with run_operator. \
Read the project with query_project (ticks: 96 per beat, 24 per 16th step, 384 per bar), find sounds with browse_sounds and listen with \
render_and_analyze. Everything you change until checkpoint is one undo step for the user.";

/// Ravune's data folder, as the app finds it (eframe's storage dir, or `RAVUNE_DATA`).
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("RAVUNE_DATA") {
        return Some(dir.into());
    }
    let home = || std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Ravune").join("data"))
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library").join("Application Support").join("Ravune"))
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| home().map(|h| h.join(".local").join("share"))).map(|d| d.join("ravune"))
    }
}

pub fn tools() -> Value {
    let object = |props: Value, required: &[&str]| json!({ "type": "object", "properties": props, "required": required });
    json!([
        { "name": "search_operators", "description": "Find Ravune operators (every edit is one) by words in their id, label or description. Returns ids, descriptions and example props.",
          "inputSchema": object(json!({ "query": { "type": "string", "description": "e.g. \"note\", \"mixer send\", \"tempo\"; empty lists all" } }), &[]) },
        { "name": "run_operator", "description": "Run one operator with JSON props (see search_operators for examples). Edits until the next checkpoint undo together with one Ctrl+Z.",
          "inputSchema": object(json!({ "id": { "type": "string" }, "props": { "type": "object" } }), &["id"]) },
        { "name": "query_project", "description": "Read the project: a summary (tempo, key, channels, patterns, mixer), one pattern's notes, the playlist clips, or everything.",
          "inputSchema": object(json!({ "what": { "type": "string", "enum": ["summary", "pattern", "clips", "full"] }, "index": { "type": "integer", "description": "pattern index for what=pattern (default: the current one)" } }), &[]) },
        { "name": "browse_sounds", "description": "Search the sound library (built-in kit, the user's folders) by name. Paths go into channel.add's `sample`.",
          "inputSchema": object(json!({ "query": { "type": "string" }, "limit": { "type": "integer" } }), &["query"]) },
        { "name": "render_and_analyze", "description": "Render bars offline and describe them: peak and RMS level, energy per band, stereo width, onsets (in beats), key, tempo.",
          "inputSchema": object(json!({ "bars": { "type": "number", "description": "default 4, at most 32" }, "from_bar": { "type": "number" }, "mode": { "type": "string", "enum": ["song", "pattern"] } }), &[]) },
        { "name": "checkpoint", "description": "Close the current undo step: later edits undo separately.", "inputSchema": object(json!({}), &[]) },
        { "name": "undo", "description": "Take back the last undo step (everything since the last checkpoint, if you edited).", "inputSchema": object(json!({}), &[]) },
        { "name": "save_project", "description": "Save the project to its file (only if the user allowed files in Preferences → Agents).", "inputSchema": object(json!({}), &[]) },
    ])
}

/// The line to Ravune, opened on the first tool call.
pub struct Ravune {
    lines: BufReader<TcpStream>,
    out: TcpStream,
    next: u64,
}

impl Ravune {
    /// Connects to the Ravune whose data folder is `dir` (default: the one Ravune uses).
    pub fn connect(dir: Option<&Path>) -> Result<Self, String> {
        let off = "Ravune isn't listening: open Ravune, then Preferences → Agents → \"Let agents use Ravune\"";
        let file = dir.map(Path::to_path_buf).or_else(data_dir).ok_or(off)?.join("agent.json");
        let info: Value = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).ok_or(off)?;
        let port = info["port"].as_u64().ok_or(off)? as u16;
        let out = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("{off} ({e})"))?;
        let lines = BufReader::new(out.try_clone().map_err(|e| e.to_string())?);
        let mut ravune = Self { lines, out, next: 1 };
        ravune.call("hello", json!({ "token": info["token"] }))?;
        Ok(ravune)
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next;
        self.next += 1;
        writeln!(self.out, "{}", json!({ "id": id, "method": method, "params": params })).map_err(|e| e.to_string())?;
        let mut line = String::new();
        if self.lines.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err("Ravune closed the connection".into());
        }
        let reply: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        match reply.get("error") {
            Some(e) => Err(e.as_str().map_or_else(|| e.to_string(), String::from)),
            None => Ok(reply["result"].clone()),
        }
    }
}

