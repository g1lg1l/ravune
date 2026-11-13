//! Keymaps: every command a key can run, three presets, and the user's changes on top.
//! `app.*` and `editor.*` / `window.*` ids are handled by the app; anything else is an
//! operator id. Space (tap = play, hold = pan), the note keys and Tab/arrows stay fixed.

use std::collections::BTreeMap;

use eframe::egui::{Key, KeyboardShortcut, ModifierNames, Modifiers};
use serde::{Deserialize, Serialize};

pub struct Command {
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
}

const fn cmd(group: &'static str, id: &'static str, label: &'static str) -> Command {
    Command { id, label, group }
}

pub const COMMANDS: [Command; 41] = [
    cmd("Transport", "transport.play_toggle", "Play / stop (besides Space)"),
    cmd("Transport", "transport.mode_toggle", "Pattern / song mode"),
    cmd("Transport", "transport.metronome_toggle", "Metronome"),
    cmd("Transport", "transport.rewind", "Back to the song start"),
    cmd("Transport", "app.record", "Record what plays"),
    cmd("Transport", "app.tap_tempo", "Tap tempo"),
    cmd("Transport", "app.retry_take", "Discard the take and record again"),
    cmd("Project", "app.start", "Start page: recent songs and templates"),
    cmd("Project", "app.new", "New project"),
    cmd("Project", "app.open", "Open"),
    cmd("Project", "app.save", "Save"),
    cmd("Project", "app.save_as", "Save as"),
    cmd("Project", "app.versions", "Versions of this song"),
    cmd("Project", "app.save_template", "Save as template"),
    cmd("Project", "app.export", "Export audio (last format)"),
    cmd("Project", "app.credits", "Export credits"),
    cmd("Project", "pattern.add", "New pattern"),
    cmd("Edit", "edit.undo", "Undo"),
    cmd("Edit", "edit.redo", "Redo"),
    cmd("Editors", "editor.rack", "Channel Rack"),
    cmd("Editors", "editor.piano", "Piano Roll"),
    cmd("Editors", "editor.device", "Channel settings"),
    cmd("Editors", "editor.mixer", "Mixer"),
    cmd("Editors", "editor.modulation", "Modulation"),
    cmd("Editors", "editor.next", "Next editor"),
    cmd("Editors", "editor.prev", "Previous editor"),
    cmd("Windows", "window.rack", "Channel Rack in a window"),
    cmd("Windows", "window.piano", "Piano Roll in a window"),
    cmd("Windows", "window.mixer", "Mixer in a window"),
    cmd("Windows", "window.modulation", "Modulation in a window"),
    cmd("Layout", "app.browser", "Fold / open the browser"),
    cmd("Layout", "app.editors", "Fold / open the editors"),
    cmd("Layout", "app.keys", "Show / hide the keys"),
    cmd("Layout", "app.visualizer", "Show / hide the visualizer"),
    cmd("Layout", "app.chat", "AI chat"),
    cmd("Layout", "workspace.next", "Next workspace"),
    cmd("Layout", "workspace.prev", "Previous workspace"),
    cmd("Find", "app.palette", "Command palette"),
    cmd("Find", "app.search", "Search the browser"),
    cmd("Find", "app.preferences", "Preferences"),
    cmd("Find", "app.specimen", "Specimen board: every widget in its states"),
];

