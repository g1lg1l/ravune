//! Agents (M8): Ravune answers MCP clients (Claude and others) through `ravune-mcp`, a
//! small stdio bridge that talks to this JSON-lines server on localhost. Off until turned on
//! in Preferences → Agents. The user decides what agents may do: read (always), edit (run
//! operators) and files (save the project). What an agent changes until its next checkpoint
//! undoes in one step, and every call lands in a log the user can read.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText, Ui};
use ravune_core::{BAR, ClipKind, Instrument, OPERATORS, PPQ, STEP};
use ravune_engine::{BANDS, Mode, Song, Transport, analyze, render_span};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::theme::Theme;
use crate::{App, icons, kit};

pub const PORT: u16 = 7317;
/// What `ravune-mcp` reads to find and open the server: `{ "port", "token" }`.
pub const CONNECTION_FILE: &str = "agent.json";
const LOG_LINES: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub enabled: bool,
    pub port: u16,
    /// Agents may run operators (change the project).
    pub edit: bool,
    /// Agents may save the project.
    pub files: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { enabled: false, port: PORT, edit: true, files: false }
    }
}

/// A request from a connection, answered on the UI thread.
struct Call {
    method: String,
    params: Value,
    reply: Sender<Result<Value, String>>,
}

pub struct Line {
    /// Seconds since Ravune started.
    pub at: f64,
    pub what: String,
    pub ok: bool,
}

#[derive(Default)]
pub struct Agent {
    server: Option<Server>,
    pub log: Vec<Line>,
    /// The agent's edits since its last checkpoint are one open undo group.
    group: bool,
    /// Connections last frame (a drop to none closes the group).
    clients: usize,
}

struct Server {
    port: u16,
    calls: Receiver<Call>,
    stop: Arc<AtomicBool>,
    clients: Arc<AtomicUsize>,
    file: Option<PathBuf>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(file) = &self.file {
            let _ = std::fs::remove_file(file);
        }
    }
}

/// A secret for the connection file, so only someone who can read the user's own files can
/// talk to Ravune (a web page posting to localhost can't).
// ponytail: std's randomly keyed hasher plus the clock, not a CSPRNG; fine for a local secret
fn token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    (0..2u64)
        .map(|i| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(nanos ^ (i as u128) << 64);
            h.write_u32(std::process::id());
            format!("{:016x}", h.finish())
        })
        .collect()
}

