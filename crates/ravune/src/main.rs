//! Ravune: a modular DAW for deep sound design.
//! Every edit goes through `ravune_core` operators, so keymaps, the command palette,
//! undo and (later) scripts and agents share one path.

// release builds are a window, not a console program: no terminal opens beside Ravune on
// Windows (debug builds keep it for their logs; the helpers it starts ask for none)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
mod i18n;

mod agent;
mod ai;
mod browser;
mod chat;
mod clipprops;
mod dawproject;
mod export;
mod extensions;
mod files;
mod help;
mod icons;
mod keymap;
mod kit;
mod lessons;
mod markdown;
mod midi;
mod mixer;
mod online;
mod orb;
mod osc;
mod plugins;
mod pianoroll;
mod playlist;
mod record;
mod script;
mod rack;
mod shell;
mod specimen;
mod theme;
mod usage;
mod visualizer;
mod widgets;
mod workspace;

use std::path::PathBuf;
use std::time::Duration;
use crate::shell::Tone;

use eframe::egui::{self, Event, Frame, Key, KeyboardShortcut, Modifiers, Response};
use ravune_core::{MAX_TRACKS, Project, Session};
use ravune_engine::{Engine, MidiSink, Mode, Msg, Song, Transport};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use theme::Theme;

// Debug builds abort if the audio callback ever allocates.
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

/// The OS cursor in window points. winit doesn't move egui's pointer during a file drag,
/// so where a file was dropped has to be asked for.
#[cfg(windows)]
fn os_cursor(ctx: &egui::Context) -> Option<egui::Pos2> {
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetCursorPos(point: *mut Point) -> i32;
    }
    let mut p = Point { x: 0, y: 0 };
    // SAFETY: GetCursorPos only writes the POINT we hand it.
    if unsafe { GetCursorPos(&mut p) } == 0 {
        return None;
    }
    let (ppp, inner) = ctx.input(|i| (i.pixels_per_point, i.viewport().inner_rect));
    Some(egui::pos2(p.x as f32, p.y as f32) / ppp - inner?.min.to_vec2())
}

#[cfg(not(windows))]
fn os_cursor(ctx: &egui::Context) -> Option<egui::Pos2> {
    ctx.input(|i| i.pointer.latest_pos())
}

fn main() -> eframe::Result {
    files::install_crash_hook();
    files::move_phasor_folders();
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon.png")).expect("assets/icon.png is a PNG");
    let viewport = egui::ViewportBuilder::default().with_title("Ravune").with_icon(icon).with_inner_size([1280.0, 800.0]).with_min_inner_size([900.0, 600.0]);
    // RAVUNE_CAPTURE: a window for screenshots (tests, docs) that never gets in anyone's way:
    // off-screen, unfocused, off the taskbar, wherever the profile left it
    let capture = std::env::var_os("RAVUNE_CAPTURE").is_some();
    let viewport = if capture { viewport.with_inner_size([1440.0, 900.0]).with_position([-6000.0, 0.0]).with_active(false).with_taskbar(false) } else { viewport };
    let options = eframe::NativeOptions {
        viewport,
        persist_window: !capture,
        // a separate profile keeps its window and settings apart too
        persistence_path: std::env::var_os("RAVUNE_DATA").and_then(|_| files::data_dir()).map(|d| d.join("app.ron")),
        ..Default::default()
    };
    eframe::run_native("Ravune", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

/// Ableton-style computer keyboard: home row = white keys, row above = black keys.
const PIANO_KEYS: [(Key, u8, &str); 17] = [
    (Key::A, 0, "A"), (Key::W, 1, "W"), (Key::S, 2, "S"), (Key::E, 3, "E"), (Key::D, 4, "D"),
    (Key::F, 5, "F"), (Key::T, 6, "T"), (Key::G, 7, "G"), (Key::Y, 8, "Y"), (Key::H, 9, "H"),
    (Key::U, 10, "U"), (Key::J, 11, "J"), (Key::K, 12, "K"), (Key::O, 13, "O"), (Key::L, 14, "L"),
    (Key::P, 15, "P"), (Key::Semicolon, 16, ";"),
];

const BUFFER_SIZES: [Option<u32>; 6] = [None, Some(64), Some(128), Some(256), Some(512), Some(1024)];

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    device: Option<String>,
    buffer: Option<u32>,
    /// The app's output volume (0..2), after the mixer; the project doesn't change.
    volume: f32,
    octave: i8,
    velocity: f32,
    /// Sample folders shown in the browser.
    folders: Vec<String>,
    favorites: Vec<String>,
    wave: widgets::WaveStyle,
    show_keys: bool,
    /// The agent chat on the right.
    show_chat: bool,
    /// Browser open (else a thin rail of icons).
    show_browser: bool,
    /// Editor area open (else just its tab bar).
    show_editor: bool,
    reduce_motion: bool,
    /// The browser plays a sound (or a synth preset) as soon as it's clicked or arrowed onto.
    preview_on_select: bool,
    /// Interface scale.
    zoom: f32,
    /// Editors popped out into their own windows.
    floating: Vec<Tab>,
    /// The theme in use (a copy: editing it doesn't touch the saved ones).
    theme: Theme,
    /// Themes the user saved or imported.
    themes: Vec<Theme>,
    keymap: keymap::Keymap,
    /// Workspaces; the panel switches above are the live copy of the current one.
    workspaces: Vec<workspace::Workspace>,
    workspace: usize,
    online: online::Prefs,
    /// Projects opened or saved lately, the latest first.
    recent: Vec<String>,
    language: i18n::Lang,
    /// What File → Export (and its key) writes.
    export_format: export::Format,
    /// Agents (MCP): off until the user turns them on.
    agent: agent::Prefs,
    /// Who answers in the AI chat, with which model, how hard it thinks.
    ai: ai::Prefs,
    /// OSC in: off until the user turns it on.
    osc: osc::Prefs,
    /// Lessons finished (by title).
    lessons_done: Vec<String>,
    /// Ask GitHub for a newer release at start.
    check_updates: bool,
    /// The selected channel's settings on the right of the rack and the piano roll…
    inspector: bool,
    /// …or in a window of their own.
    inspector_window: bool,
    /// A sound's cut handles (and a clip's envelope points) land on 16ths.
    snap_cuts: bool,
    /// The mixer shows its ghost tracks (sounds' own tracks) after the others.
    show_ghosts: bool,
    /// The visualizer: shown, where, spectrum or scope.
    viz: visualizer::Prefs,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device: None,
            buffer: Some(256),
            volume: 1.0,
            octave: 4,
            velocity: 0.8,
            folders: Vec::new(),
            favorites: Vec::new(),
            wave: widgets::WaveStyle::default(),
            show_keys: false,
            show_chat: false,
            show_browser: true,
            show_editor: true,
            reduce_motion: false,
            preview_on_select: true,
            zoom: 1.0,
            floating: Vec::new(),
            theme: Theme::dark(),
            themes: Vec::new(),
            keymap: keymap::Keymap::default(),
            workspaces: workspace::Workspace::builtins(),
            workspace: 0,
            online: online::Prefs::default(),
            recent: Vec::new(),
            language: i18n::Lang::default(),
            export_format: export::Format::default(),
            agent: agent::Prefs::default(),
            ai: ai::Prefs::default(),
            osc: osc::Prefs::default(),
            lessons_done: Vec::new(),
            check_updates: true,
            inspector: true,
            inspector_window: false,
            snap_cuts: false,
            show_ghosts: false,
            viz: visualizer::Prefs::default(),
        }
    }
}