/// Keys that aren't commands. Shown on the F1 sheet after the commands.
pub const FIXED: [(&str, &[(&str, &str)]); 5] = [
    ("Always", &[("Space", "Play / stop · hold in the playlist to pan"), ("F1", "This sheet"), ("Esc", "Close a dialog"), ("Ctrl ,", "Preferences → Keys: presets, rebinding")]),
    ("Keyboard control", &[("Tab", "Next control (Shift: previous)"), ("Arrows", "Move between controls"), ("Enter", "Press / toggle"), ("Up Down", "Turn a focused knob (Shift: fine)"), ("Delete", "Reset a focused knob"), ("Alt click / Enter", "Type a knob's value")]),
    ("Play notes", &[("A W S E D F T G", "Notes, Ableton layout"), ("Z X", "Octave down / up"), ("C V", "Velocity down / up")]),
    ("Piano roll", &[("Arrows", "Move the selected notes (Shift: an octave)"), ("Shift ← →", "Shorter / longer"), ("Ctrl C X V", "Copy, cut, paste where the pointer is"), ("Ctrl D", "Duplicate the selected notes")]),
    ("Playlist", &[("Space + drag", "Pan around"), ("Ctrl wheel", "Zoom"), ("Delete", "Delete the selected clip"), ("Ctrl D", "Duplicate the selected clip")]),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preset {
    #[default]
    #[serde(alias = "Phasor")]
    Ravune,
    FlStudio,
    Ableton,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Ravune, Preset::FlStudio, Preset::Ableton];

    pub fn name(self) -> &'static str {
        match self {
            Preset::Ravune => "Ravune",
            Preset::FlStudio => "FL Studio",
            Preset::Ableton => "Ableton",
        }
    }

    /// The preset's keys for a command.
    pub fn keys(self, id: &str) -> Vec<KeyboardShortcut> {
        const C: Modifiers = Modifiers::COMMAND;
        const S: Modifiers = Modifiers::SHIFT;
        const CS: Modifiers = Modifiers { shift: true, ..Modifiers::COMMAND };
        const CA: Modifiers = Modifiers { alt: true, ..Modifiers::COMMAND };
        const N: Modifiers = Modifiers::NONE;
        let k = |m: Modifiers, key: Key| KeyboardShortcut::new(m, key);
        let own = match (self, id) {
            (Preset::Ravune, "transport.mode_toggle") => vec![k(C, Key::L)],
            (Preset::Ravune, "transport.metronome_toggle") => vec![k(C, Key::M)],
            (Preset::Ravune, "pattern.add") => vec![k(N, Key::F4)],
            (Preset::Ravune, "app.export") => vec![k(C, Key::E)],
            (Preset::Ravune, "app.record") => vec![k(C, Key::R)],
            (Preset::Ravune, "app.retry_take") => vec![k(C, Key::Period)],
            (Preset::Ravune, "app.preferences") => vec![k(C, Key::Comma)],
            (Preset::Ravune, "app.browser") => vec![k(C, Key::B)],
            (Preset::Ravune, "app.editors") => vec![k(C, Key::J)],
            (Preset::Ravune, "app.keys") => vec![k(CS, Key::K)],
            // FL: F4 next empty pattern, Ctrl+R render, Alt+F8 browser, F10 settings,
            // Ctrl+T typing keyboard, Ctrl+Alt+Z undo history
            (Preset::FlStudio, "transport.mode_toggle") => vec![k(C, Key::L)],
            (Preset::FlStudio, "transport.metronome_toggle") => vec![k(C, Key::M)],
            (Preset::FlStudio, "pattern.add") => vec![k(N, Key::F4)],
            (Preset::FlStudio, "app.export") => vec![k(C, Key::R)],
            (Preset::FlStudio, "app.preferences") => vec![k(N, Key::F10)],
            (Preset::FlStudio, "editor.modulation") => vec![k(N, Key::F11)],
            (Preset::FlStudio, "window.modulation") => vec![k(S, Key::F11)],
            (Preset::FlStudio, "app.browser") => vec![k(Modifiers::ALT, Key::F8)],
            (Preset::FlStudio, "app.editors") => vec![k(C, Key::J)],
            (Preset::FlStudio, "app.keys") => vec![k(C, Key::T)],
            (Preset::FlStudio, "edit.undo") => vec![k(C, Key::Z), k(CA, Key::Z)],
            // Ableton: Ctrl+Alt+B browser, Ctrl+Alt+L detail view, Ctrl+Alt+M mixer,
            // Ctrl+Shift+R export, Ctrl+Shift+M new MIDI clip, M computer MIDI keyboard
            (Preset::Ableton, "transport.mode_toggle") => vec![k(C, Key::L)],
            (Preset::Ableton, "pattern.add") => vec![k(CS, Key::M)],
            (Preset::Ableton, "app.export") => vec![k(CS, Key::R)],
            (Preset::Ableton, "app.preferences") => vec![k(C, Key::Comma)],
            (Preset::Ableton, "app.browser") => vec![k(CA, Key::B)],
            (Preset::Ableton, "app.editors") => vec![k(CA, Key::L)],
            (Preset::Ableton, "editor.mixer") => vec![k(CA, Key::M), k(N, Key::F9)],
            (Preset::Ableton, "app.keys") => vec![k(N, Key::M)],
            _ => Vec::new(),
        };
        if !own.is_empty() {
            return own;
        }
        // what all three share
        match id {
            "app.new" => vec![k(C, Key::N)],
            "app.open" => vec![k(C, Key::O)],
            "app.save" => vec![k(C, Key::S)],
            "app.save_as" => vec![k(CS, Key::S)],
            "app.palette" => vec![k(C, Key::K), k(N, Key::F3)],
            "app.search" => vec![k(C, Key::F)],
            "app.chat" => vec![k(C, Key::I)],
            "transport.rewind" => vec![k(N, Key::Home)],
            "editor.next" => vec![k(C, Key::Tab)],
            "workspace.next" => vec![k(C, Key::PageDown)],
            "workspace.prev" => vec![k(C, Key::PageUp)],
            "editor.prev" => vec![k(CS, Key::Tab)],
            "editor.rack" => vec![k(N, Key::F6)],
            "editor.piano" => vec![k(N, Key::F7)],
            "editor.device" => vec![k(N, Key::F8)],
            "editor.mixer" => vec![k(N, Key::F9)],
            "editor.modulation" => vec![k(N, Key::F10)],
            "window.rack" => vec![k(S, Key::F6)],
            "window.piano" => vec![k(S, Key::F7)],
            "window.mixer" => vec![k(S, Key::F9)],
            "window.modulation" => vec![k(S, Key::F10)],
            "edit.undo" => vec![k(C, Key::Z)],
            "edit.redo" => vec![k(CS, Key::Z), k(C, Key::Y)],
            _ => Vec::new(),
        }
    }
}

