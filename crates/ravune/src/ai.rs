//! The AI providers behind the AI chat (ported from Velo): the user's own Claude Code or Codex
//! (their CLIs, signed in on their own), or a model behind an OpenAI-compatible endpoint
//! (Ollama, LM Studio, llama.cpp's server, or a cloud API). Every one of them gets nothing but
//! Ravune's tools: the CLIs through the `ravune-mcp` bridge, a local model through Ravune's own
//! loop over the same agent server. Each turns its output into the same [`Event`]s for the chat.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use eframe::egui;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What the AI is told besides the user's words.
pub const BRIEF: &str = "You are the AI inside Ravune, the music app (a DAW) the user is looking at. Use the ravune tools to read and \
change the open project: start with query_project, find the edit you need with search_operators and run it with run_operator, find \
sounds with browse_sounds. The user hears every change at once. When you change the sound, listen with render_and_analyze before you \
answer. Pictures the user attaches are references (a screenshot of a pattern or a mixer, a chord chart): follow them. Don't call \
checkpoint: each of your replies undoes in one step. Use only the ravune tools. Answer briefly, in the user's language: a sentence or \
two, short Markdown (bold, lists, `code`) when it helps. Never write em dashes.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    #[default]
    ClaudeCode,
    Codex,
    /// A model behind an OpenAI-compatible endpoint.
    Local,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::ClaudeCode, Provider::Codex, Provider::Local];

    pub fn name(self) -> &'static str {
        match self {
            Provider::ClaudeCode => "Claude Code",
            Provider::Codex => "Codex",
            Provider::Local => "Local or API model",
        }
    }

    /// Its CLI, for the CLIs.
    pub fn program(self) -> Option<&'static str> {
        match self {
            Provider::ClaudeCode => Some("claude"),
            Provider::Codex => Some("codex"),
            Provider::Local => None,
        }
    }

    /// Its model in the prefs; empty for a CLI's default.
    pub fn model(self, prefs: &mut Prefs) -> &mut String {
        match self {
            Provider::ClaudeCode => &mut prefs.claude_model,
            Provider::Codex => &mut prefs.codex_model,
            Provider::Local => &mut prefs.model,
        }
    }

    /// Whether its model looks at pictures: Claude's and Codex's all do.
    pub fn sees_images(self, prefs: &Prefs) -> bool {
        self != Provider::Local || prefs.sees_images
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub provider: Provider,
    /// The local model's OpenAI-compatible endpoint, up to `/v1`.
    pub endpoint: String,
    pub model: String,
    /// The local model looks at pictures (a vision model).
    pub sees_images: bool,
    /// Codex's model, empty for its default.
    pub codex_model: String,
    /// Claude Code's model (an alias like `opus` or a full id), empty for its default.
    pub claude_model: String,
    /// How long Claude Code's or Codex's model thinks before it acts: one of [`EFFORTS`].
    pub effort: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            provider: Provider::ClaudeCode,
            endpoint: "http://localhost:11434/v1".into(),
            model: String::new(),
            sees_images: false,
            codex_model: String::new(),
            claude_model: String::new(),
            effort: "high".into(),
        }
    }
}

/// Claude Code's model aliases, each the newest of its line, and the model that is today (only
/// the name shown: Claude Code resolves the alias itself).
pub const CLAUDE_MODELS: [(&str, &str); 4] = [("fable", "Fable 5.1"), ("opus", "Opus 5.5"), ("sonnet", "Sonnet 5.5"), ("haiku", "Haiku 5.5")];

/// The efforts both CLIs take, from faster to smarter (Claude Code's `--effort`, Codex's
/// `model_reasoning_effort`).
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// A model as people know it: Opus 5.5 for Claude Code's `opus`, Codex's own name for its id,
/// anything else as it is.
pub fn model_name<'a>(model: &'a str, codex: &'a [(String, String)]) -> &'a str {
    let claude = CLAUDE_MODELS.iter().map(|(id, name)| (*id, *name));
    claude.chain(codex.iter().map(|(id, name)| (id.as_str(), name.as_str()))).find(|(id, _)| *id == model).map_or(model, |(_, name)| name)
}

/// Codex's models when it has no list of its own yet.
const CODEX_MODELS: [&str; 3] = ["gpt-6.1-sol", "gpt-6-astra", "gpt-6-luna"];

