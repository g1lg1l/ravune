//! Scripts (M8): Luau that drives Ravune through the same operators as everything else.
//! Built-in ones ship with the app; the user's own live in Documents/Ravune/Scripts (`.luau`
//! or `.lua`, a first `-- ` comment line says what it does). A run is one undo step; scripts
//! are sandboxed (no files, no OS) and stopped after two seconds.
//!
//! The API, a global `ravune` table:
//! - `ravune.run(id, props)` runs an operator (errors stop the script)
//! - `ravune.notes([channel])` the selected (or given) channel's notes in the current pattern:
//!   `{ index, start, length, key, velocity }`, ticks
//! - `ravune.pattern()` `{ index, name, steps }` · `ravune.selected()` `{ channel, slot, pattern }`
//! - `ravune.scale()` `{ root (0 = C), name, keys }` (the scale's pitch classes from the root)
//! - `ravune.project()` the summary agents get · `ravune.print(text)` · `ravune.args`
//! - `ravune.STEP`, `ravune.BEAT`, `ravune.BAR` in ticks

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mlua::{Lua, LuaSerdeExt, Value as LuaValue, VmState};
use ravune_core::{BAR, PPQ, STEP};
use serde_json::{Value, json};

use crate::App;
use crate::shell::Tone;

/// Built-in scripts: name, what it does, source, arguments.
const BUILTIN: [(&str, &str, &str, &str); 9] = [
    ("Euclid 3 over 8", "Three hits spread evenly over 8 steps, on the selected channel", include_str!("../assets/scripts/euclid.luau"), r#"{"hits": 3, "steps": 8}"#),
    ("Euclid 5 over 16", "Five hits spread evenly over 16 steps, on the selected channel", include_str!("../assets/scripts/euclid.luau"), r#"{"hits": 5, "steps": 16}"#),
    ("Euclid 7 over 16", "Seven hits spread evenly over 16 steps, on the selected channel", include_str!("../assets/scripts/euclid.luau"), r#"{"hits": 7, "steps": 16}"#),
    ("Arpeggiate chords", "Each chord on the selected channel becomes a rising arpeggio of 16ths", include_str!("../assets/scripts/arpeggiate.luau"), r#"{}"#),
    ("Arpeggiate up and down", "Each chord becomes an arpeggio going up and back down", include_str!("../assets/scripts/arpeggiate.luau"), r#"{"direction": "updown"}"#),
    ("Random bassline in key", "A new bassline on the selected channel, in the song's scale, off the beat", include_str!("../assets/scripts/bassline.luau"), r#"{}"#),
    ("Reverse notes", "The selected channel's notes back to front", include_str!("../assets/scripts/reverse.luau"), r#"{}"#),
    ("Half time", "Every note twice as long and twice as far apart", include_str!("../assets/scripts/stretch.luau"), r#"{"factor": 2}"#),
    ("Double time", "Every note half as long and twice as close", include_str!("../assets/scripts/stretch.luau"), r#"{"factor": 0.5}"#),
];

#[derive(Clone)]
pub struct Script {
    pub name: String,
    pub about: String,
    source: Source,
}

#[derive(Clone)]
enum Source {
    Builtin(&'static str, &'static str),
    File(PathBuf),
}

/// Documents/Ravune/Scripts.
pub fn scripts_dir() -> PathBuf {
    crate::browser::library_dir().with_file_name("Scripts")
}

/// The built-in scripts, then the user's (A–Z), then the ones extensions bring.
pub fn list() -> Vec<Script> {
    let mut all: Vec<Script> = BUILTIN.iter().map(|(name, about, source, args)| Script { name: crate::i18n::t(name).into(), about: crate::i18n::t(about).into(), source: Source::Builtin(source, args) }).collect();
    let mut own: Vec<PathBuf> = std::fs::read_dir(scripts_dir()).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "luau" || e == "lua")).collect();
    own.sort();
    own.extend(crate::extensions::files("scripts"));
    all.extend(own.into_iter().map(Script::from_file));
    all
}

impl Script {
    /// A script file: named after the file, about what its first `--` line says.
    pub fn from_file(path: PathBuf) -> Script {
        let about = std::fs::read_to_string(&path).ok().and_then(|t| t.lines().next().and_then(|l| l.strip_prefix("--")).map(|l| l.trim().to_string())).unwrap_or_default();
        let name = path.file_stem().map_or(String::new(), |s| s.to_string_lossy().into_owned());
        Script { name, about, source: Source::File(path) }
    }
}

/// Lua has one number type: whole numbers go back as integers, so `start = 96` fits a u32.
fn integers(v: Value) -> Value {
    match v {
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 9e15 => json!(f as i64),
            _ => Value::Number(n),
        },
        Value::Array(a) if a.is_empty() => json!({}), // `{}` from Lua: no props
        Value::Array(a) => Value::Array(a.into_iter().map(integers).collect()),
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, integers(v))).collect()),
        other => other,
    }
}

impl App {
    /// Runs a script as one undo step; says how it went.
    pub fn run_script(&mut self, script: &Script) {
        match self.script_result(script) {
            Ok(printed) if printed.is_empty() => self.say_as(Tone::Done, tr!("Ran {}", script.name)),
            Ok(printed) => self.say(format!("{}: {}", script.name, printed.join(" · "))),
            Err(e) => self.say_as(Tone::Error, tr!("{} stopped: {}", script.name, e)),
        }
    }

    fn script_result(&mut self, script: &Script) -> Result<Vec<String>, String> {
        let (source, args) = match &script.source {
            Source::Builtin(source, args) => (source.to_string(), serde_json::from_str::<Value>(args).unwrap_or(json!({}))),
            Source::File(path) => (std::fs::read_to_string(path).map_err(|e| e.to_string())?, json!({})),
        };
        self.end_agent_group();
        self.session.begin_group("Script");
        let printed = RefCell::new(Vec::new());
        let result = run_lua(self, &source, &script.name, args, &printed);
        if result.is_err() {
            // a script that failed halfway leaves nothing behind
            self.session.end_group();
            if self.session.undo_label() == Some("Script") {
                let _ = self.session.run("edit.undo", json!({}));
            }
        } else {
            self.session.end_group();
        }
        result.map(|()| printed.into_inner())
    }
}

fn run_lua(app: &mut App, source: &str, name: &str, args: Value, printed: &RefCell<Vec<String>>) -> Result<(), String> {
    let lua = Lua::new();
    let started = Instant::now();
    lua.set_interrupt(move |_| if started.elapsed() > Duration::from_secs(2) { Err(mlua::Error::runtime("it ran longer than 2 seconds")) } else { Ok(VmState::Continue) });
    let app = RefCell::new(app);
    lua.scope(|scope| {
        let api = lua.create_table()?;
        api.set("STEP", STEP)?;
        api.set("BEAT", PPQ)?;
        api.set("BAR", BAR)?;
        api.set("args", lua.to_value(&args)?)?;
        api.set(
            "run",
            scope.create_function(|lua, (id, props): (String, Option<LuaValue>)| {
                let props: Value = props.map(|p| lua.from_value(p)).transpose()?.map_or(json!({}), integers);
                app.borrow_mut().session.run(&id, props).map_err(|e| mlua::Error::runtime(format!("{id}: {e}")))
            })?,
        )?;
        api.set(
            "notes",
            scope.create_function(|lua, channel: Option<usize>| {
                let app = app.borrow();
                let s = &app.session;
                let slot = s.project.channels.get(channel.unwrap_or(s.selected)).map(|c| c.slot);
                let notes = slot.and_then(|slot| s.current_pattern().notes.get(&slot)).cloned().unwrap_or_default();
                let list: Vec<Value> = notes.iter().enumerate().map(|(i, n)| json!({ "index": i, "start": n.start, "length": n.length, "key": n.key, "velocity": n.velocity })).collect();
                lua.to_value(&list)
            })?,
        )?;
        api.set(
            "pattern",
            scope.create_function(|lua, ()| {
                let app = app.borrow();
                let p = app.session.current_pattern();
                lua.to_value(&json!({ "index": app.session.pattern, "name": p.name, "steps": p.length }))
            })?,
        )?;
        api.set(
            "selected",
            scope.create_function(|lua, ()| {
                let app = app.borrow();
                let s = &app.session;
                lua.to_value(&json!({ "channel": s.selected, "slot": s.selected_channel().map(|c| c.slot), "pattern": s.pattern }))
            })?,
        )?;
        api.set(
            "scale",
            scope.create_function(|lua, ()| {
                let app = app.borrow();
                let p = &app.session.project;
                let keys: Vec<u8> = (0..12u8).filter(|k| p.scale.contains(0, *k)).collect();
                lua.to_value(&json!({ "root": p.root % 12, "name": format!("{:?}", p.scale), "keys": keys }))
            })?,
        )?;
        api.set("project", scope.create_function(|lua, ()| lua.to_value(&app.borrow().query_project(&json!({}))))?)?;
        api.set(
            "print",
            scope.create_function(|_, text: String| {
                printed.borrow_mut().push(text);
                Ok(())
            })?,
        )?;
        lua.globals().set("phasor", api.clone())?; // scripts written before the rename
        lua.globals().set("ravune", api)?;
        lua.sandbox(true)?;
        lua.load(source).set_name(name).exec()
    })
    .map_err(|e| e.to_string().lines().next().unwrap_or("error").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(name: &str) -> Script {
        list().into_iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no script {name}"))
    }

    #[test]
    fn built_in_scripts_edit_in_one_undo_step() {
        let mut app = App::for_tests();
        app.session.selected = 0; // the kick
        let before = app.session.project.clone();
        app.run_script(&script("Euclid 5 over 16"));
        let kick: Vec<u32> = app.session.current_pattern().notes[&0].iter().map(|n| n.start / STEP).collect();
        assert_eq!(kick, vec![0, 4, 7, 10, 13], "{:?}", app.toast);
        app.session.run("edit.undo", json!({})).unwrap();
        assert_eq!(app.session.project, before, "one undo");

        // a chord on the VA-1 becomes an arpeggio, every 16th over its length
        app.session.selected = 5;
        for key in [57, 60, 64] {
            app.session.run("note.add", json!({ "channel": 5, "key": key, "start": 0, "length": 96 })).unwrap();
        }
        app.run_script(&script("Arpeggiate chords"));
        let arp: Vec<(u32, u8)> = app.session.current_pattern().notes[&5].iter().map(|n| (n.start, n.key)).collect();
        assert_eq!(arp, vec![(0, 57), (24, 60), (48, 64), (72, 57)]);
    }

    #[test]
    fn a_failing_script_leaves_nothing_and_a_loop_is_stopped() {
        let mut app = App::for_tests();
        let before = app.session.project.clone();
        let bad = Script { name: "bad".into(), about: String::new(), source: Source::Builtin("ravune.run('transport.set_tempo', { bpm = 99 }); ravune.run('no.such', {})", "{}") };
        app.run_script(&bad);
        assert_eq!(app.session.project, before, "rolled back");
        assert!(app.toast.as_ref().is_some_and(|t| t.text.contains("no.such")));
        let forever = Script { name: "forever".into(), about: String::new(), source: Source::Builtin("while true do end", "{}") };
        let started = Instant::now();
        app.run_script(&forever);
        assert!(started.elapsed() < Duration::from_secs(5) && app.toast.as_ref().is_some_and(|t| t.text.contains("2 seconds")));
        let sandboxed = Script { name: "io".into(), about: String::new(), source: Source::Builtin("io.open('x', 'w')", "{}") };
        app.run_script(&sandboxed);
        assert!(app.toast.as_ref().is_some_and(|t| t.text.contains("stopped")), "no io library");
    }
}