fn listen(listener: TcpListener, token: String, calls: Sender<Call>, stop: Arc<AtomicBool>, clients: Arc<AtomicUsize>, ctx: egui::Context) {
    let _ = listener.set_nonblocking(true);
    while !stop.load(Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let (token, calls, stop, clients, ctx) = (token.clone(), calls.clone(), stop.clone(), clients.clone(), ctx.clone());
                std::thread::spawn(move || serve(stream, &token, calls, stop, clients, ctx));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn send(stream: &mut TcpStream, id: &Value, result: Result<Value, String>) -> std::io::Result<()> {
    let line = match result {
        Ok(v) => json!({ "id": id, "result": v }),
        Err(e) => json!({ "id": id, "error": e }),
    };
    writeln!(stream, "{line}")
}

/// One connection: a hello carrying the token first (anything else is dropped), then one
/// JSON request per line, each answered by the UI thread.
fn serve(stream: TcpStream, token: &str, calls: Sender<Call>, stop: Arc<AtomicBool>, clients: Arc<AtomicUsize>, ctx: egui::Context) {
    let _ = stream.set_nonblocking(false);
    let Ok(mut out) = stream.try_clone() else { return };
    let mut lines = BufReader::new(stream).lines();
    let Some(Ok(first)) = lines.next() else { return };
    let hello: Value = serde_json::from_str(&first).unwrap_or_default();
    if hello["method"] != "hello" || hello["params"]["token"].as_str() != Some(token) {
        let _ = send(&mut out, &hello["id"], Err("wrong or missing token: use ravune-mcp, which reads it from Ravune's data folder".into()));
        return;
    }
    if send(&mut out, &hello["id"], Ok(json!({ "name": "Ravune", "version": env!("CARGO_PKG_VERSION") }))).is_err() {
        return;
    }
    clients.fetch_add(1, Relaxed);
    ctx.request_repaint();
    for line in lines {
        let Ok(line) = line else { break };
        if stop.load(Relaxed) {
            break;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = send(&mut out, &Value::Null, Err(format!("not JSON: {e}")));
                continue;
            }
        };
        let (reply, answer) = channel();
        let call = Call { method: request["method"].as_str().unwrap_or("").to_string(), params: request["params"].clone(), reply };
        if calls.send(call).is_err() {
            break;
        }
        ctx.request_repaint();
        let result = answer.recv_timeout(Duration::from_secs(120)).unwrap_or_else(|_| Err("Ravune didn't answer in time".into()));
        if send(&mut out, &request["id"], result).is_err() {
            break;
        }
    }
    clients.fetch_sub(1, Relaxed);
    ctx.request_repaint();
}

fn instrument_name(i: &Instrument) -> String {
    match i {
        Instrument::Sampler { sample } => format!("sampler: {sample}"),
        Instrument::Synth { .. } => "VA-1 synth".into(),
        Instrument::Multi { file } => format!("multi-sample: {file}"),
        Instrument::Plugin { plugin } => format!("plugin: {}", plugin.name),
    }
}

/// `ravune-mcp`, next to Ravune: the stdio bridge MCP clients start.
pub fn bridge() -> PathBuf {
    crate::extensions::sibling("mcp")
}

/// What adds Ravune to Claude Code in every folder: without `--scope user` it's there only in
/// the folder the command ran in.
pub fn claude_command() -> String {
    format!("claude mcp add --scope user ravune -- \"{}\"", bridge().display())
}

/// Ravune as an entry of the MCP servers that Claude Desktop, Cursor and others keep in a file.
pub fn mcp_json() -> String {
    serde_json::to_string_pretty(&json!({ "mcpServers": { "ravune": { "command": bridge().to_string_lossy() } } })).unwrap_or_default()
}

/// Bytes as base64: a picture for a model.
pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { ABC[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

impl App {
    /// Keeps the server in step with the preferences and answers what came in.
    pub fn poll_agent(&mut self, ctx: &egui::Context, now: f64) {
        let prefs = self.settings.agent.clone();
        let running = self.agent.server.as_ref().map(|s| s.port);
        if !prefs.enabled && running.is_some() || prefs.enabled && running.is_some_and(|p| p != prefs.port) {
            self.agent.server = None;
            self.end_agent_group();
        }
        if prefs.enabled && self.agent.server.is_none() {
            match TcpListener::bind(("127.0.0.1", prefs.port)) {
                Ok(listener) => {
                    let token = token();
                    let file = crate::files::data_dir().map(|d| d.join(CONNECTION_FILE));
                    if let Some(f) = &file {
                        let _ = std::fs::create_dir_all(f.parent().unwrap_or(f));
                        let _ = std::fs::write(f, json!({ "port": prefs.port, "token": token }).to_string());
                    }
                    let (tx, calls) = channel();
                    let (stop, clients) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
                    let (s, c, ctx) = (stop.clone(), clients.clone(), ctx.clone());
                    std::thread::Builder::new().name("agents".into()).spawn(move || listen(listener, token, tx, s, c, ctx)).expect("spawn the agents thread");
                    self.agent.server = Some(Server { port: prefs.port, calls, stop, clients, file });
                }
                Err(e) => {
                    self.settings.agent.enabled = false;
                    self.say(tr!("Agents can't listen on port {}: {}", prefs.port, e));
                }
            }
        }
        let Some(server) = &self.agent.server else { return };
        let clients = server.clients.load(Relaxed);
        let calls: Vec<Call> = server.calls.try_iter().collect();
        if clients == 0 && self.agent.clients > 0 {
            self.end_agent_group(); // it left: its edits are done
        }
        self.agent.clients = clients;
        for call in calls {
            let result = self.agent_call(&call.method, &call.params);
            let detail = match call.method.as_str() {
                "run_operator" => format!("run_operator {}", call.params["id"].as_str().unwrap_or("?")),
                m => m.to_string(),
            };
            let what = match &result {
                Ok(_) => detail,
                Err(e) => format!("{detail}: {e}"),
            };
            self.agent.log.push(Line { at: now, what, ok: result.is_ok() });
            if self.agent.log.len() > LOG_LINES {
                self.agent.log.remove(0);
            }
            let _ = call.reply.send(result);
        }
    }

    /// Closes the agent's undo group (the user edits, or the agent checkpoints or leaves).
    pub fn end_agent_group(&mut self) {
        if std::mem::take(&mut self.agent.group) {
            self.session.end_group();
        }
    }

    pub fn agents_connected(&self) -> Option<usize> {
        self.agent.server.as_ref().map(|s| s.clients.load(Relaxed))
    }

    fn agent_call(&mut self, method: &str, p: &Value) -> Result<Value, String> {
        let prefs = self.settings.agent.clone();
        let read_only = || tr!("This agent may only read: the user can allow edits in Preferences → Agents").to_string();
        match method {
            "search_operators" => {
                let query = p["query"].as_str().unwrap_or("").to_lowercase();
                let words: Vec<&str> = query.split_whitespace().collect();
                let found: Vec<Value> = OPERATORS
                    .iter()
                    .filter(|o| words.iter().all(|w| o.id.contains(w) || o.label.to_lowercase().contains(w) || o.description.to_lowercase().contains(w)))
                    .map(|o| json!({ "id": o.id, "label": o.label, "description": o.description, "example": serde_json::from_str::<Value>(o.example).unwrap_or(json!({})), "undoable": o.undo }))
                    .collect();
                Ok(json!({ "operators": found, "count": found.len() }))
            }
            "run_operator" => {
                if !prefs.edit {
                    return Err(read_only());
                }
                let id = p["id"].as_str().ok_or("run_operator needs the operator's id")?;
                let props = p.get("props").filter(|v| !v.is_null()).cloned().unwrap_or(json!({}));
                if !self.session.in_group() {
                    self.session.begin_group("Agent edits");
                }
                self.agent.group = true;
                self.session.run(id, props)?;
                Ok(json!({ "ok": true, "undo": "one Ctrl+Z takes back everything changed since the last checkpoint" }))
            }
            "checkpoint" => {
                self.end_agent_group();
                Ok(json!({ "ok": true }))
            }
            "undo" => {
                if !prefs.edit {
                    return Err(read_only());
                }
                self.agent.group = false;
                self.session.run("edit.undo", json!({}))?;
                Ok(json!({ "ok": true, "next_undo": self.session.undo_label() }))
            }
            "query_project" => Ok(self.query_project(p)),
            "browse_sounds" => {
                let query = p["query"].as_str().unwrap_or("").to_lowercase();
                let limit = p["limit"].as_u64().unwrap_or(30).min(200) as usize;
                let hits: Vec<Value> = self.find_sounds(&query).into_iter().take(limit).map(|path| json!({ "name": ravune_core::sample_name(&path), "path": path })).collect();
                Ok(json!({ "sounds": hits, "use": "pass a path as `sample` to channel.add or channel.set, or to clip.add for the playlist" }))
            }
            "render_and_analyze" => self.render_and_analyze(p),
            "save_project" => {
                if !prefs.files {
                    return Err(tr!("This agent may not save: the user can allow it in Preferences → Agents").into());
                }
                if self.file.is_none() {
                    return Err(tr!("The project has no file yet: the user saves it once (Ctrl+S) first").into());
                }
                if self.save() { Ok(json!({ "ok": true })) } else { Err("saving failed".into()) }
            }
            _ => Err(format!("unknown method '{method}'")),
        }
    }

    /// The project as an agent wants it: a summary, one pattern's notes, the clips, or all of it.
    pub fn query_project(&self, p: &Value) -> Value {
        let s = &self.session;
        let project = &s.project;
        match p["what"].as_str().unwrap_or("summary") {
            "full" => serde_json::to_value(project).unwrap_or_default(),
            "pattern" => {
                let i = p["index"].as_u64().map_or(s.pattern, |i| i as usize).min(project.patterns.len().saturating_sub(1));
                let Some(pattern) = project.patterns.get(i) else { return json!({ "error": "no patterns" }) };
                let notes: serde_json::Map<String, Value> = pattern
                    .notes
                    .iter()
                    .map(|(slot, list)| {
                        let name = project.channels.iter().find(|c| c.slot == *slot).map_or("?", |c| c.name.as_str());
                        (format!("{slot} {name}"), json!(list.iter().map(|n| json!({ "start": n.start, "length": n.length, "key": n.key, "velocity": n.velocity })).collect::<Vec<_>>()))
                    })
                    .collect();
                json!({ "index": i, "name": pattern.name, "steps": pattern.length, "ticks_per_step": STEP, "ticks_per_beat": PPQ, "notes_by_channel_slot": notes, "automation": pattern.automation.len() })
            }
            "clips" => json!(project.clips.iter().enumerate().map(|(i, c)| {
                let what = match &c.kind {
                    ClipKind::Pattern { pattern } => format!("pattern {pattern} {}", project.patterns.get(*pattern).map_or("?", |p| p.name.as_str())),
                    ClipKind::Audio { sample } => format!("audio {sample}"),
                    ClipKind::Automation { target, .. } => format!("automation {}", crate::mixer::target_name(project, *target)),
                };
                json!({ "index": i, "lane": c.lane, "start_bar": c.start as f64 / BAR as f64, "bars": c.length as f64 / BAR as f64, "what": what, "muted": c.fx.mute })
            }).collect::<Vec<_>>()),
            _ => json!({
                "tempo": project.tempo,
                "swing": project.swing,
                "key": format!("{} {:?}", ravune_engine::KEYS[project.root as usize % 12], project.scale),
                "playing": s.playing,
                "song_mode": s.song_mode,
                "position_bar": s.position as f64 / BAR as f64,
                "song_bars": project.song_end() as f64 / BAR as f64,
                "selected_channel": s.selected,
                "current_pattern": s.pattern,
                "ticks": { "per_beat": PPQ, "per_step": STEP, "per_bar": BAR },
                "channels": project.channels.iter().enumerate().map(|(i, c)| json!({ "index": i, "slot": c.slot, "name": c.name, "instrument": instrument_name(&c.instrument), "mixer_track": c.track, "gain": c.gain, "mute": c.mute })).collect::<Vec<_>>(),
                "patterns": project.patterns.iter().enumerate().map(|(i, p)| json!({ "index": i, "name": p.name, "steps": p.length, "notes": p.notes.values().map(Vec::len).sum::<usize>() })).collect::<Vec<_>>(),
                "clips": project.clips.len(),
                "mixer": project.mixer.iter().enumerate().map(|(i, t)| json!({ "index": i, "name": t.name, "gain": t.gain, "pan": t.pan, "eq": t.eq, "mute": t.mute, "route": t.route, "effects": t.devices.iter().map(|d| d.plugin.as_ref().map_or(d.kind.name(), |p| p.name.as_str())).collect::<Vec<_>>(), "sends": t.sends.iter().map(|s| json!({ "to": s.to, "amount": s.amount })).collect::<Vec<_>>() })).collect::<Vec<_>>(),
                "macros": project.macros.iter().map(|m| json!({ "name": m.name, "value": m.value })).collect::<Vec<_>>(),
                "lfos": project.lfos.len(),
                "undo": s.undo_label(),
            }),
        }
    }

    /// Renders a few bars offline and says what they sound like.
    fn render_and_analyze(&mut self, p: &Value) -> Result<Value, String> {
        let project = &self.session.project;
        let song_mode = match p["mode"].as_str() {
            Some("pattern") => false,
            Some("song") => true,
            _ => !project.clips.is_empty(),
        };
        let bars = p["bars"].as_f64().unwrap_or(4.0).clamp(0.25, 32.0);
        let from = p["from_bar"].as_f64().unwrap_or(0.0).max(0.0);
        let mode = if song_mode { Mode::Song } else { Mode::Pattern(self.session.pattern) };
        let transport = Transport { playing: true, mode, start: (from * BAR as f64) as u32, jump: 0, looped: None };
        let seconds = (bars * 4.0 * 60.0 / project.tempo as f64) as f32;
        let library = &mut self.library;
        let song = Song::compile(project, transport, |path| library.get(path).ok());
        let started = Instant::now();
        let frames = render_span(song, 22_050.0, seconds);
        let a = analyze(&frames, 22_050.0);
        let beats_per_second = project.tempo as f64 / 60.0;
        let bands: serde_json::Map<String, Value> = BANDS.iter().zip(a.bands).map(|((name, ..), db)| (name.to_string(), json!((db * 10.0).round() / 10.0))).collect();
        Ok(json!({
            "rendered": format!("{} bars of the {} from bar {}", bars, if song_mode { "song" } else { "current pattern" }, from),
            "seconds": a.seconds,
            "peak_db": (a.peak_db * 10.0).round() / 10.0,
            "rms_db": (a.rms_db * 10.0).round() / 10.0,
            "band_share_db": bands,
            "stereo_width_db": (a.width_db * 10.0).round() / 10.0,
            "onsets": a.onsets.len(),
            "onset_beats": a.onsets.iter().take(32).map(|t| ((*t as f64 * beats_per_second) * 100.0).round() / 100.0).collect::<Vec<_>>(),
            "key": a.key_name(),
            "key_fit": (a.key.2 * 100.0).round() / 100.0,
            "bpm": a.bpm,
            "silent": a.peak_db < -60.0,
            "render_ms": started.elapsed().as_millis() as u64,
        }))
    }

    /// Preferences → Agents.
    pub fn prefs_agents(&mut self, ui: &mut Ui, t: &Theme) {
        kit::section(ui, t, tr!("AGENTS (MCP)"), true, |ui| {
            let a = &mut self.settings.agent;
            ui.checkbox(&mut a.enabled, tr!("Let agents use Ravune (Claude and other MCP clients)"));
            ui.label(RichText::new(tr!("They can search and run any command, read the project, find sounds and listen to a render. Everything an agent changes undoes in one step.")).small().color(t.fg_muted));
            ui.add_enabled_ui(a.enabled, |ui| {
                ui.checkbox(&mut a.edit, tr!("May change the project"));
                ui.checkbox(&mut a.files, tr!("May save the project"));
                ui.horizontal(|ui| {
                    ui.label(tr!("Port"));
                    ui.add(egui::DragValue::new(&mut a.port).range(1024..=65535));
                });
            });
            let status = match self.agents_connected() {
                Some(0) => tr!("Listening on 127.0.0.1:{} · no agent yet", self.settings.agent.port),
                Some(n) => tr!("Listening on 127.0.0.1:{} · {} connected", self.settings.agent.port, n),
                None => tr!("Off").to_string(),
            };
            ui.label(RichText::new(format!("{}  {status}", icons::ROBOT)).color(if self.agents_connected().is_some() { t.select } else { t.fg_muted }));
        });
        kit::section(ui, t, tr!("SET UP"), false, |ui| {
            let command = claude_command();
            ui.label(RichText::new(tr!("In Claude Code (or any MCP client), add the bridge next to Ravune:")).small().color(t.fg_muted));
            // the button first (right to left), so the command shortens into what's left and
            // never pushes the window wider
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if kit::icon_button(ui, t, icons::COPY, tr!("Copy"), false).clicked() {
                        ui.ctx().copy_text(command.clone());
                        self.say(tr!("Copied"));
                    }
                    ui.add(egui::Label::new(RichText::new(&command).font(Theme::mono(10.0))).truncate());
                });
            });
        });
        kit::section(ui, t, tr!("LOG"), false, |ui| {
            if self.agent.log.is_empty() {
                ui.label(RichText::new(tr!("Every call an agent makes shows up here.")).small().color(t.fg_muted));
                return;
            }
            // the log keeps its own little scroll, stuck to the newest line
            egui::ScrollArea::vertical().max_height(160.0).stick_to_bottom(true).show(ui, |ui| {
                for line in &self.agent.log {
                    let color = if line.ok { t.fg_text } else { t.meter_hot };
                    let at = format!("{:02}:{:02}", (line.at / 60.0) as u32, line.at as u32 % 60);
                    ui.add(egui::Label::new(RichText::new(format!("{at}  {}", line.what)).font(Theme::mono(10.0)).color(color)).truncate());
                }
            });
            if kit::button(ui, t, Some(icons::TRASH), tr!("Clear the log"), false).clicked() {
                self.agent.log.clear();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
    }
}