/// Services to connect in a click: name, OpenAI-compatible endpoint, where to get a key (none:
/// it runs on this computer).
pub const PRESETS: [(&str, &str, Option<&str>); 7] = [
    ("Ollama", "http://localhost:11434/v1", None),
    ("LM Studio", "http://localhost:1234/v1", None),
    ("OpenAI", "https://api.openai.com/v1", Some("https://platform.openai.com/api-keys")),
    ("Anthropic", "https://api.anthropic.com/v1", Some("https://console.anthropic.com/settings/keys")),
    ("Gemini", "https://generativelanguage.googleapis.com/v1beta/openai", Some("https://aistudio.google.com/apikey")),
    ("OpenRouter", "https://openrouter.ai/api/v1", Some("https://openrouter.ai/keys")),
    ("Groq", "https://api.groq.com/openai/v1", Some("https://console.groq.com/keys")),
];

/// The preset an endpoint is, if any.
pub fn preset(endpoint: &str) -> Option<usize> {
    PRESETS.iter().position(|p| host(p.1) == host(endpoint))
}

/// Whether a model's name says it looks at pictures: a guess, the menu's tick corrects it.
pub fn sees(model: &str) -> bool {
    let m = model.to_lowercase();
    ["vision", "-vl", "vl:", "llava", "gpt-4o", "gpt-4.1", "gpt-5", "gpt-6", "claude", "gemini", "gemma3", "pixtral", "minicpm-v", "moondream", "llama4", "mistral-small"].iter().any(|k| m.contains(k))
}

/// The models Codex offers, from the list it keeps (~/.codex/models_cache.json): each one's id
/// and name.
pub fn codex_models() -> Vec<(String, String)> {
    let dir = std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".codex")));
    let cache = dir.and_then(|d| std::fs::read_to_string(d.join("models_cache.json")).ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or_default();
    let listed: Vec<(String, String)> = cache["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["visibility"] != "hide")
        .filter_map(|m| {
            let id = m["slug"].as_str()?;
            Some((id.to_string(), m["display_name"].as_str().unwrap_or(id).to_string()))
        })
        .collect();
    if listed.is_empty() { CODEX_MODELS.map(|m| (m.to_string(), m.to_string())).into() } else { listed }
}

/// Tries a provider: a CLI installed and signed in, an endpoint answering. What it found, or
/// what's wrong (on a worker thread: a CLI takes a second to say).
pub fn check(prefs: &Prefs, provider: Provider) -> Result<String, String> {
    let Some(program) = provider.program() else {
        return match models(&prefs.endpoint)?.len() {
            0 => Err(tr!("It answers, but offers no models.").into()),
            n => Ok(tr!("Connected: {} models", n)),
        };
    };
    let exe = find(program).ok_or_else(|| tr!("{} isn't installed here (looked for `{}`).", provider.name(), program))?;
    let args: Vec<String> = if provider == Provider::ClaudeCode { vec!["auth".into(), "status".into()] } else { vec!["login".into(), "status".into()] };
    let out = spawn(&exe, &args, &workdir()).and_then(Child::wait_with_output).map_err(|e| tr!("{} didn't start: {}", provider.name(), e))?;
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if provider == Provider::ClaudeCode {
        let v: Value = serde_json::from_str(&said).unwrap_or_default();
        return match (v["loggedIn"] == true || v["authMethod"].as_str().is_some_and(|m| m != "none"), v["email"].as_str()) {
            (true, Some(email)) => Ok(tr!("Signed in as {}", email)),
            (true, None) => Ok(tr!("Signed in").into()),
            _ => Err(tr!("Signed out: run claude auth login in a terminal (the Claude app's sign-in doesn't carry over).").into()),
        };
    }
    match said.lines().map(str::trim).find(|l| !l.is_empty()) {
        Some(line) if out.status.success() && line.to_lowercase().contains("logged in") => Ok(line.to_string()),
        _ => Err(tr!("Signed out: run codex login in a terminal.").into()),
    }
}

/// The local servers answering on their usual ports: which preset, and its models.
pub fn detect() -> Vec<(usize, Vec<String>)> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(3))).timeout_connect(Some(Duration::from_millis(800))).http_status_as_error(false).build().into();
    std::thread::scope(|s| {
        let asks: Vec<_> = PRESETS.iter().enumerate().filter(|(_, p)| p.2.is_none()).map(|(i, p)| (i, s.spawn(|| models_via(&agent, p.1)))).collect();
        asks.into_iter().filter_map(|(i, ask)| ask.join().ok()?.ok().map(|m| (i, m))).collect()
    })
}