impl Settings {
    /// The theme as drawn: the chosen one, with motion off if asked.
    fn effective_theme(&self) -> Theme {
        let mut theme = self.theme.clone();
        if self.reduce_motion {
            theme.motion = 0.0;
        }
        theme
    }
}

#[derive(Default)]
struct Palette {
    query: String,
    selected: usize,
    /// The scripts, read once when it opens (not every frame).
    scripts: Vec<script::Script>,
}

/// Editors: docked in the bottom area or floating in their own windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tab {
    Rack,
    PianoRoll,
    Device,
    Mixer,
    Modulation,
}

struct App {
    session: Session,
    settings: Settings,
    theme: Theme,
    engine: Option<Engine>,
    engine_error: Option<String>,
    last_attempt: f64,
    /// What the audio thread has: project + transport, and the keyboard's target slot.
    sent: Option<(Project, Transport)>,
    /// The output volume the engine has.
    sent_volume: Option<f32>,
    sent_target: Option<Option<u8>>,
    /// Playhead in beats while playing (pattern or song, depending on the mode).
    beat: Option<f64>,
    tab: Tab,
    /// A second docked editor beside `tab`.
    split: Option<Tab>,
    /// A floating editor to bring to the front this frame.
    raise: Option<Tab>,
    /// Mixer track whose effects the Mixer tab shows.
    mixer_track: usize,
    /// The mixer's effects panel (right side) is open.
    mixer_fx: bool,
    /// A cable being pulled out of this mixer track's jack.
    mixer_cable: Option<usize>,
    /// Effect cards folded to their title bar (by device id).
    folded: std::collections::HashSet<u32>,
    /// Peak-hold levels for the mixer meters.
    holds: [f32; MAX_TRACKS],
    viz: visualizer::Visualizer,
    /// Click sparks still flying.
    sparks: Vec<kit::Spark>,
    /// The docked editor shown last frame, and when it changed (for the crossfade).
    last_tab: Tab,
    tab_since: f64,
    /// Per-track peak levels, decaying (meters).
    meters: [f32; MAX_TRACKS],
    playlist: playlist::PlaylistView,
    piano: pianoroll::PianoView,
    export: Option<export::Export>,
    /// The take being recorded (the record button).
    recording: Option<record::Recording>,
    /// Automation recording armed: knobs turned while the song plays write automation.
    auto_rec: bool,
    /// The knob being recorded, its clip and the last tick written (what the next move overwrites from).
    auto_last: Option<(ravune_core::Target, usize, u32)>,
    /// MCP clients talking to Ravune: the server, the log, the agent's undo group.
    agent: agent::Agent,
    /// The AI chat: the conversations and the reply on its way.
    chat: chat::Chat,
    /// OSC in, while it's on.
    osc: Option<osc::Osc>,
    /// The app's own CPU, memory and GPU, for the status bar.
    usage: usage::Usage,
    /// The specimen board, while it's open.
    specimen: Option<specimen::Specimen>,
    /// The extension gallery and the update check.
    ext: extensions::Online,
    midi: midi::Midi,
    midi_sink: MidiSink,
    library: browser::Library,
    browser: browser::Browser,
    online: online::Online,
    plugins: plugins::Plugins,
    /// Step painting gesture in the rack: (channel, turning steps on?).
    paint: Option<(usize, bool)>,
    keys_down: Vec<(Key, u8)>,
    /// Space was held to pan, so letting go of it mustn't toggle playback.
    space_panned: bool,
    /// Audio files dropped from the OS this frame, and where (window coordinates).
    dropped: Vec<PathBuf>,
    drop_at: Option<egui::Pos2>,
    mouse_note: Option<u8>,
    played: bool,
    level: f32,
    palette: Option<Palette>,
    show_settings: bool,
    prefs_tab: shell::PrefsTab,
    show_help: bool,
    show_manual: bool,
    manual_chapter: usize,
    show_about: bool,
    show_theme_editor: bool,
    /// The keymap command waiting for its new shortcut (Preferences → Keys).
    binding: Option<&'static str>,
    /// The parameter waiting for a controller to move (MIDI learn).
    learning: Option<ravune_core::Target>,
    rename: Option<shell::Renaming>,
    devices: Vec<String>,
    /// Info View: help for whatever the mouse is over (collected this frame, shown next).
    info: String,
    info_next: String,
    /// The help of the area the mouse is over (the playlist's tool, an editor's first steps),
    /// for the status bar when what it's over has none of its own.
    info_zone: String,
    info_zone_next: String,
    toast: Option<shell::Toast>,
    /// What a toast's button asked for, once it's clicked (its owner takes it).
    toast_acted: Option<&'static str>,
    // files
    file: Option<PathBuf>,
    saved: Project,
    autosaved: Project,
    last_autosave: f64,
    pending: Option<files::Pending>,
    recovery: Option<Project>,
    /// What went wrong last time Ravune crashed, to report.
    crash: Option<String>,
    /// The lesson running and its step.
    lesson: Option<(usize, usize)>,
    title: String,
}