/// A preset plus the commands the user rebound (so new commands still get defaults).
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keymap {
    pub preset: Preset,
    pub changed: BTreeMap<String, Vec<KeyboardShortcut>>,
}

impl Keymap {
    pub fn keys(&self, id: &str) -> Vec<KeyboardShortcut> {
        self.changed.get(id).cloned().unwrap_or_else(|| self.preset.keys(id))
    }

    pub fn bindings(&self) -> Vec<(&'static str, KeyboardShortcut)> {
        COMMANDS.iter().flat_map(|c| self.keys(c.id).into_iter().map(move |k| (c.id, k))).collect()
    }

    /// Binds `keys` to `id` alone (taking it from any other command); returns who lost it.
    pub fn bind(&mut self, id: &str, keys: KeyboardShortcut) -> Option<&'static str> {
        let mut taken = None;
        for c in COMMANDS.iter().filter(|c| c.id != id) {
            let mut list = self.keys(c.id);
            if list.contains(&keys) {
                list.retain(|k| *k != keys);
                self.changed.insert(c.id.into(), list);
                taken = Some(c.label);
            }
        }
        self.changed.insert(id.into(), vec![keys]);
        taken
    }

    pub fn clear(&mut self, id: &str) {
        self.changed.insert(id.into(), Vec::new());
    }

    /// "Ctrl K · F3", or "" if unbound.
    pub fn label(&self, id: &str) -> String {
        self.keys(id).iter().map(text).collect::<Vec<_>>().join(" · ")
    }
}

/// "Ctrl Shift S": modifier and key names, space-separated (how the F1 sheet draws keys).
pub fn text(k: &KeyboardShortcut) -> String {
    let names = ModifierNames { concat: " ", ..ModifierNames::NAMES };
    let mods = names.format(&k.modifiers, cfg!(target_os = "macos"));
    // "," rather than "Comma", but names where egui's symbol isn't in our font subset (arrows)
    let symbol = k.logical_key.symbol_or_name();
    let key = if symbol.is_ascii() { symbol } else { k.logical_key.name() };
    if mods.is_empty() { key.to_string() } else { format!("{mods} {key}") }
}

/// The key a chord is matched on. Ctrl+Alt is AltGr on European layouts, which turns the
/// key into a character, so those chords go by where the key is.
pub fn chord_key(key: Key, physical: Option<Key>, mods: Modifiers) -> Key {
    if mods.ctrl && mods.alt { physical.unwrap_or(key) } else { key }
}

/// Ctrl, Shift, Alt and Super on their own (egui reports them as keys too).
pub fn is_modifier(key: Key) -> bool {
    use Key::*;
    matches!(key, ShiftLeft | ShiftRight | ControlLeft | ControlRight | AltLeft | AltRight | SuperLeft | SuperRight)
}

/// Keys that play notes or move focus can't be bound without a modifier.
pub fn reserved(k: &KeyboardShortcut) -> bool {
    use Key::*;
    k.modifiers.is_none()
        && (crate::PIANO_KEYS.iter().any(|(key, ..)| *key == k.logical_key)
            || matches!(k.logical_key, Z | X | C | V | Space | Tab | Escape | Enter | ArrowUp | ArrowDown | ArrowLeft | ArrowRight | Delete | Backspace | F1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_have_no_clashes_and_rebinding_moves_keys() {
        for preset in Preset::ALL {
            let map = Keymap { preset, ..Default::default() };
            let all = map.bindings();
            for (i, (id, k)) in all.iter().enumerate() {
                assert!(!reserved(k), "{} binds a reserved key to {id}", preset.name());
                assert!(all[i + 1..].iter().all(|(_, other)| other != k), "{}: {} used twice", preset.name(), text(k));
            }
            assert!(COMMANDS.iter().filter(|c| !map.keys(c.id).is_empty()).count() >= 25, "{} leaves too much unbound", preset.name());
        }
        let mut map = Keymap::default();
        let ctrl_k = KeyboardShortcut::new(Modifiers::COMMAND, Key::K);
        assert_eq!(map.bind("app.save", ctrl_k), Some("Command palette"));
        assert_eq!(map.keys("app.palette"), vec![KeyboardShortcut::new(Modifiers::NONE, Key::F3)]);
        assert_eq!(map.label("app.save"), if cfg!(target_os = "macos") { "Cmd K" } else { "Ctrl K" });
        map.clear("app.save");
        assert!(map.keys("app.save").is_empty());
    }
}