/// What a reply did, as it happened.
#[derive(Debug, PartialEq)]
pub enum Event {
    /// The CLI's session (Claude Code) or thread (Codex): the next message carries on with it.
    Session(String),
    Text(String),
    /// A command started (its id, and what it is, short).
    Tool(String, String),
    /// A command ended: well, or with what went wrong.
    ToolDone(String, Option<String>),
    Problem(String),
    /// The local model's conversation so far: what it remembers next time.
    History(Vec<Value>),
}

/// One reply on its way.
pub struct Run {
    child: Option<Child>,
    pub events: Receiver<Event>,
    stop: Arc<AtomicBool>,
}

impl Drop for Run {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(child) = &mut self.child {
            let _ = child.kill(); // stopped, or the chat closed; done already is fine too
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
impl Run {
    pub fn for_test(events: Receiver<Event>) -> Self {
        Self { child: None, events, stop: Arc::new(AtomicBool::new(false)) }
    }
}

// ---- finding the CLIs ------------------------------------------------------------------------

/// Where CLIs live besides the PATH: an app opened from the Finder gets a bare PATH, while
/// installers put claude in ~/.local/bin, npm and Homebrew elsewhere.
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(home) = home() {
        for d in [".local/bin", ".claude/local", ".npm-global/bin", ".bun/bin", ".volta/bin", "AppData/Roaming/npm", "AppData/Local/Programs/OpenAI/Codex/bin"] {
            dirs.push(home.join(d));
        }
        // nvm: every installed Node, newest last
        if let Ok(nodes) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            let mut found: Vec<PathBuf> = nodes.filter_map(|e| e.ok().map(|e| e.path().join("bin"))).collect();
            found.sort();
            dirs.extend(found.into_iter().rev());
        }
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    dirs
}

/// A CLI's path, wherever it's installed.
pub fn find(program: &str) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) { vec![format!("{program}.cmd"), format!("{program}.exe")] } else { vec![program.to_string()] };
    search_dirs().into_iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

/// [`find`], looked for again at most every two seconds: the AI panel asks every frame, and a
/// search of the PATH is dozens of file checks.
pub fn found(program: &str) -> Option<PathBuf> {
    static SEEN: std::sync::Mutex<Vec<(String, std::time::Instant, Option<PathBuf>)>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().ok()?;
    seen.retain(|(_, at, _)| at.elapsed() < Duration::from_secs(2));
    if let Some((_, _, path)) = seen.iter().find(|(p, ..)| p == program) {
        return path.clone();
    }
    let path = find(program);
    seen.push((program.to_string(), std::time::Instant::now(), path.clone()));
    path
}