impl App {
    /// Ravune at rest: settings and state, no fonts, engine or plugin scan (`new` starts
    /// those). Tests build this.
    fn idle(settings: Settings, recovery: Option<Project>) -> Self {
        let theme = settings.effective_theme();
        Self {
            session: Session::default(),
            settings,
            theme,
            engine: None,
            engine_error: None,
            last_attempt: 0.0,
            sent: None,
            sent_volume: None,
            sent_target: None,
            beat: None,
            tab: Tab::Rack,
            split: None,
            raise: None,
            mixer_track: 0,
            mixer_fx: true,
            mixer_cable: None,
            folded: Default::default(),
            holds: [0.0; MAX_TRACKS],
            viz: visualizer::Visualizer::default(),
            sparks: Vec::new(),
            last_tab: Tab::Rack,
            tab_since: 0.0,
            meters: [0.0; MAX_TRACKS],
            playlist: playlist::PlaylistView::default(),
            piano: pianoroll::PianoView::default(),
            export: None,
            recording: None,
            auto_rec: false,
            auto_last: None,
            agent: agent::Agent::default(),
            chat: chat::Chat::default(),
            osc: None,
            usage: usage::Usage::new(),
            specimen: specimen::Specimen::from_env(),
            ext: extensions::Online::default(),
            midi: midi::Midi::default(),
            midi_sink: MidiSink::default(),
            library: browser::Library::default(),
            browser: browser::Browser::default(),
            online: online::Online::default(),
            plugins: plugins::Plugins::default(),
            paint: None,
            keys_down: Vec::new(),
            space_panned: false,
            dropped: Vec::new(),
            drop_at: None,
            mouse_note: None,
            played: false,
            level: 0.0,
            palette: None,
            show_settings: false,
            prefs_tab: shell::PrefsTab::default(),
            show_help: false,
            show_manual: false,
            manual_chapter: 0,
            show_about: false,
            show_theme_editor: false,
            binding: None,
            learning: None,
            rename: None,
            devices: Vec::new(),
            info: String::new(),
            info_next: String::new(),
            info_zone: String::new(),
            info_zone_next: String::new(),
            toast: None,
            toast_acted: None,
            file: None,
            saved: Project::default(),
            autosaved: Project::default(),
            last_autosave: 0.0,
            pending: None,
            recovery,
            crash: None,
            lesson: None,
            title: String::new(),
        }
    }

    #[cfg(test)]
    fn for_tests() -> Self {
        Self::idle(Settings::default(), None)
    }

    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let settings: Settings = cc.storage.and_then(|s| eframe::get_value(s, "settings")).unwrap_or_default();
        i18n::set(settings.language);
        let mut app = Self::idle(settings, files::take_recovery());
        app.crash = files::take_crash();
        app.theme.apply(&cc.egui_ctx);
        if let Some(state) = &cc.wgpu_render_state {
            visualizer::install(state);
            app.viz.gpu = true;
        }
        cc.egui_ctx.set_zoom_factor(app.settings.zoom);
        app.load_workspace(app.settings.workspace);
        app.start_engine(0.0);
        app.start_scan(false);
        // RAVUNE_CAPTURE=extensions (or agents, look…): opens on that Preferences tab, for screenshots
        if let Some(tab) = std::env::var("RAVUNE_CAPTURE").ok().and_then(|v| shell::PrefsTab::named(&v)) {
            (app.show_settings, app.prefs_tab) = (true, tab);
        }
        // `ravune song.ravune` (also what double-clicking a project file does)
        if let Some(path) = std::env::args_os().nth(1) {
            app.request(files::Pending::Open(path.into()));
        }
        app
    }

    // ---- engine & operators -------------------------------------------------

    fn start_engine(&mut self, now: f64) {
        self.engine = None; // close the old stream first: some drivers allow only one
        *self.midi_sink.lock().unwrap() = None;
        self.last_attempt = now;
        self.sent = None;
        self.sent_volume = None;
        self.sent_target = None;
        match Engine::start(self.settings.device.as_deref(), self.settings.buffer, &self.midi_sink) {
            Ok(engine) => {
                self.engine = Some(engine);
                self.engine_error = None;
                self.detach_plugins(); // the new engine gets every plugin again
            }
            Err(e) => self.engine_error = Some(e),
        }
    }

    /// Rebuilds the stream if the device vanished; retries every 3 s while there is none.
    fn keep_engine_alive(&mut self, now: f64) {
        let dead = self.engine.as_ref().is_some_and(|e| e.stats().failed());
        if dead || (self.engine.is_none() && now - self.last_attempt > 3.0) {
            self.start_engine(now);
            if dead && self.engine.is_some() {
                self.say(tr!("Audio device changed: reconnected"));
            }
        }
    }

    /// Compiles and sends the project to the audio thread whenever it changed.
    fn sync_engine(&mut self) {
        if self.sent_volume != Some(self.settings.volume)
            && let Some(e) = &mut self.engine
        {
            e.send(Msg::Output(self.settings.volume));
            self.sent_volume = Some(self.settings.volume);
        }
        let Some(engine) = &mut self.engine else { return };
        let s = &self.session;
        let mode = if s.song_mode { Mode::Song } else { Mode::Pattern(s.pattern) };
        let transport = Transport { playing: s.playing, mode, start: s.position, jump: s.jumps, looped: s.loop_range };
        if self.sent.as_ref().is_none_or(|(p, t)| *p != s.project || *t != transport) {
            let library = &mut self.library;
            let song = Song::compile(&s.project, transport, |path| library.get(path).ok());
            engine.load(song);
            self.sent = Some((s.project.clone(), transport));
        }
        let target = self.session.selected_channel().map(|c| c.slot);
        if self.sent_target != Some(target) {
            engine.send(Msg::Target(target));
            self.sent_target = Some(target);
        }
    }

    fn run(&mut self, id: &str, props: Value) {
        self.user_edit(id);
        if let Err(e) = self.session.run(id, props) {
            self.say_as(Tone::Error, e);
        }
    }

    fn run_merge(&mut self, id: &str, props: Value, key: &str) {
        self.user_edit(id);
        if let Err(e) = self.session.run_merge(id, props, key) {
            self.say_as(Tone::Error, e);
        }
    }

    /// Runs an operator into the last undo step (see `Session::run_joined`).
    fn run_joined(&mut self, id: &str, props: Value) {
        if let Err(e) = self.session.run_joined(id, props) {
            self.say_as(Tone::Error, e);
        }
    }

    /// The user changes something: an agent's edits so far stay one undo step of their own.
    fn user_edit(&mut self, id: &str) {
        if ravune_core::find(id).is_some_and(|o| o.undo) {
            self.end_agent_group();
        }
    }

    /// A toast, to read and go on. Operators' errors arrive in English: they're shown in the
    /// interface language when the catalog has them.
    fn say(&mut self, msg: impl Into<String>) {
        self.say_as(shell::Tone::Info, msg);
    }

    /// A toast in a tone: done, waiting on the user, or gone wrong.
    fn say_as(&mut self, tone: shell::Tone, msg: impl Into<String>) {
        let msg = msg.into();
        self.toast = Some(shell::Toast::new(tone, i18n::lookup(&msg).map_or(msg, String::from)));
    }

    /// The specimen board, while it's open; the toasts it asks for.
    fn specimen_ui(&mut self, ctx: &egui::Context, cost: Option<f32>) {
        let Some(board) = &mut self.specimen else { return };
        let (open, toast) = board.show(ctx, &self.theme, cost);
        if !open {
            self.specimen = None;
        }
        match toast {
            Some(shell::Tone::Done) => self.say_undo(tr!("Chat deleted"), "specimen"),
            Some(tone) => self.say_as(tone, tr!("A toast in its tone: it holds while hovered, a click sends it away.")),
            None => {}
        }
    }

    /// A toast with an Undo: clicked, `what` lands in `toast_acted` for its owner to take.
    fn say_undo(&mut self, msg: impl Into<String>, what: &'static str) {
        self.say_as(shell::Tone::Done, msg);
        self.toast = self.toast.take().map(|t| t.with_action(tr!("Undo"), what));
    }

    fn base_note(&self) -> u8 {
        (self.settings.octave as u8 + 1) * 12
    }

    fn note_on(&mut self, note: u8) {
        self.played = true;
        if let Some(e) = &mut self.engine {
            e.send(Msg::NoteOn { note, velocity: self.settings.velocity });
        }
    }

    fn note_off(&mut self, note: u8) {
        if let Some(e) = &mut self.engine {
            e.send(Msg::NoteOff { note });
        }
    }

    /// Files dropped on the window: projects open, folders join the browser, audio is
    /// kept for whichever panel is under the cursor (playlist → clip, rack → channel).
    fn take_drops(&mut self, ctx: &egui::Context) {
        let files: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if files.is_empty() {
            return;
        }
        self.drop_at = os_cursor(ctx);
        // pictures dropped on the open AI chat go with the next message
        let files = self.chat_take_drop(self.drop_at, files);
        for path in files {
            let ext = path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default();
            if path.is_dir() {
                let folder = path.to_string_lossy().into_owned();
                if !self.settings.folders.contains(&folder) {
                    self.settings.folders.push(folder);
                }
                self.say_as(Tone::Done, tr!("Added {} to the browser", path.display()));
            } else if files::EXTENSIONS.contains(&ext.as_str()) {
                self.request(files::Pending::Open(path));
            } else if browser::AUDIO_EXTENSIONS.contains(&ext.as_str()) || ravune_core::INSTRUMENT_EXTENSIONS.contains(&ext.as_str()) {
                self.dropped.push(path);
            } else {
                self.say_as(Tone::Error, tr!("Can't use {}: not audio, a project or a folder", path.display()));
            }
        }
    }

    /// A click spark where the pointer is (unless motion is off).
    fn spark(&mut self, ui: &egui::Ui, color: egui::Color32) {
        if let (Some(pos), true) = (ui.input(|i| i.pointer.interact_pos()), self.theme.motion > 0.0) {
            self.sparks.push(kit::Spark { pos, at: ui.input(|i| i.time), color });
        }
    }

    /// "Label (Ctrl B)" with whatever the keymap binds to `command` right now.
    fn tip(&self, label: &str, command: &str) -> String {
        match self.settings.keymap.label(command) {
            keys if keys.is_empty() => label.to_string(),
            keys => format!("{label} ({keys})"),
        }
    }

    /// Runs a keymap command (`keymap::COMMANDS`): app actions here, the rest are operators.
    fn command(&mut self, id: &str) {
        let tab = |name: &str| shell::EDITORS.iter().find(|e| e.3 == name).map(|e| e.0);
        match id {
            "app.palette" => self.open_palette(),
            "app.new" => self.request(files::Pending::New),
            "app.open" => self.open_dialog(),
            "app.save" => {
                self.save();
            }
            "app.save_as" => {
                self.save_as();
            }
            "app.export" => self.export_dialog(),
            "app.record" => self.toggle_record(),
            "app.credits" => self.export_credits(),
            "app.preferences" => self.toggle_preferences(),
            "app.browser" => self.settings.show_browser = !self.settings.show_browser,
            "app.editors" => self.settings.show_editor = !self.settings.show_editor,
            "app.keys" => self.settings.show_keys = !self.settings.show_keys,
            "app.visualizer" => self.settings.viz.show = !self.settings.viz.show,
            "app.specimen" => self.specimen = if self.specimen.is_some() { None } else { Some(specimen::Specimen::default()) },
            "app.chat" => {
                self.settings.show_chat = !self.settings.show_chat;
                self.chat.focus = self.settings.show_chat;
            }
            "app.search" => {
                self.settings.show_browser = true;
                self.browser.focus_search = true;
            }
            // the channel's settings (what the Device tab was): beside the rack or the piano roll
            "editor.device" => {
                let beside = matches!(self.tab, Tab::Rack | Tab::PianoRoll) || matches!(self.split, Some(Tab::Rack | Tab::PianoRoll));
                self.settings.inspector = !(self.settings.inspector && beside);
                if !beside {
                    self.show_editor(Tab::Rack);
                }
            }
            "window.device" => {}
            "editor.next" | "editor.prev" => {
                let n = shell::EDITORS.len();
                let i = shell::EDITORS.iter().position(|e| e.0 == self.tab).unwrap_or(0);
                let step = if id == "editor.next" { 1 } else { n - 1 };
                self.show_editor(shell::EDITORS[(i + step) % n].0);
            }
            "transport.rewind" => self.run("transport.set_position", json!({ "tick": 0 })),
            "workspace.next" => self.step_workspace(1),
            "workspace.prev" => self.step_workspace(-1),
            _ => match (tab(id), tab(&id.replacen("window.", "editor.", 1))) {
                (Some(t), _) => self.show_editor(t),
                (None, Some(t)) => self.toggle_window(t),
                _ => self.run(id, json!({})),
            },
        }
    }

    /// Controller moves: the first one after "MIDI learn" maps itself; then mapped ones
    /// turn their knobs (one undo step per sweep).
    fn apply_ccs(&mut self) {
        let ccs: Vec<(u8, u8, u8)> = self.midi.ccs.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        if self.learning.is_some() {
            self.info_next = tr!("MIDI learn: move a knob or fader on your controller · Esc cancels").into();
        }
        for (channel, cc, value) in ccs {
            if let Some(target) = self.learning.take() {
                self.run("midi.learn", json!({ "channel": channel, "cc": cc, "target": target }));
                self.say_as(Tone::Done, tr!("CC {} on channel {} now moves {}", cc, channel + 1, mixer::target_name(&self.session.project, target)));
                continue;
            }
            let targets: Vec<ravune_core::Target> = self.session.project.midi_map.iter().filter(|m| m.cc == cc && m.channel == channel).map(|m| m.target).collect();
            for target in targets {
                self.set_target(target, value as f32 / 127.0, &format!("cc-{channel}-{cc}"));
            }
        }
    }

    /// Sets a parameter from a 0..1 position along its knob's travel.
    fn set_target(&mut self, target: ravune_core::Target, norm: f32, merge: &str) {
        use ravune_core::Target;
        let p = &self.session.project;
        let (op, props) = match target {
            Target::Synth { slot, param } => {
                let Some(channel) = p.channels.iter().position(|c| c.slot == slot) else { return };
                ("synth.set_param", json!({ "channel": channel, "param": param, "value": param.info().from_norm(norm) }))
            }
            Target::Device { id, param } => {
                let Some(value) = p.device(id).and_then(|(_, _, d)| d.kind.params().get(param).map(|i| i.from_norm(norm))) else { return };
                ("device.set", json!({ "id": id, "param": param, "value": value }))
            }
            Target::TrackGain { track } => ("mixer.set", json!({ "track": track, "gain": norm * 1.25 })),
            Target::TrackPan { track } => ("mixer.set", json!({ "track": track, "pan": norm * 2.0 - 1.0 })),
            Target::ChannelGain { slot } => {
                let Some(channel) = p.channels.iter().position(|c| c.slot == slot) else { return };
                ("channel.set", json!({ "channel": channel, "gain": norm }))
            }
            Target::Macro { index } => ("macro.set", json!({ "index": index, "value": norm })),
        };
        self.run_merge(op, props, merge);
    }

    /// Preferences → Keys is waiting for a shortcut: the next key press is it.
    fn capture_binding(&mut self, ctx: &egui::Context) {
        let Some(id) = self.binding else { return };
        let got = ctx.input_mut(|i| {
            let mut got = None;
            i.events.retain(|e| match e {
                // Ctrl, Shift, Alt alone: wait for the key they go with
                Event::Key { key, physical_key, pressed: true, modifiers, .. } if got.is_none() && !keymap::is_modifier(*key) => {
                    got = Some((keymap::chord_key(*key, *physical_key, *modifiers), *modifiers));
                    false
                }
                Event::Key { .. } => false,
                _ => true,
            });
            got
        });
        let Some((key, m)) = got else { return };
        self.binding = None;
        self.space_panned |= key == Key::Space; // its release mustn't start playback
        let label = keymap::COMMANDS.iter().find(|c| c.id == id).map_or(id, |c| i18n::t(c.label));
        if key == Key::Escape {
            return;
        }
        if m.is_none() && matches!(key, Key::Backspace | Key::Delete) {
            self.settings.keymap.clear(id);
            self.say(tr!("{}: no shortcut", label));
            return;
        }
        // Ctrl is "command" off the Mac, like the presets
        let mac = cfg!(target_os = "macos");
        let mods = Modifiers { alt: m.alt, shift: m.shift, ctrl: mac && m.ctrl, command: m.command || (!mac && m.ctrl), mac_cmd: false };
        let keys = KeyboardShortcut::new(mods, key);
        if keymap::reserved(&keys) {
            self.say(tr!("{} plays notes or moves focus: add Ctrl, Alt or Shift", keymap::text(&keys)));
            return;
        }
        let msg = match self.settings.keymap.bind(id, keys) {
            Some(other) => tr!("{} now runs {} (it was {})", keymap::text(&keys), label, i18n::t(other)),
            None => tr!("{} now runs {}", keymap::text(&keys), label),
        };
        self.say_as(Tone::Done, msg);
    }

    /// Help for the Info bar: whatever the mouse is over or the keyboard has focused.
    fn hint(&mut self, r: &Response, text: impl Into<String>) {
        if r.hovered() || r.has_focus() {
            self.info_next = text.into();
        }
    }

    /// Help for a whole area (`ui`'s), in the Info bar while the mouse is over it and nothing
    /// in it says something of its own: how the area works, kept out of its bar.
    fn zone_hint(&mut self, ui: &egui::Ui, text: impl Into<String>) {
        if ui.rect_contains_pointer(ui.max_rect()) {
            self.info_zone_next = text.into();
        }
    }

    // ---- input --------------------------------------------------------------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| !i.key_down(Key::Space) && !i.key_released(Key::Space)) {
            self.space_panned = false; // released while typing somewhere: forget the pan
        }
        if self.binding.is_some() {
            self.capture_binding(ctx);
            return;
        }
        if self.learning.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.learning = None;
        }
        let typing = ctx.text_edit_focused(); // a focused button or knob still lets shortcuts through
        if !typing && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F1)) {
            self.show_help = !self.show_help;
        }
        let modal = self.palette.is_some() || self.pending.is_some() || self.recovery.is_some() || self.crash.is_some() || self.rename.is_some() || self.show_help;
        if typing || modal {
            return; // typing in a field or a dialog is open
        }
        // the keymap; exact modifiers, so Ctrl+K never fires for Ctrl+Shift+K
        let bindings = self.settings.keymap.bindings();
        let mut fired = Vec::new();
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                Event::Key { key, physical_key, pressed: true, repeat: false, modifiers } => {
                    let key = keymap::chord_key(*key, *physical_key, *modifiers);
                    match bindings.iter().find(|(_, k)| k.logical_key == key && modifiers.matches_exact(k.modifiers)) {
                        Some(&(id, _)) => {
                            fired.push(id);
                            false
                        }
                        None => true,
                    }
                }
                _ => true,
            })
        });
        for id in fired {
            self.command(id);
        }
        // a tap on Space plays / stops; a hold is the playlist's hand tool
        ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Space)); // so focused buttons don't take it
        if ctx.input(|i| i.key_released(Key::Space) && i.modifiers.is_none()) && !std::mem::take(&mut self.space_panned) {
            self.run("transport.play_toggle", json!({}));
        }
        for event in ctx.input(|i| i.events.clone()) {
            match event {
                Event::Key { key, physical_key, pressed, repeat, modifiers } => {
                    if modifiers.command || modifiers.ctrl || modifiers.alt {
                        continue;
                    }
                    let key = physical_key.unwrap_or(key); // same spot on AZERTY/QWERTZ
                    if pressed && !repeat {
                        let s = &mut self.settings;
                        match key {
                            Key::Z => s.octave = (s.octave - 1).max(0),
                            Key::X => s.octave = (s.octave + 1).min(8),
                            Key::C => s.velocity = (s.velocity - 0.1).max(0.1),
                            Key::V => s.velocity = (s.velocity + 0.1).min(1.0),
                            _ => {}
                        }
                    }
                    let Some(&(_, offset, _)) = PIANO_KEYS.iter().find(|(k, ..)| *k == key) else { continue };
                    let held = self.keys_down.iter().position(|(k, _)| *k == key);
                    match (pressed, held) {
                        (true, None) => {
                            let note = self.base_note() + offset;
                            self.keys_down.push((key, note));
                            self.note_on(note);
                        }
                        (false, Some(i)) => {
                            let (_, note) = self.keys_down.remove(i);
                            self.note_off(note);
                        }
                        _ => {}
                    }
                }
                Event::WindowFocused(false) => {
                    for (_, note) in std::mem::take(&mut self.keys_down) {
                        self.note_off(note); // no stuck notes when alt-tabbing
                    }
                }
                _ => {}
            }
        }
    }
}