/// Runs a CLI with its own folder first on the PATH (npm's scripts start with `env node`, and
/// Node sits beside them), in a folder of its own, with no console on Windows.
fn spawn(program: &Path, args: &[String], dir: &Path) -> std::io::Result<Child> {
    let mut path: Vec<PathBuf> = program.parent().map(Path::to_path_buf).into_iter().collect();
    path.extend(search_dirs());
    let mut command = Command::new(program);
    command.args(args).current_dir(dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Ok(joined) = std::env::join_paths(path) {
        command.env("PATH", joined);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command.spawn()
}

/// Where the CLIs run: an empty folder in Ravune's data folder (nothing there to read).
fn workdir() -> PathBuf {
    let dir = crate::files::data_dir().unwrap_or_else(std::env::temp_dir).join("ai");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

// ---- reading what the CLIs say ---------------------------------------------------------------

/// A command, short: `mixer.set {"low":-3.0,"track":1}`, `query project`; a script's code
/// left out (it's in the project).
pub fn label(name: &str, input: &Value) -> String {
    let name = name.strip_prefix("mcp__ravune__").unwrap_or(name);
    match (name, input["id"].as_str(), &input["props"]) {
        ("run_operator", Some(id), Value::Null) => id.into(),
        ("run_operator", Some(id), props) => {
            let mut props = props.clone();
            if let Some(code) = props.get_mut("code") {
                *code = json!("…");
            }
            format!("{id} {props}")
        }
        _ => name.replace('_', " "),
    }
}

/// One line of `claude -p --output-format stream-json`.
pub fn claude_line(line: &str) -> Vec<Event> {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    let mut out = Vec::new();
    match v["type"].as_str() {
        Some("system") if v["subtype"] == "init" => out.extend(v["session_id"].as_str().map(|id| Event::Session(id.into()))),
        Some("assistant") => {
            for block in v["message"]["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => out.extend(block["text"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(|s| Event::Text(s.into()))),
                    Some("tool_use") => out.push(Event::Tool(block["id"].as_str().unwrap_or_default().into(), label(block["name"].as_str().unwrap_or("?"), &block["input"]))),
                    _ => {}
                }
            }
        }
        Some("user") => {
            for block in v["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                let said = || match &block["content"] {
                    Value::String(s) => s.trim().to_string(),
                    parts => parts.as_array().into_iter().flatten().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("
"),
                };
                out.push(Event::ToolDone(block["tool_use_id"].as_str().unwrap_or_default().into(), (block["is_error"] == true).then(said)));
            }
        }
        Some("result") if v["is_error"] == true => out.push(Event::Problem(v["result"].as_str().unwrap_or("Claude Code stopped").trim().into())),
        _ => {}
    }
    out
}

/// One line of `codex exec --json`.
pub fn codex_line(line: &str) -> Vec<Event> {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    let item = &v["item"];
    let id = || item["id"].as_str().unwrap_or_default().to_string();
    match (v["type"].as_str(), item["type"].as_str()) {
        (Some("thread.started"), _) => v["thread_id"].as_str().map(|t| vec![Event::Session(t.into())]).unwrap_or_default(),
        (Some("item.started"), Some("mcp_tool_call")) => vec![Event::Tool(id(), label(item["tool"].as_str().unwrap_or("?"), &item["arguments"]))],
        (Some("item.started"), Some("command_execution")) => vec![Event::Tool(id(), format!("shell {}", item["command"].as_str().unwrap_or("")))],
        (Some("item.completed"), Some("mcp_tool_call" | "command_execution")) => {
            let failed = item["status"] == "failed" || !item["error"].is_null();
            vec![Event::ToolDone(id(), failed.then(|| item["error"]["message"].as_str().or(item["error"].as_str()).unwrap_or("failed").to_string()))]
        }
        (Some("item.completed"), Some("agent_message")) => item["text"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(|s| vec![Event::Text(s.into())]).unwrap_or_default(),
        (Some("item.completed"), Some("error")) | (Some("error"), _) => vec![Event::Problem(item["message"].as_str().or(v["message"].as_str()).unwrap_or("Codex stopped").into())],
        (Some("turn.failed"), _) => vec![Event::Problem(v["error"]["message"].as_str().unwrap_or("Codex stopped").into())],
        _ => Vec::new(),
    }
}

/// Where pictures attached to messages are kept.
pub fn pictures_dir() -> PathBuf {
    crate::files::data_dir().unwrap_or_else(std::env::temp_dir).join("pictures")
}

/// Keeps a picture attached to a message: at most 1568 px a side (what the models look at
/// anyway), a JPEG named by what's in it.
pub fn keep_picture(picture: image::DynamicImage) -> Result<PathBuf, String> {
    use std::hash::{Hash, Hasher};
    let picture = if picture.width().max(picture.height()) > 1568 { picture.resize(1568, 1568, image::imageops::FilterType::Triangle) } else { picture };
    let mut jpeg = Vec::new();
    picture.to_rgb8().write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 88)).map_err(|e| e.to_string())?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    jpeg.hash(&mut hash);
    let dir = pictures_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{:016x}.jpg", hash.finish()));
    std::fs::write(&path, &jpeg).map_err(|e| e.to_string())?;
    Ok(path)
}

fn base64_of(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| crate::agent::base64(&bytes))
}

/// Starts a reply: `session` carries on a CLI's conversation, `history` a local model's;
/// `pictures` go with the message.
pub fn start(prefs: &Prefs, provider: Provider, prompt: &str, pictures: &[PathBuf], session: Option<&str>, history: Vec<Value>, ctx: egui::Context) -> Result<Run, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, events) = channel();
    if provider == Provider::Local {
        let (prefs, prompt, pictures, stop2) = (prefs.clone(), prompt.to_string(), pictures.to_vec(), stop.clone());
        std::thread::Builder::new().name("ai".into()).spawn(move || local(&prefs, &prompt, &pictures, history, &tx, &stop2, &ctx)).map_err(|e| e.to_string())?;
        return Ok(Run { child: None, events, stop });
    }
    let bridge = crate::agent::bridge();
    if !bridge.exists() {
        return Err(tr!("ravune-mcp isn't next to Ravune ({}): the AI talks to Ravune through it", bridge.display()));
    }
    let program = provider.program().unwrap_or_default();
    let exe = find(program).ok_or_else(|| tr!("{} isn't installed here (looked for `{}`).", provider.name(), program))?;
    let bridge = bridge.to_string_lossy().to_string();
    // the bridge is told where Ravune's data is: clients start it with their own environment
    let data_args: Vec<String> = crate::files::data_dir().map(|d| vec!["--data".to_string(), d.to_string_lossy().to_string()]).unwrap_or_default();
    let pictures: Vec<String> = pictures.iter().map(|p| p.to_string_lossy().to_string()).collect();
    let effort = EFFORTS.iter().find(|e| **e == prefs.effort);
    let mut input = prompt.to_string();
    let args: Vec<String> = match provider {
        Provider::ClaudeCode => {
            let mcp = json!({ "mcpServers": { "ravune": { "command": bridge, "args": data_args } } }).to_string();
            // nothing but Ravune's tools; none of the user's own settings, hooks or MCP servers
            let mut a: Vec<String> = ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--tools", "", "--setting-sources", "", "--strict-mcp-config", "--mcp-config"].map(String::from).into();
            a.extend([mcp, "--allowedTools".into(), "mcp__ravune".into(), "--append-system-prompt".into(), BRIEF.into()]);
            if !prefs.claude_model.trim().is_empty() {
                a.extend(["--model".into(), prefs.claude_model.trim().into()]);
            }
            if let Some(effort) = effort {
                a.extend(["--effort".into(), effort.to_string()]);
            }
            if let Some(id) = session {
                a.extend(["--resume".into(), id.into()]);
            }
            // the message as a JSON line: its words and its pictures
            let mut content = vec![json!({ "type": "text", "text": prompt })];
            content.extend(pictures.iter().filter_map(|p| base64_of(Path::new(p))).map(|data| json!({ "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": data } })));
            input = format!("{}\n", json!({ "type": "user", "message": { "role": "user", "content": content } }));
            a
        }
        _ => {
            // TOML values: JSON strings are valid TOML basic strings
            let toml = |s: &str| serde_json::to_string(s).unwrap_or_default();
            // pictures right after exec: `--image` takes every value up to the next flag
            let images = || pictures.iter().flat_map(|p| ["--image".to_string(), p.clone()]);
            let mut a: Vec<String> = vec!["exec".into()];
            if session.is_none() {
                a.extend(images());
            }
            a.extend(["--json", "--skip-git-repo-check", "--ignore-user-config", "--sandbox", "read-only"].map(String::from));
            a.extend(["-c".into(), format!("mcp_servers.ravune.command={}", toml(&bridge)), "-c".into(), format!("mcp_servers.ravune.args={}", serde_json::to_string(&data_args).unwrap_or_default())]);
            // Ravune's tools run without asking (Ravune has its own permissions); with nobody to
            // ask, Codex would refuse them
            a.extend(["-c".into(), "mcp_servers.ravune.default_tools_approval_mode=\"approve\"".into()]);
            a.extend(["-c".into(), "approval_policy=\"never\"".into(), "-c".into(), format!("developer_instructions={}", toml(BRIEF))]);
            if !prefs.codex_model.trim().is_empty() {
                a.extend(["-m".into(), prefs.codex_model.trim().into()]);
            }
            if let Some(effort) = effort {
                a.extend(["-c".into(), format!("model_reasoning_effort={}", toml(effort))]);
            }
            if let Some(id) = session {
                a.push("resume".into());
                a.extend(images());
                a.push(id.into());
            }
            a.push("-".into()); // the prompt on stdin
            a
        }
    };
    let mut child = spawn(&exe, &args, &workdir()).map_err(|e| tr!("{} didn't start: {}", provider.name(), e))?;
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take().ok_or("no output")?, child.stderr.take());
    std::thread::Builder::new()
        .name("ai".into())
        .spawn(move || {
            if let Some(mut stdin) = stdin {
                let _ = stdin.write_all(input.as_bytes()); // closed when it drops: the end of the message
            }
            // what it says on stderr, for when it stops without saying why
            let err = std::thread::spawn(move || stderr.map(|e| BufReader::new(e).lines().map_while(Result::ok).collect::<Vec<_>>().join("\n")).unwrap_or_default());
            let mut said = false;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let parsed = if provider == Provider::Codex { codex_line(&line) } else { claude_line(&line) };
                said |= !parsed.is_empty();
                for event in parsed {
                    if tx.send(event).is_err() {
                        return;
                    }
                }
                ctx.request_repaint();
            }
            let err = err.join().unwrap_or_default();
            if !said && !err.trim().is_empty() {
                let _ = tx.send(Event::Problem(err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default().trim().to_string()));
            }
            drop(tx); // done: the panel sees the channel close
            ctx.request_repaint();
        })
        .map_err(|e| e.to_string())?;
    Ok(Run { child: Some(child), events, stop })
}

// ---- a model behind an OpenAI-compatible endpoint, local or not ----

/// What the system keychain files Ravune's keys under.
const KEYCHAIN: &str = "Ravune";