impl eframe::App for App {
    /// Housekeeping that must also run while the window is minimized.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time);
        self.keep_engine_alive(now);
        self.midi.poll(now, &self.midi_sink, ctx);
        if let Some(e) = &mut self.engine {
            e.collect_garbage();
        }
        self.autosave(now);
        self.poll_export();
        self.poll_recording();
        self.poll_agent(ctx, now);
        self.poll_osc(ctx);
        self.usage.sample();
        self.chat_poll();
        self.poll_online();
        self.poll_extensions(ctx);
        self.sync_plugins();
        self.poll_scan();
        self.apply_ccs();
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ui.input(|i| i.time);
        files::capture_to_file(&ctx);
        self.info = std::mem::take(&mut self.info_next);
        self.info_zone = std::mem::take(&mut self.info_zone_next);
        self.handle_keys(&ctx);
        self.take_drops(&ctx);
        if let Some(e) = &self.engine {
            self.level = e.stats().take_peak().max(self.level * 0.85);
            for (i, m) in self.meters.iter_mut().enumerate() {
                *m = e.stats().take_meter(i).max(*m * 0.85);
                let hold = &mut self.holds[i];
                *hold = if *m >= *hold { *m } else { (*hold * 0.985).max(*m) };
            }
        }
        self.beat = match &self.engine {
            Some(e) if self.session.playing => Some(e.stats().beat()),
            _ => None,
        };