/// An endpoint's host ("api.openai.com"): what its key is filed under.
pub fn host(endpoint: &str) -> String {
    let rest = endpoint.trim().split_once("://").map_or(endpoint.trim(), |(_, r)| r);
    rest.split('/').next().unwrap_or(rest).to_string()
}

/// An endpoint's API key, from the system keychain: never in Ravune's files.
pub fn key(endpoint: &str) -> Option<String> {
    keyring::Entry::new(KEYCHAIN, &host(endpoint)).ok()?.get_password().ok().filter(|k| !k.is_empty())
}

/// Saves an endpoint's API key in the system keychain; an empty one removes it.
pub fn set_key(endpoint: &str, key: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYCHAIN, &host(endpoint)).map_err(|e| e.to_string())?;
    let done = if key.trim().is_empty() { entry.delete_credential().or_else(|e| if matches!(e, keyring::Error::NoEntry) { Ok(()) } else { Err(e) }) } else { entry.set_password(key.trim()) };
    done.map_err(|e| e.to_string())
}

/// A request that didn't get through, said plainly.
fn unreached(endpoint: &str, e: ureq::Error) -> String {
    match e {
        ureq::Error::Io(ref io) if io.kind() == std::io::ErrorKind::ConnectionRefused => tr!("Nothing answers at {}: is it running?", host(endpoint)),
        ureq::Error::ConnectionFailed | ureq::Error::Timeout(_) => tr!("Nothing answers at {}: is it running?", host(endpoint)),
        ureq::Error::HostNotFound => tr!("{} wasn't found: check the endpoint.", host(endpoint)),
        e => e.to_string(),
    }
}

fn http() -> ureq::Agent {
    // a local model on a laptop can take minutes for one step
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(600))).timeout_connect(Some(Duration::from_secs(5))).http_status_as_error(false).build().into()
}

fn read(mut response: ureq::http::Response<ureq::Body>) -> Result<Value, String> {
    let status = response.status();
    let text = response.body_mut().with_config().limit(64 << 20).read_to_string().map_err(|e| e.to_string())?;
    if !status.is_success() {
        let message = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"]["message"].as_str().or(v["error"].as_str()).map(String::from)).unwrap_or(text);
        return Err(format!("{status}: {message}"));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// The models an OpenAI-compatible endpoint offers.
pub fn models(endpoint: &str) -> Result<Vec<String>, String> {
    models_via(&http(), endpoint)
}

fn models_via(agent: &ureq::Agent, endpoint: &str) -> Result<Vec<String>, String> {
    let url = format!("{}/models", endpoint.trim().trim_end_matches('/'));
    let mut request = agent.get(&url);
    if let Some(key) = key(endpoint) {
        request = request.header("Authorization", format!("Bearer {key}"));
        // Anthropic lists its models with its own headers only
        if host(endpoint) == "api.anthropic.com" {
            request = request.header("x-api-key", &key).header("anthropic-version", "2023-06-01");
        }
    }
    let v = read(request.call().map_err(|e| unreached(endpoint, e))?)?;
    Ok(v["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(String::from)).collect())
}

/// Calls a model wrote as JSON in its text instead of making them (small local models do):
/// `{"name": …, "arguments": …}` objects, naming a tool or an operator.
fn written_calls(text: &str) -> Vec<(String, Value)> {
    let tools: Vec<String> = ravune_mcp::tools().as_array().into_iter().flatten().filter_map(|t| t["name"].as_str().map(String::from)).collect();
    let mut calls = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('{') {
        let mut values = serde_json::Deserializer::from_str(&rest[at..]).into_iter::<Value>();
        match values.next() {
            Some(Ok(v)) => {
                if let Some(name) = v["name"].as_str() {
                    let args = v.get("arguments").or_else(|| v.get("parameters")).cloned().unwrap_or(json!({}));
                    if tools.iter().any(|t| t == name) {
                        calls.push((name.to_string(), args));
                    } else if name.contains('.') {
                        calls.push(("run_operator".into(), json!({ "id": name, "props": args })));
                    }
                }
                rest = &rest[at + values.byte_offset()..];
            }
            _ => rest = &rest[at + 1..],
        }
    }
    calls
}

/// Ravune's tools as OpenAI functions.
fn functions() -> Vec<Value> {
    ravune_mcp::tools().as_array().into_iter().flatten().map(|t| json!({ "type": "function", "function": { "name": t["name"], "description": t["description"], "parameters": t["inputSchema"] } })).collect()
}

/// A local model's turn: ask, run the commands it calls through Ravune's agent server, give it
/// the results, until it answers in words (at most 24 steps).
fn local(prefs: &Prefs, prompt: &str, pictures: &[PathBuf], mut history: Vec<Value>, tx: &Sender<Event>, stop: &AtomicBool, ctx: &egui::Context) {
    let say = |e: Event| {
        let _ = tx.send(e);
        ctx.request_repaint();
    };
    if prefs.model.trim().is_empty() {
        return say(Event::Problem(tr!("Pick a local model first (the menu under the box lists what the endpoint offers).").into()));
    }
    if history.is_empty() {
        history.push(json!({ "role": "system", "content": format!("{BRIEF}\n\n{}", ravune_mcp::INSTRUCTIONS) }));
    }
    let pictures: Vec<String> = pictures.iter().filter(|_| prefs.sees_images).filter_map(|p| base64_of(p)).collect();
    if pictures.is_empty() {
        history.push(json!({ "role": "user", "content": prompt }));
    } else {
        let mut parts = vec![json!({ "type": "text", "text": prompt })];
        parts.extend(pictures.iter().map(|b| json!({ "type": "image_url", "image_url": { "url": format!("data:image/jpeg;base64,{b}") } })));
        history.push(json!({ "role": "user", "content": parts }));
    }
    // the agents' server starts with the first message: give it a moment
    let dir = crate::files::data_dir();
    let mut ravune = None;
    for _ in 0..30 {
        match ravune_mcp::Ravune::connect(dir.as_deref()) {
            Ok(r) => {
                ravune = Some(r);
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    let Some(mut ravune) = ravune else { return say(Event::Problem(tr!("Ravune's agent server didn't start (Preferences → Agents).").into())) };
    let url = format!("{}/chat/completions", prefs.endpoint.trim().trim_end_matches('/'));
    let tools = functions();
    let auth = key(&prefs.endpoint).map(|k| format!("Bearer {k}"));
    for _ in 0..24 {
        if stop.load(Relaxed) {
            return;
        }
        let body = json!({ "model": prefs.model.trim(), "messages": history, "tools": tools, "stream": false });
        let mut request = http().post(&url).header("Content-Type", "application/json");
        if let Some(auth) = &auth {
            request = request.header("Authorization", auth);
        }
        let reply = match request.send(body.to_string()).map_err(|e| unreached(&prefs.endpoint, e)).and_then(read) {
            Ok(v) => v,
            Err(e) => return say(Event::Problem(tr!("The local model didn't answer ({}): {}", prefs.endpoint, e))),
        };
        let message = &reply["choices"][0]["message"];
        let content = message["content"].as_str().unwrap_or("");
        let calls = message["tool_calls"].as_array().cloned().unwrap_or_default();
        history.push(json!({ "role": "assistant", "content": content, "tool_calls": if calls.is_empty() { Value::Null } else { json!(calls) } }));
        // smaller models write their calls as JSON in the text instead of making them
        let written = if calls.is_empty() { written_calls(content) } else { Vec::new() };
        let calls: Vec<(String, String, Value)> = if written.is_empty() {
            calls
                .iter()
                .enumerate()
                .map(|(n, call)| {
                    let id = call["id"].as_str().map_or_else(|| format!("call_{n}"), String::from);
                    // arguments come as a JSON string (some servers send the object itself)
                    let args = match &call["function"]["arguments"] {
                        Value::String(s) => serde_json::from_str(s).unwrap_or(json!({})),
                        v => v.clone(),
                    };
                    (id, call["function"]["name"].as_str().unwrap_or("").to_string(), args)
                })
                .collect()
        } else {
            written.into_iter().enumerate().map(|(n, (name, args))| (format!("written_{}_{n}", history.len()), name, args)).collect()
        };
        let in_text = calls.iter().any(|c| c.0.starts_with("written_"));
        if !in_text && let Some(text) = Some(content.trim()).filter(|t| !t.is_empty()) {
            say(Event::Text(text.into()));
        }
        if calls.is_empty() {
            break;
        }
        let mut results = Vec::new();
        for (id, name, args) in calls {
            say(Event::Tool(id.clone(), label(&name, &args)));
            let result = ravune.call(&name, args);
            say(Event::ToolDone(id.clone(), result.as_ref().err().cloned()));
            let answer = match result {
                Ok(v) => v.to_string(),
                Err(e) => format!("error: {e}"),
            };
            if in_text {
                results.push(format!("{name}: {answer}"));
            } else {
                history.push(json!({ "role": "tool", "tool_call_id": id, "content": answer }));
            }
        }
        // calls written as text get their results as a message
        if in_text {
            history.push(json!({ "role": "user", "content": format!("Results of those calls:\n{}", results.join("\n")) }));
        }
    }
    // the pictures stay out of what's kept, the user's words stay: they're big and stale by the
    // next message
    history.retain_mut(|m| {
        let Some(parts) = m["content"].as_array().filter(|_| m["role"] == "user") else { return true };
        let words = parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n");
        m["content"] = json!(words);
        !words.is_empty()
    });
    say(Event::History(history));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_show_by_their_names() {
        let codex = [("gpt-6.1-sol".to_string(), "GPT-6.1-Sol".to_string())];
        assert_eq!(model_name("opus", &codex), "Opus 5.5");
        assert_eq!(model_name("gpt-6.1-sol", &codex), "GPT-6.1-Sol");
        assert_eq!(model_name("qwen2.5:14b", &codex), "qwen2.5:14b");
    }

    #[test]
    fn claude_and_codex_output_become_the_same_events() {
        let claude = [
            r#"{"type":"system","subtype":"init","session_id":"s1","tools":[]}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":" Sure. "},{"type":"tool_use","id":"t1","name":"mcp__ravune__run_operator","input":{"id":"script.set","props":{"index":0,"code":"print(1)"}}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate","session_id":"s1"}"#,
            "not json",
        ];
        assert_eq!(
            claude.iter().flat_map(|l| claude_line(l)).collect::<Vec<_>>(),
            vec![Event::Session("s1".into()), Event::Text("Sure.".into()), Event::Tool("t1".into(), r#"script.set {"code":"…","index":0}"#.into()), Event::ToolDone("t1".into(), None), Event::Problem("Failed to authenticate".into()),]
        );
        let codex = [
            r#"{"type":"thread.started","thread_id":"th1"}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.started","item":{"id":"i1","type":"mcp_tool_call","server":"ravune","tool":"query_project","arguments":{},"status":"in_progress"}}"#,
            r#"{"type":"item.completed","item":{"id":"i1","type":"mcp_tool_call","server":"ravune","tool":"query_project","arguments":{},"status":"completed"}}"#,
            r#"{"type":"item.completed","item":{"id":"i2","type":"reasoning","text":"thinking"}}"#,
            r#"{"type":"item.completed","item":{"id":"i3","type":"agent_message","text":"Done."}}"#,
            r#"{"type":"turn.failed","error":{"message":"usage limit"}}"#,
        ];
        assert_eq!(
            codex.iter().flat_map(|l| codex_line(l)).collect::<Vec<_>>(),
            vec![Event::Session("th1".into()), Event::Tool("i1".into(), "query project".into()), Event::ToolDone("i1".into(), None), Event::Text("Done.".into()), Event::Problem("usage limit".into())]
        );
    }

    #[test]
    fn keys_are_filed_by_host() {
        assert_eq!(host("https://api.openai.com/v1"), "api.openai.com");
        assert_eq!(host(" http://localhost:11434/v1/ "), "localhost:11434");
        assert_eq!(host("openrouter.ai/api/v1"), "openrouter.ai");
    }

    #[test]
    fn services_and_eyes_are_told_by_name() {
        assert_eq!(preset("http://localhost:11434/v1/"), Some(0), "Ollama");
        assert_eq!(preset("https://openrouter.ai/api/v1"), Some(5));
        assert_eq!(preset("http://192.168.1.4:8080/v1"), None, "a server of the user's own");
        assert!(["llava:13b", "qwen2.5vl:7b", "gpt-4o-mini", "claude-sonnet-5-5", "gemini-2.5-flash", "llama3.2-vision"].iter().all(|m| sees(m)));
        assert!(!["qwen2.5:14b", "llama3.1:8b", "deepseek-r1"].iter().any(|m| sees(m)));
    }

    #[test]
    fn calls_written_as_text_are_found() {
        // what qwen2.5-coder answered in a real run
        let text = "{\n    \"name\": \"pattern.add\",\n    \"arguments\": {}\n}\n\n{\n    \"name\": \"query_project\"\n}\nThen { not json";
        assert_eq!(written_calls(text), [("run_operator".to_string(), json!({ "id": "pattern.add", "props": {} })), ("query_project".to_string(), json!({}))]);
        assert!(written_calls("A four-bar groove, done.").is_empty());
    }

    #[test]
    fn ravunes_tools_are_offered_as_functions() {
        let f = functions();
        assert_eq!(f.len(), 8);
        assert!(f.iter().all(|t| t["type"] == "function" && t["function"]["parameters"]["type"] == "object"));
    }
}