        // Regions touch; panels draw 1-px dividers between them and nothing has margins.
        let t = self.theme.clone();
        let fill = |c| Frame::new().fill(c);
        egui::Panel::top("menu").frame(fill(t.bg_panel)).exact_size(t.bar_h).show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("top").frame(fill(t.bg_panel)).exact_size(t.bar_h).show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status").frame(fill(t.bg_field)).exact_size(24.0).show(ui, |ui| self.status_bar(ui));
        // the chat takes the full height beside everything else, like an IDE's side bar
        let was = self.settings.show_chat;
        let mut chat = was;
        egui::Panel::right("chat").frame(fill(t.bg_panel)).resizable(true).default_size(380.0).size_range(300.0..=640.0).show_collapsible(ui, &mut chat, |ui| self.chat_panel(ui));
        if chat != was {
            self.settings.show_chat = chat;
        }
        // every side region folds away with a slide; folded, it leaves a rail or a bar
        // a chevron inside the panel changes the setting itself: keep that, unless the panel changed it
        // declared before the keys: at the bottom it sits under them
        self.visualizer_panel(ui);
        let was = self.settings.show_keys;
        let mut keys = was;
        egui::Panel::bottom("keys").frame(fill(t.bg_panel)).show_collapsible(ui, &mut keys, |ui| self.keyboard_panel(ui));
        if keys != was {
            self.settings.show_keys = keys;
        }
        let was = self.settings.show_browser;
        let mut browser = was;
        egui::Panel::show_switched(
            ui,
            &mut browser,
            egui::Panel::left("browser-rail").frame(fill(t.bg_panel)).exact_size(38.0).resizable(false),
            egui::Panel::left("browser").frame(fill(t.bg_panel)).resizable(true).default_size(264.0).size_range(200.0..=480.0),
            |ui, open| if open { self.browser_panel(ui) } else { self.browser_rail(ui) },
        );
        if browser != was {
            self.settings.show_browser = browser;
        }
        let was = self.settings.show_editor;
        let mut editor_open = was;
        let editor = egui::Panel::show_switched(
            ui,
            &mut editor_open,
            egui::Panel::bottom("editor-bar").frame(fill(t.bg_base)).exact_size(t.bar_h).resizable(false),
            egui::Panel::bottom("editor").frame(fill(t.bg_base)).resizable(true).default_size(360.0),
            |ui, open| if open { self.editor_area(ui) } else { self.editor_tabs(ui) },
        );
        if editor_open != was {
            self.settings.show_editor = editor_open;
        }
        if self.tab == Tab::Rack && self.drop_at.is_some_and(|p| editor.response.rect.contains(p)) {
            for path in std::mem::take(&mut self.dropped) {
                self.run("channel.add", json!({ "sample": path.to_string_lossy() }));
            }
        }
        let song_beat = self.beat.filter(|_| self.session.song_mode);
        egui::CentralPanel::default().frame(fill(t.bg_base)).show(ui, |ui| self.playlist(ui, song_beat));
        if !self.dropped.is_empty() {
            self.drop_on_playlist(None); // dropped somewhere else: new lanes at the start marker
        }
        self.windows(&ctx);
        self.visualizer_window(&ctx);
        self.preferences(&ctx, now);
        self.theme_editor(&ctx);
        self.shortcut_sheet(&ctx);
        self.manual(&ctx);
        self.about(&ctx);
        self.lesson_card(&ctx, now);
        self.rename_dialog(&ctx);
        self.palette_ui(&ctx);
        self.file_modals(&ctx);
        self.specimen_ui(&ctx, frame.info().cpu_usage);
        self.toast_ui(&ctx);
        kit::sparks(&ctx, &mut self.sparks, now);
        self.sync_engine();

        let title = format!("{}{} · Ravune", self.file_name(), if self.dirty() { " •" } else { "" });
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        let busy = self.session.playing
            || self.level > 0.001
            || self.meters.iter().any(|m| *m > 0.001)
            || self.engine.as_ref().is_some_and(|e| e.stats().any_held());
        if busy {
            // about 60 frames a second, also when nothing (vsync) holds the window back:
            // covered, minimized or off-screen
            ctx.request_repaint_after(Duration::from_millis(16));
        } else {
            ctx.request_repaint_after(Duration::from_secs(1)); // MIDI hot-plug + device checks
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.keep_workspace();
        eframe::set_value(storage, "settings", &self.settings);
    }

    fn on_exit(&mut self) {
        self.exit_cleanup();
        self.finish_update();
    }
}
