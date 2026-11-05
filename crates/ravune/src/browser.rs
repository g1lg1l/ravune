//! Sample library (built-in kit + decoded-sample cache) and the browser panel: Sounds
//! (favorites, the built-in kit, your folders as a tree), Instruments (VA-1 presets) and
//! Plugins (what's installed; loading arrives in M7). Every row works from the keyboard.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Align2, CornerRadius, EventFilter, FontId, Id, Key, LayerId, Order, Rect, RichText, Ui, pos2, vec2};
use ravune_core::{INSTRUMENT_EXTENSIONS, Instrument, SYNTH_PRESETS, is_instrument, note_name, sample_name};
use ravune_engine::{Msg, MultiMap, Sample};
use serde_json::json;

use crate::shell::{Rename, Renaming};
use crate::theme::Theme;
use crate::{App, icons, kit};
use crate::shell::Tone;

/// The built-in kit: every .wav in `assets/library` (listed by build.rs), named by its path
/// there. The sounds at the top were synthesized by `ravune-engine/examples/make_kit.rs`;
/// the folders hold CC0 recordings (see assets/library/LICENSE.txt). All CC0.
pub(crate) const BUILTIN: &[(&str, &[u8])] = &include!(concat!(env!("OUT_DIR"), "/builtin.rs"));

/// Where Ravune keeps its library of free instruments and sounds: Documents/Ravune/Library.
pub fn library_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default();
    home.join("Documents").join("Ravune").join("Library")
}

/// The kit's folders, in the order the browser shows them (others follow, A–Z).
const KIT_FOLDERS: [&str; 9] = ["Kicks", "Snares", "Claps", "Hi-hats", "Cymbals", "Toms", "Percussion", "808 & Bass", "FX"];

/// The kit folder a built-in sound shows in: its own, or for the synthesized ones at the top,
/// the one its name says.
fn kit_folder(name: &str) -> &str {
    if let Some((folder, _)) = name.rsplit_once('/') {
        return folder;
    }
    match name {
        n if n.starts_with("kick") => "Kicks",
        n if n.starts_with("snare") => "Snares",
        n if n.starts_with("clap") => "Claps",
        n if n.starts_with("hat") => "Hi-hats",
        n if n.starts_with("tom") => "Toms",
        n if n.starts_with("sub") => "808 & Bass",
        n if n.starts_with("noise") => "FX",
        _ => "Percussion",
    }
}

pub const AUDIO_EXTENSIONS: [&str; 7] = ["wav", "flac", "mp3", "ogg", "aif", "aiff", "wave"];

/// Search walks at most this many folders, so a huge library can't stall a frame.
const SEARCH_FOLDERS: usize = 4000;
const SEARCH_RESULTS: usize = 400;
const INDENT: f32 = 14.0;

/// Decoded samples by path. Nothing is ever evicted, so the audio thread never drops
/// the last reference to a sample.
/// ponytail: unbounded for the session; add a deferred-free collector if memory matters.
#[derive(Default)]
pub struct Library {
    cache: HashMap<String, Result<Arc<Sample>, String>>,
    listings: HashMap<PathBuf, (Vec<PathBuf>, Vec<PathBuf>)>,
    instruments: HashMap<String, Result<InstrumentInfo, String>>,
}

/// What the browser and the device view show about a multi-sample instrument.
#[derive(Clone)]
pub struct InstrumentInfo {
    pub regions: usize,
    pub range: (u8, u8),
    /// The sample nearest middle C, for previews.
    pub preview: Arc<Sample>,
}

impl InstrumentInfo {
    pub fn summary(&self) -> String {
        tr!("{} regions · {}–{}", self.regions, note_name(self.range.0), note_name(self.range.1))
    }
}

impl Library {
    pub fn get(&mut self, path: &str) -> Result<Arc<Sample>, String> {
        self.cache.entry(path.to_string()).or_insert_with(|| load(path)).clone()
    }

    /// Reads an .sfz / .dspreset once (its samples land in the cache too).
    pub fn instrument(&mut self, path: &str) -> Result<InstrumentInfo, String> {
        if let Some(info) = self.instruments.get(path) {
            return info.clone();
        }
        let info = MultiMap::load(Path::new(path), &mut |p| self.get(p).ok()).map(|map| {
            let near = map.regions.iter().min_by_key(|r| (r.root - 60.0).abs() as u32).map(|r| r.sample.clone());
            InstrumentInfo { regions: map.regions.len(), range: map.range(), preview: near.unwrap_or_else(|| map.regions[0].sample.clone()) }
        });
        self.instruments.insert(path.to_string(), info.clone());
        info
    }

    /// Sub-folders and audio files of `dir`, cached until [`Self::refresh`].
    fn list(&mut self, dir: &Path) -> &(Vec<PathBuf>, Vec<PathBuf>) {
        self.listings.entry(dir.to_path_buf()).or_insert_with(|| {
            let (mut dirs, mut files) = (Vec::new(), Vec::new());
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if is_audio(&path) || is_instrument(&path.to_string_lossy()) {
                    files.push(path);
                }
            }
            dirs.sort();
            files.sort();
            (dirs, files)
        })
    }

    pub fn refresh(&mut self) {
        self.listings.clear();
        self.instruments.clear();
    }

    /// Audio files under `root` whose name contains `query`, breadth-first and bounded.
    fn search(&mut self, root: &Path, query: &str, out: &mut Vec<String>, budget: &mut usize) {
        let mut queue = vec![root.to_path_buf()];
        while let Some(dir) = queue.pop() {
            if *budget == 0 || out.len() >= SEARCH_RESULTS {
                return;
            }
            *budget -= 1;
            let (dirs, files) = self.list(&dir).clone();
            out.extend(files.iter().map(|f| f.to_string_lossy().into_owned()).filter(|f| sample_name(f).to_lowercase().contains(query)));
            queue.extend(dirs.into_iter().rev());
        }
    }
}

/// A sample's bytes and facts, to carry it into an exported file (DAWproject).
pub fn sample_audio(library: &mut Library, path: &str) -> Option<crate::dawproject::Audio> {
    let bytes = match path.strip_prefix("builtin:") {
        Some(name) => BUILTIN.iter().find(|(n, _)| *n == name)?.1.to_vec(),
        None => std::fs::read(path).ok()?,
    };
    let sound = library.get(path).ok()?;
    let file = Path::new(path.trim_start_matches("builtin:")).file_name()?.to_string_lossy().into_owned();
    let channels = crate::dawproject::channels_of(&bytes);
    Some(crate::dawproject::Audio { bytes, file, seconds: sound.seconds() as f64, rate: sound.rate as u32, channels })
}

fn load(path: &str) -> Result<Arc<Sample>, String> {
    if let Some(name) = path.strip_prefix("preset:") {
        return preset_note(name).map(Arc::new);
    }
    match path.strip_prefix("builtin:") {
        Some(name) => {
            let (_, bytes) = BUILTIN.iter().find(|(n, _)| *n == name).ok_or(format!("no built-in sample '{name}'"))?;
            Sample::decode_bytes(bytes, Path::new(name).extension().and_then(|e| e.to_str()))
        }
        None => Sample::decode_file(Path::new(path)),
    }
    .map(Arc::new)
}

/// A VA-1 preset playing middle C for a beat, rendered once (the engine previews it like
/// any sound).
fn preset_note(name: &str) -> Result<Sample, String> {
    use ravune_core::{Channel, Clip, ClipKind, Instrument, Note, PPQ, Pattern, Project};
    let (name, params) = ravune_core::synth_preset(name).ok_or(format!("no preset '{name}'"))?;
    let mut p = Project { channels: vec![Channel::new(0, name, Instrument::Synth { params })], patterns: vec![Pattern::default()], tempo: 120.0, ..Project::default() };
    p.patterns[0].notes_mut(0).push(Note { start: 0, length: PPQ, key: 60, velocity: 110, ..Note::default() });
    p.clips = vec![Clip { lane: 0, start: 0, length: PPQ, offset: 0, kind: ClipKind::Pattern { pattern: 0 }, fx: Default::default() }];
    let transport = ravune_engine::Transport { playing: true, mode: ravune_engine::Mode::Song, start: 0, jump: 0, looped: None };
    let song = ravune_engine::Song::compile(&p, transport, |_| None);
    let rate = 48_000.0;
    let stereo = ravune_engine::render_offline(song, rate, 1.0, |_| {});
    let mut mono: Vec<f32> = stereo.chunks(2).map(|f| (f[0] + f[1]) * 0.5).collect();
    // the release's tail, not the silence after it
    mono.truncate(mono.iter().rposition(|x| x.abs() > 1e-4).map_or(0, |i| i + 1));
    Ok(Sample::new(mono, rate))
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Opens the folder a file is in, with the file picked out where the system can.
fn show_in_folder(path: &str) {
    let result = if cfg!(windows) {
        std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").args(["-R", path]).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(Path::new(path).parent().unwrap_or(Path::new("."))).spawn()
    };
    if let Err(e) = result {
        eprintln!("can't show {path}: {e}");
    }
}

/// Drag-and-drop payload: a sample path (or `builtin:` name).
pub struct SampleDrag(pub String);

/// Drag-and-drop payload: a pattern (its index).
pub struct PatternDrag(pub usize);

/// Drag-and-drop payload: a plugin from the scan.
pub struct PluginDrag(pub ravune_bridge::Found);

#[derive(Default, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Tab {
    #[default]
    Sounds,
    Patterns,
    Instruments,
    Plugins,
    Online,
}

const TABS: [(Tab, &str, &str); 5] = [
    (Tab::Sounds, icons::WAVEFORM, "Sounds"),
    (Tab::Patterns, icons::SQUARES_FOUR, "Patterns"),
    (Tab::Instruments, icons::PIANO_KEYS, "Synths"),
    (Tab::Plugins, icons::PLUGS, "Plugins"),
    (Tab::Online, icons::GLOBE, "Online"),
];

#[derive(Default)]
pub struct Browser {
    query: String,
    /// The last clicked or focused item, highlighted (and previewed if it's a sound).
    pub selected: Option<String>,
    /// Ctrl+F: put the cursor in the search field.
    pub focus_search: bool,
    pub tab: Tab,
    /// Tree nodes whose open / closed state differs from their default.
    toggled: HashSet<String>,
    /// A preview playing, and when it ends by itself: till then the bar has a Stop.
    playing: Option<std::time::Instant>,
    /// Files or folders waiting on "Delete?": what to call them, and them.
    doomed: Option<(String, Vec<PathBuf>)>,
}

impl Browser {
    /// Sections start open, folders inside them closed.
    pub(crate) fn is_open(&self, key: &str, default: bool) -> bool {
        default != self.toggled.contains(key)
    }

    pub(crate) fn toggle(&mut self, key: &str) {
        if !self.toggled.remove(key) {
            self.toggled.insert(key.to_string());
        }
    }
}

/// Where CLAP and VST3 plugins are installed, per platform.
fn plugin_dirs() -> Vec<(&'static str, PathBuf)> {
    let mut dirs = Vec::new();
    if cfg!(windows) {
        for (kind, sub) in [("CLAP", "CLAP"), ("VST3", "VST3")] {
            dirs.push((kind, PathBuf::from(r"C:\Program Files\Common Files").join(sub)));
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                dirs.push((kind, PathBuf::from(local).join(r"Programs\Common").join(sub)));
            }
        }
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let mac = cfg!(target_os = "macos");
        for (kind, system, user) in [
            ("CLAP", if mac { "/Library/Audio/Plug-Ins/CLAP" } else { "/usr/lib/clap" }, if mac { "Library/Audio/Plug-Ins/CLAP" } else { ".clap" }),
            ("VST3", if mac { "/Library/Audio/Plug-Ins/VST3" } else { "/usr/lib/vst3" }, if mac { "Library/Audio/Plug-Ins/VST3" } else { ".vst3" }),
        ] {
            dirs.push((kind, PathBuf::from(system)));
            dirs.push((kind, home.join(user)));
        }
    }
    dirs
}

/// Every CLAP and VST3 plugin file in the standard folders.
pub fn plugin_files() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for (kind, dir) in plugin_dirs() {
        let ext = kind.to_lowercase();
        let mut stack = vec![dir];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(&ext)) {
                    found.push((kind, path)); // a .vst3 bundle is a folder: don't walk into it
                } else if path.is_dir() && found.len() < 2000 {
                    stack.push(path);
                }
            }
        }
    }
    found.sort_by(|a, b| a.1.file_stem().cmp(&b.1.file_stem()));
    found.into_iter().map(|(_, path)| path).collect()
}

/// Paints a tree row's content: indent, optional caret, icon, name, a right-hand note.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_row(ui: &Ui, t: &Theme, row: &egui::Response, depth: usize, caret: Option<bool>, icon: &str, name: &str, strong: bool, note: &str) {
    let rect = row.rect;
    let mut painter = ui.painter_at(rect);
    painter.multiply_opacity(kit::row_alpha(ui, row));
    let mut x = rect.left() + t.pad + depth as f32 * INDENT;
    let y = rect.center().y;
    if let Some(open) = caret {
        painter.text(pos2(x + 5.0, y), Align2::CENTER_CENTER, if open { icons::CARET_DOWN } else { icons::CARET_RIGHT }, FontId::proportional(11.0), t.fg_muted);
    }
    x += 14.0;
    painter.text(pos2(x + 7.0, y), Align2::CENTER_CENTER, icon, FontId::proportional(14.0), if strong { t.fg_text } else { t.fg_muted });
    x += 20.0;
    let font = if strong { FontId::proportional(12.5) } else { FontId::proportional(12.0) };
    let right = if note.is_empty() {
        rect.right() - t.pad
    } else {
        painter.text(rect.right_center() - vec2(t.pad, 0.0), Align2::RIGHT_CENTER, note, FontId::proportional(11.0), t.fg_muted).left() - t.gap * 2.0
    };
    // a name too long for the row ends in "…"; the whole of it shows on hover
    if kit::fit_text(&painter, pos2(x, y), Align2::LEFT_CENTER, name, font, t.fg_text, right - x).1 {
        row.clone().on_hover_text(name);
    }
}

impl App {
    pub fn browser_panel(&mut self, ui: &mut Ui) {
        let t = self.theme.clone();
        kit::region(ui);
        kit::bar(ui, &t, |ui| {
            // the buttons first, right to left: the tabs fit in the room they leave, never under them
            kit::right(ui, |ui| {
                if kit::icon_button(ui, &t, icons::CARET_LEFT, &self.tip(tr!("Fold the browser into a rail"), "app.browser"), false).clicked() {
                    self.settings.show_browser = false;
                }
                // a long sound (one from online can last minutes) stops here
                if let Some(until) = self.browser.playing {
                    let left = until.saturating_duration_since(std::time::Instant::now());
                    if left.is_zero() {
                        self.browser.playing = None;
                    } else {
                        ui.ctx().request_repaint_after(left);
                        if kit::icon_button(ui, &t, icons::STOP, tr!("Stop the preview"), true).clicked() {
                            self.stop_preview();
                        }
                    }
                }
                let on = self.settings.preview_on_select;
                let r = kit::icon_button(ui, &t, if on { icons::EAR } else { icons::SPEAKER_SLASH }, tr!("Preview as you select"), on);
                if r.clicked() {
                    self.settings.preview_on_select = !on;
                }
                self.hint(&r, if on {
                    tr!("Preview as you select: on, a sound or synth preset plays as soon as you click it or arrow onto it (the ▶ on a row always plays it)")
                } else {
                    tr!("Preview as you select: off, click the ▶ on a row to hear it")
                });
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let tabs = TABS.map(|(tab, icon, label)| (tab, icon, crate::i18n::t(label)));
                    if let Some(tab) = kit::tabs(ui, &t, Id::unique("browser-tabs"), self.browser.tab, &tabs, true) {
                        self.browser.tab = tab;
                    }
                });
            });
        });
        match self.browser.tab {
            Tab::Sounds => self.sounds(ui, &t),
            Tab::Patterns => self.patterns_tab(ui, &t),
            Tab::Instruments => self.instruments(ui, &t),
            Tab::Plugins => self.plugins_tab(ui, &t),
            Tab::Online => self.online_tab(ui, &t),
        }
        self.paint_drag_label(ui);
    }

    /// The folded browser: one icon per tab; any of them opens it again.
    pub fn browser_rail(&mut self, ui: &mut Ui) {
        let t = self.theme.clone();
        kit::region(ui);
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = t.gap;
            ui.add_space(t.gap);
            if kit::icon_button(ui, &t, icons::CARET_RIGHT, &self.tip(tr!("Open the browser"), "app.browser"), false).clicked() {
                self.settings.show_browser = true;
            }
            for (tab, icon, label) in TABS {
                if kit::icon_button(ui, &t, icon, crate::i18n::t(label), self.browser.tab == tab).clicked() {
                    self.browser.tab = tab;
                    self.settings.show_browser = true;
                }
            }
        });
    }

    // ---- sounds -------------------------------------------------------------

    fn sounds(&mut self, ui: &mut Ui, t: &Theme) {
        kit::bar(ui, t, |ui| {
            let search = kit::field(&mut self.browser.query)
                .hint_text(format!("{}  {}   Ctrl F", icons::MAGNIFYING_GLASS, tr!("Search sounds")))
                .desired_width(ui.available_width() - 2.0 * 26.0 - 2.0 * t.gap);
            let r = ui.add(search);
            if std::mem::take(&mut self.browser.focus_search) {
                r.request_focus();
            }
            self.hint(&r, tr!("Filter the kit, your favorites and every folder below (subfolders too)."));
            let r = kit::icon_button(ui, t, icons::FOLDER_PLUS, tr!("Add a folder with your sounds"), false);
            if r.clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                let dir = dir.to_string_lossy().into_owned();
                if !self.settings.folders.contains(&dir) {
                    self.settings.folders.push(dir);
                }
            }
            if kit::icon_button(ui, t, icons::ARROW_COUNTER_CLOCKWISE, tr!("Rescan folders for new files"), false).clicked() {
                self.library.refresh();
            }
        });
        let query = self.browser.query.trim().to_lowercase();
        self.confirm_delete(ui.ctx());
        kit::scroll(ui, t, "browser-sounds", false, t.bg_panel, |ui| {
            if query.is_empty() {
                self.sound_tree(ui, t);
            } else {
                self.sound_search(ui, t, &query);
            }
        });
    }

    fn sound_tree(&mut self, ui: &mut Ui, t: &Theme) {
        if !self.settings.favorites.is_empty() && self.section(ui, t, "favorites", icons::STAR, tr!("Favorites"), "") {
            for path in self.settings.favorites.clone() {
                self.file_row(ui, t, &path, 1, "");
            }
        }
        if self.section(ui, t, "library", icons::MUSIC_NOTES, tr!("Built-in kit"), &tr!("{} sounds · CC0", BUILTIN.len())) {
            // folders in a set order, each folded until opened
            let mut folders: Vec<&str> = BUILTIN.iter().map(|(n, _)| kit_folder(n)).collect();
            folders.sort_by_key(|f| (KIT_FOLDERS.iter().position(|k| k == f).unwrap_or(KIT_FOLDERS.len()), f.to_string()));
            folders.dedup();
            for folder in folders {
                let key = format!("kit:{folder}");
                let open = self.browser.is_open(&key, false);
                let names: Vec<&str> = BUILTIN.iter().map(|(n, _)| *n).filter(|n| kit_folder(n) == folder).collect();
                let r = kit::row(ui, t, Id::unique(("kit-folder", folder)), false);
                paint_row(ui, t, &r, 1, Some(open), if open { icons::FOLDER_OPEN } else { icons::FOLDER }, folder, false, &names.len().to_string());
                if r.clicked() || self.tree_keys(ui, &r, &key, open) {
                    self.browser.toggle(&key);
                }
                self.hint(&r, tr!("{}: click or ← → to open", folder));
                if open {
                    for name in names {
                        self.file_row(ui, t, &format!("builtin:{name}"), 2, "");
                    }
                }
            }
        }
        // the library Ravune fills (free instruments and more), when it's there
        let library = library_dir();
        if library.is_dir() && !self.settings.folders.iter().any(|f| Path::new(f) == library) && self.section(ui, t, "ravune-library", icons::PIANO_KEYS, tr!("Library"), "Documents/Ravune") {
            self.folder(ui, t, &library, 1);
        }
        let mut remove = None;
        for (i, folder) in self.settings.folders.clone().iter().enumerate() {
            let name = Path::new(folder).file_name().map_or(folder.clone(), |n| n.to_string_lossy().into_owned());
            let key = format!("root:{folder}");
            let open = self.browser.is_open(&key, true);
            let r = kit::row(ui, t, Id::unique(("browser-root", folder)), false);
            paint_row(ui, t, &r, 0, Some(open), if open { icons::FOLDER_OPEN } else { icons::FOLDER }, &name, true, "");
            let online = Path::new(folder) == self.settings.online.folder();
            let mut clear = false;
            if r.hovered() {
                let x = Rect::from_center_size(r.rect.right_center() - vec2(t.pad + 6.0, 0.0), vec2(16.0, 16.0));
                ui.painter().text(x.center(), Align2::CENTER_CENTER, icons::X, FontId::proportional(12.0), t.fg_muted);
                if r.clicked() && r.interact_pointer_pos().is_some_and(|p| x.contains(p)) {
                    remove = Some(i);
                }
                // Online Sounds: a trash beside it clears it (after a "Delete?")
                if online {
                    let bin = x.translate(vec2(-20.0, 0.0));
                    let on = r.hover_pos().is_some_and(|p| bin.contains(p));
                    ui.painter().text(bin.center(), Align2::CENTER_CENTER, icons::TRASH, FontId::proportional(12.0), if on { t.meter_hot } else { t.fg_muted });
                    clear = r.clicked() && r.interact_pointer_pos().is_some_and(|p| bin.contains(p));
                }
            }
            if clear {
                let all: Vec<PathBuf> = std::fs::read_dir(folder).into_iter().flatten().flatten().map(|e| e.path()).collect();
                self.browser.doomed = Some((tr!("everything in {} ({})", name, all.len()), all));
            } else if self.tree_keys(ui, &r, &key, open) || (r.clicked() && remove.is_none()) {
                self.browser.toggle(&key);
            }
            self.take_sound(ui, t, &r, Path::new(folder));
            self.hint(&r, if online {
                tr!("{} · click or ← → to open · drop a sound here to move it in · the bin clears it · × removes it from the browser", folder)
            } else {
                tr!("{} · click or ← → to open · drop a sound here to move it in · × removes it from the browser", folder)
            });
            if open {
                self.folder(ui, t, Path::new(folder), 1);
            }
        }
        if let Some(i) = remove {
            self.settings.folders.remove(i);
        }
        if self.settings.folders.is_empty() {
            let r = kit::row(ui, t, Id::unique("browser-add-folder"), false);
            paint_row(ui, t, &r, 0, None, icons::FOLDER_PLUS, tr!("Add a folder with your sounds…"), false, "");
            if r.clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                self.settings.folders.push(dir.to_string_lossy().into_owned());
            }
            self.hint(&r, tr!("Pick a folder (subfolders included), or drop one onto the window from Explorer."));
        }
    }

    /// A section header row; returns whether it's open.
    pub(crate) fn section(&mut self, ui: &mut Ui, t: &Theme, key: &str, icon: &str, name: &str, note: &str) -> bool {
        let open = self.browser.is_open(key, true);
        let r = kit::row(ui, t, Id::unique(("browser-section", key)), false);
        paint_row(ui, t, &r, 0, Some(open), icon, name, true, note);
        if r.clicked() || self.tree_keys(ui, &r, key, open) {
            self.browser.toggle(key);
        }
        open
    }

    /// Right / Left open and close a focused folder row. Returns true to toggle it.
    fn tree_keys(&mut self, ui: &Ui, r: &egui::Response, _key: &str, open: bool) -> bool {
        if !r.has_focus() {
            return false;
        }
        ui.memory_mut(|m| m.set_focus_lock_filter(r.id, EventFilter { horizontal_arrows: true, ..Default::default() }));
        let (right, left) = ui.input(|i| (i.key_pressed(Key::ArrowRight), i.key_pressed(Key::ArrowLeft)));
        (right && !open) || (left && open)
    }

    fn folder(&mut self, ui: &mut Ui, t: &Theme, dir: &Path, depth: usize) {
        let (dirs, files) = self.library.list(dir).clone();
        for sub in dirs {
            let name = sub.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let key = sub.to_string_lossy().into_owned();
            let open = self.browser.is_open(&key, false);
            let r = kit::row(ui, t, Id::unique(("browser-dir", &key)), false);
            paint_row(ui, t, &r, depth, Some(open), if open { icons::FOLDER_OPEN } else { icons::FOLDER }, &name, false, "");
            if r.clicked() || self.tree_keys(ui, &r, &key, open) {
                self.browser.toggle(&key);
            }
            self.take_sound(ui, t, &r, &sub);
            r.context_menu(|ui| {
                if ui.button(format!("{}  {}", icons::TRASH, tr!("Delete the folder…"))).clicked() {
                    self.browser.doomed = Some((tr!("the folder {} and all in it", name), vec![sub.clone()]));
                    ui.close();
                }
            });
            if open {
                self.folder(ui, t, &sub, depth + 1);
            }
        }
        for file in files {
            self.file_row(ui, t, &file.to_string_lossy(), depth, "");
        }
    }

    /// Sounds whose name contains `query` (lower case): favorites, the built-in kit, the
    /// user's folders and the library. Also what agents search.
    pub fn find_sounds(&mut self, query: &str) -> Vec<String> {
        let mut hits: Vec<String> = self.settings.favorites.iter().filter(|f| sample_name(f).to_lowercase().contains(query)).cloned().collect();
        let kit: Vec<String> = BUILTIN.iter().map(|(n, _)| format!("builtin:{n}")).filter(|p| p.to_lowercase().contains(query) && !hits.contains(p)).collect();
        hits.extend(kit);
        let mut budget = SEARCH_FOLDERS;
        for folder in self.settings.folders.clone().into_iter().chain([library_dir().to_string_lossy().into_owned()]) {
            self.library.search(Path::new(&folder), query, &mut hits, &mut budget);
        }
        hits
    }

    fn sound_search(&mut self, ui: &mut Ui, t: &Theme, query: &str) {
        let hits = self.find_sounds(query);
        if hits.is_empty() {
            kit::padded(ui, t, |ui| {
                ui.label(RichText::new(tr!("Nothing here matches.")).color(t.fg_muted));
                if kit::button(ui, t, Some(icons::GLOBE), &tr!("Search \"{}\" online", query), false).clicked() {
                    self.browser.tab = Tab::Online;
                    self.search_online(ui.ctx(), query);
                }
            });
        }
        for path in hits {
            let parent = match path.strip_prefix("builtin:") {
                Some(name) => kit_folder(name).to_string(),
                None => Path::new(&path).parent().and_then(Path::file_name).map_or(String::new(), |p| p.to_string_lossy().into_owned()),
            };
            self.file_row(ui, t, &path, 0, &parent);
        }
    }

    /// One sound. Click or arrow onto it = preview; double-click or Enter = new channel;
    /// drag = onto the rack or the playlist; the star keeps it in Favorites.
    fn file_row(&mut self, ui: &mut Ui, t: &Theme, path: &str, depth: usize, note: &str) {
        let selected = self.browser.selected.as_deref() == Some(path);
        let r = kit::row(ui, t, Id::unique(("browser-file", path)), selected);
        let favorite = self.settings.favorites.iter().any(|f| f == path);
        let star = Rect::from_center_size(r.rect.right_center() - vec2(t.pad + 6.0, 0.0), vec2(18.0, r.rect.height()));
        let on_star = r.interact_pointer_pos().or(r.hover_pos()).is_some_and(|p| star.contains(p));
        let play = star.translate(vec2(-20.0, 0.0));
        let on_play = r.interact_pointer_pos().or(r.hover_pos()).is_some_and(|p| play.contains(p));
        let icon = if is_instrument(path) { icons::PIANO_KEYS } else { icons::WAVEFORM };
        paint_row(ui, t, &r, depth, None, icon, sample_name(path), false, if r.hovered() || favorite { "" } else { note });
        if favorite || r.hovered() {
            let color = if favorite { t.accent } else { t.fg_muted };
            ui.painter().text(star.center(), Align2::CENTER_CENTER, icons::STAR, FontId::proportional(13.0), color);
        }
        if r.hovered() {
            ui.painter().text(play.center(), Align2::CENTER_CENTER, icons::PLAY, FontId::proportional(12.0), if on_play { t.accent } else { t.fg_muted });
        }
        if r.drag_started() {
            r.dnd_set_drag_payload(SampleDrag(path.to_string()));
        }
        let enter = r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if r.clicked() && on_star {
            if favorite {
                self.settings.favorites.retain(|f| f != path);
            } else {
                self.settings.favorites.push(path.to_string());
            }
        } else if r.clicked() && on_play {
            self.browser.selected = Some(path.to_string());
            self.preview(path);
        } else if r.double_clicked() || enter {
            self.run("channel.add", json!({ "sample": path }));
        } else if r.clicked() || r.gained_focus() {
            self.browser.selected = Some(path.to_string());
            if self.settings.preview_on_select {
                self.preview(path);
            }
        }
        r.context_menu(|ui| self.sound_menu(ui, path, favorite));
        if is_instrument(path) {
            let about = self.library.instrument(path).map_or_else(|e| e, |i| i.summary());
            self.hint(&r, tr!("Multi-sample instrument · {} · double-click or Enter adds it as a channel, then play it from the keys", about));
        } else {
            self.hint(&r, tr!("Click or ↑ ↓ to preview (▶ always plays) · double-click or Enter adds a channel · drag onto the rack or the playlist · ☆ favorite"));
        }
    }

    /// Right-click on a sound: preview, a new channel, the selected channel, the playlist,
    /// favorite, where it lives.
    fn sound_menu(&mut self, ui: &mut Ui, path: &str, favorite: bool) {
        if ui.button(format!("{}  {}", icons::PLAY, tr!("Preview"))).clicked() {
            self.preview(path);
            ui.close();
        }
        if ui.add(egui::Button::new(format!("{}  {}", icons::PLUS, tr!("Add as a new channel"))).shortcut_text("Enter")).clicked() {
            self.run("channel.add", json!({ "sample": path }));
            ui.close();
        }
        if let Some(name) = self.session.selected_channel().map(|c| c.name.clone())
            && ui.button(format!("{}  {}", icons::ARROW_RIGHT, tr!("Send to “{}”", name))).clicked()
        {
            self.replace_instrument(self.session.selected, None, Some(path));
            ui.close();
        }
        if !is_instrument(path) && ui.button(format!("{}  {}", icons::LIST, tr!("Put on the playlist at the start marker"))).clicked() {
            let lane = self.session.project.clips.iter().map(|c| c.lane + 1).max().unwrap_or(0);
            self.add_audio_clip(path, lane, self.session.position);
            ui.close();
        }
        if ui.button(format!("{}  {}", icons::STAR, if favorite { tr!("Remove from favorites") } else { tr!("Add to favorites") })).clicked() {
            if favorite {
                self.settings.favorites.retain(|f| f != path);
            } else {
                self.settings.favorites.push(path.to_string());
            }
            ui.close();
        }
        ui.separator();
        if !path.starts_with("builtin:") && ui.button(format!("{}  {}", icons::FOLDER_OPEN, tr!("Show in the file manager"))).clicked() {
            show_in_folder(path);
            ui.close();
        }
        if ui.button(format!("{}  {}", icons::COPY, tr!("Copy the path"))).clicked() {
            ui.ctx().copy_text(path.to_string());
            ui.close();
        }
        if !path.starts_with("builtin:") {
            ui.separator();
            if ui.button(format!("{}  {}", icons::TRASH, tr!("Delete…"))).clicked() {
                self.browser.doomed = Some((sample_name(path).to_string(), vec![PathBuf::from(path)]));
                ui.close();
            }
        }
    }

    /// A folder row taking a sound dropped on it: the file moves in (not the built-in kit's).
    fn take_sound(&mut self, ui: &Ui, t: &Theme, r: &egui::Response, dir: &Path) {
        let movable = |p: &str| !p.starts_with("builtin:") && !is_instrument(p) && Path::new(p).parent() != Some(dir);
        if kit::drag_over::<SampleDrag>(r).is_some_and(|d| movable(&d.0)) {
            ui.painter().rect_stroke(r.rect, CornerRadius::same(t.radius_sm), egui::Stroke::new(2.0, t.select), egui::StrokeKind::Inside);
        }
        if let Some(d) = kit::dropped::<SampleDrag>(r).filter(|d| movable(&d.0)) {
            self.move_sound(&d.0, dir);
        }
    }

    /// Moves a sound file into `dir` (a name taken there gets a number), with its credit,
    /// and points the song's channels and clips, and the favorites, at its new place.
    pub fn move_sound(&mut self, path: &str, dir: &Path) {
        let from = Path::new(path);
        let (Some(stem), Some(old)) = (from.file_stem().map(|s| s.to_string_lossy().into_owned()), from.file_name().map(|s| s.to_string_lossy().into_owned())) else { return };
        let ext = from.extension().map_or(String::new(), |e| format!(".{}", e.to_string_lossy()));
        let mut to = dir.join(&old);
        for n in 2.. {
            if !to.exists() {
                break;
            }
            to = dir.join(format!("{stem} {n}{ext}"));
        }
        // across drives a rename can't: then a copy, and the old one goes
        if let Err(e) = std::fs::rename(from, &to).or_else(|_| std::fs::copy(from, &to).and_then(|_| std::fs::remove_file(from))) {
            self.say_as(Tone::Error, tr!("Couldn't move {}: {}", old, e));
            return;
        }
        let new = to.to_string_lossy().into_owned();
        if let Some(parent) = from.parent() {
            crate::online::move_credit(parent, dir, &old, &to.file_name().map_or(old.clone(), |n| n.to_string_lossy().into_owned()));
        }
        let p = &self.session.project;
        let channels: Vec<(usize, String, f32, f32)> = p.channels.iter().enumerate().filter(|(_, c)| matches!(&c.instrument, Instrument::Sampler { sample } if sample == path)).map(|(i, c)| (i, c.name.clone(), c.start, c.end)).collect();
        let clips: Vec<usize> = p.clips.iter().enumerate().filter(|(_, c)| matches!(&c.kind, ravune_core::ClipKind::Audio { sample } if sample == path)).map(|(k, _)| k).collect();
        if !channels.is_empty() || !clips.is_empty() {
            self.session.begin_group("Move sound");
            for (i, name, start, end) in channels {
                self.run("channel.set", json!({ "channel": i, "sample": new, "name": name, "start": start, "end": end }));
            }
            for k in clips {
                self.run("clip.set", json!({ "index": k, "sample": new }));
            }
            self.session.end_group();
        }
        for f in self.settings.favorites.iter_mut().filter(|f| *f == path) {
            *f = new.clone();
        }
        self.library.refresh();
        self.say_as(Tone::Done, tr!("Moved {} to {}", old, dir.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned())));
    }

    /// "Delete?" for what's waiting on it: deleted for good (not to the recycle bin) on yes.
    fn confirm_delete(&mut self, ctx: &egui::Context) {
        let Some((what, paths)) = self.browser.doomed.clone() else { return };
        let mut choice = None;
        egui::Modal::new(Id::unique("browser-delete")).show(ctx, |ui| {
            ui.set_max_width(380.0);
            ui.label(RichText::new(tr!("Delete {}?", what)).heading());
            ui.label(tr!("It's deleted from the disk for good, not put in the recycle bin. A song that uses it will miss it."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                choice = [(tr!("Delete"), true), (tr!("Cancel"), false)].into_iter().find_map(|(label, yes)| ui.button(label).clicked().then_some(yes));
            });
        });
        match choice {
            Some(true) => {
                self.browser.doomed = None;
                let mut gone = 0;
                for p in &paths {
                    let done = if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
                    if done.is_ok() {
                        gone += 1;
                        if let (Some(dir), Some(name)) = (p.parent(), p.file_name()) {
                            crate::online::drop_credit(dir, &name.to_string_lossy());
                        }
                        self.settings.favorites.retain(|f| !Path::new(f).starts_with(p));
                    }
                }
                self.library.refresh();
                self.say_as(Tone::Done, if gone == paths.len() { tr!("Deleted {}", what) } else { tr!("Deleted {} of {}: the rest is in use or locked", gone, paths.len()) });
            }
            Some(false) => self.browser.doomed = None,
            None => {}
        }
    }

    /// Silences the browser's preview.
    pub fn stop_preview(&mut self) {
        self.browser.playing = None;
        if let Some(e) = &mut self.engine {
            e.send(Msg::Preview(None));
        }
    }

    pub fn preview(&mut self, path: &str) {
        let sample = if is_instrument(path) { self.library.instrument(path).map(|i| i.preview) } else { self.library.get(path) };
        match sample {
            Ok(sample) => {
                self.browser.playing = Some(std::time::Instant::now() + std::time::Duration::from_secs_f32(sample.seconds()));
                if let Some(e) = &mut self.engine {
                    e.send(Msg::Preview(Some(sample)));
                }
            }
            Err(e) => self.say_as(Tone::Error, e),
        }
    }

    // ---- instruments --------------------------------------------------------

    fn instruments(&mut self, ui: &mut Ui, t: &Theme) {
        kit::scroll(ui, t, "browser-instruments", false, t.bg_panel, |ui| {
            if self.section(ui, t, "va1", icons::WAVE_SAWTOOTH, tr!("VA-1 synth"), tr!("built in")) {
                for (name, _) in SYNTH_PRESETS {
                    let key = format!("preset:{name}");
                    let selected = self.browser.selected.as_deref() == Some(key.as_str());
                    let r = kit::row(ui, t, Id::unique(("browser-preset", name)), selected);
                    paint_row(ui, t, &r, 1, None, icons::MUSIC_NOTE, name, false, "");
                    let play = Rect::from_center_size(r.rect.right_center() - vec2(t.pad + 6.0, 0.0), vec2(18.0, r.rect.height()));
                    let on_play = r.interact_pointer_pos().or(r.hover_pos()).is_some_and(|p| play.contains(p));
                    if r.hovered() {
                        ui.painter().text(play.center(), Align2::CENTER_CENTER, icons::PLAY, FontId::proportional(12.0), if on_play { t.accent } else { t.fg_muted });
                    }
                    let enter = r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if r.clicked() && on_play {
                        self.browser.selected = Some(key.clone());
                        self.preview(&key);
                    } else if r.double_clicked() || enter {
                        self.run("channel.add", json!({ "preset": name }));
                    } else if r.clicked() || r.gained_focus() {
                        if self.settings.preview_on_select {
                            self.preview(&key);
                        }
                        self.browser.selected = Some(key);
                    }
                    self.hint(&r, tr!("{}: click or ↑ ↓ to hear it (▶ always plays) · double-click or Enter adds a VA-1 channel with this sound.", name));
                }
            }
            if self.section(ui, t, "sampler", icons::WAVEFORM, "Sampler", tr!("built in")) {
                let r = kit::row(ui, t, Id::unique("browser-sampler-tip"), false);
                paint_row(ui, t, &r, 1, None, icons::INFO, tr!("Drag any sound from Sounds"), false, "");
                self.hint(&r, tr!("Every sound you add becomes a sampler channel: pitch, volume, choke groups."));
                let r = kit::row(ui, t, Id::unique("browser-multi-tip"), false);
                paint_row(ui, t, &r, 1, None, icons::PIANO_KEYS, tr!("SFZ and Decent Sampler"), false, &INSTRUMENT_EXTENSIONS.join(" "));
                self.hint(&r, tr!("Multi-sample instruments (.sfz, .dspreset) show up in Sounds next to your samples: add one and play it across the keys."));
            }
        });
    }

    // ---- patterns -----------------------------------------------------------

    /// Every pattern: click to edit it, drag it onto the playlist, right-click for more.
    fn patterns_tab(&mut self, ui: &mut Ui, t: &Theme) {
        kit::scroll(ui, t, "browser-patterns", false, t.bg_panel, |ui| {
            let r = kit::row(ui, t, Id::unique("pattern-new"), false);
            paint_row(ui, t, &r, 0, None, icons::PLUS, tr!("New pattern"), true, "F4");
            if r.clicked() || r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                self.run("pattern.add", json!({}));
                self.tab = crate::Tab::Rack;
            }
            self.hint(&r, tr!("An empty pattern, opened in the rack. Drag it onto the playlist when it's ready."));
            let project = &self.session.project;
            let list: Vec<(String, Option<[u8; 3]>, usize, usize)> = project
                .patterns
                .iter()
                .enumerate()
                .map(|(i, p)| (p.name.clone(), p.color, p.length, project.clips.iter().filter(|c| c.kind == ravune_core::ClipKind::Pattern { pattern: i }).count()))
                .collect();
            let current = self.session.pattern;
            for (i, (name, color, length, uses)) in list.iter().enumerate() {
                let r = kit::row(ui, t, Id::unique(("pattern-row", i)), i == current);
                let size = match length {
                    16 => tr!("1 bar").to_string(),
                    l if l % 16 == 0 => tr!("{} bars", l / 16),
                    l => tr!("{} steps", l),
                };
                let note = if *uses == 0 { size } else { format!("{size} · {uses}×") };
                paint_row(ui, t, &r, 0, None, "", name, i == current, &note);
                let mut painter = ui.painter_at(r.rect);
                painter.multiply_opacity(kit::row_alpha(ui, &r));
                let chip = Rect::from_center_size(pos2(r.rect.left() + t.pad + 21.0, r.rect.center().y), vec2(11.0, 11.0));
                painter.rect_filled(chip, CornerRadius::same(t.radius_sm), Theme::pattern(i, *color));
                if r.drag_started() {
                    r.dnd_set_drag_payload(PatternDrag(i));
                }
                let enter = r.has_focus() && ui.input(|inp| inp.key_pressed(Key::Enter));
                if r.double_clicked() || enter {
                    self.run("pattern.select", json!({ "pattern": i }));
                    self.tab = crate::Tab::Rack;
                } else if r.clicked() {
                    self.run("pattern.select", json!({ "pattern": i }));
                }
                r.context_menu(|ui| {
                    if ui.button(format!("{}  {}", icons::PENCIL_SIMPLE, tr!("Rename…"))).clicked() {
                        self.rename = Some(Renaming::new(Rename::Pattern(i), name.clone()));
                        ui.close();
                    }
                    ui.menu_button(format!("{}  {}", icons::CIRCLE, tr!("Color")), |ui| self.pattern_color_menu(ui, i));
                    if ui.button(format!("{}  {}", icons::COPY, tr!("Clone"))).clicked() {
                        self.run("pattern.duplicate", json!({ "pattern": i }));
                        ui.close();
                    }
                    if ui.add_enabled(list.len() > 1, egui::Button::new(format!("{}  {}", icons::TRASH, tr!("Delete (and its clips)")))).clicked() {
                        self.run("pattern.remove", json!({ "pattern": i }));
                        ui.close();
                    }
                });
                self.hint(&r, tr!("{}: click to show it in the rack and piano roll · double-click opens the rack · drag onto the playlist · right-click to rename, color, clone", name));
            }
        });
    }

    /// Shows what's being dragged (a sound, a pattern) under the cursor.
    fn paint_drag_label(&self, ui: &Ui) {
        let ctx = ui.ctx();
        let Some(pos) = ctx.pointer_hover_pos() else { return };
        let (text, fill) = if let Some(s) = egui::DragAndDrop::payload::<SampleDrag>(ctx) {
            (format!("{}  {}", icons::WAVEFORM, sample_name(&s.0)), self.theme.accent)
        } else if let Some(p) = egui::DragAndDrop::payload::<PatternDrag>(ctx)
            && let Some(pattern) = self.session.project.patterns.get(p.0)
        {
            (format!("{}  {}", icons::SQUARES_FOUR, pattern.name), Theme::pattern(p.0, pattern.color))
        } else if let Some(p) = egui::DragAndDrop::payload::<PluginDrag>(ctx) {
            (format!("{}  {}", if p.0.instrument { icons::PIANO_KEYS } else { icons::PLUG }, p.0.name), self.theme.select)
        } else {
            return;
        };
        let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::unique("browser-drag")));
        let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), self.theme.bg_base);
        let rect = Rect::from_min_size(pos + vec2(12.0, 8.0), galley.size() + vec2(12.0, 6.0));
        painter.rect_filled(rect, CornerRadius::same(self.theme.radius_md), fill);
        painter.galley(rect.min + vec2(6.0, 3.0), galley, self.theme.bg_base);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_presets_preview_as_a_note() {
        for (name, _) in SYNTH_PRESETS {
            let s = load(&format!("preset:{name}")).unwrap();
            let peak = s.frames.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak > 0.01 && s.seconds() > 0.1 && s.seconds() < 3.5, "{name}: peak {peak}, {} s", s.seconds());
        }
        assert!(load("preset:Nope").is_err());
    }

    /// A sound dropped on a folder moves there, with its credit, and the song follows it.
    #[test]
    fn a_sound_moves_into_a_folder_and_the_song_follows() {
        let root = std::env::temp_dir().join("ravune-move-test");
        let _ = std::fs::remove_dir_all(&root);
        let dest = root.join("Bass");
        std::fs::create_dir_all(&dest).unwrap();
        let file = root.join("growl.wav");
        std::fs::write(&file, b"RIFF").unwrap();
        std::fs::write(dest.join("growl.wav"), b"taken").unwrap();
        std::fs::write(root.join("credits.json"), r#"{"growl.wav": {"provider": "Openverse", "id": "1", "title": "Growl", "author": "A", "license": "CC0", "license_url": "", "page": "", "audio": "", "ext": "wav", "seconds": 1.0, "tags": []}}"#).unwrap();
        let mut app = App::for_tests();
        let path = file.to_string_lossy().into_owned();
        app.session.run("channel.add", json!({ "sample": path })).unwrap();
        let ch = app.session.project.channels.len() - 1;
        app.session.run("channel.set", json!({ "channel": ch, "name": "Growly", "start": 0.25 })).unwrap();
        app.move_sound(&path, &dest);
        let moved = dest.join("growl 2.wav");
        assert!(moved.exists() && !file.exists(), "moved, the name taken there gets a number");
        let c = &app.session.project.channels[ch];
        assert!(matches!(&c.instrument, Instrument::Sampler { sample } if Path::new(sample) == moved), "the channel follows it");
        assert_eq!((c.name.as_str(), c.start), ("Growly", 0.25), "and keeps its name and cut");
        let credits = std::fs::read_to_string(dest.join("credits.json")).unwrap();
        assert!(credits.contains("growl 2.wav") && !root.join("credits.json").exists(), "the credit moves along");
    }

    /// Every built-in sound decodes (WAV and FLAC), has a sound in it and sits in a folder.
    #[test]
    fn the_whole_library_loads() {
        assert!(BUILTIN.len() > 140, "{} sounds", BUILTIN.len());
        for (name, _) in BUILTIN {
            let s = load(&format!("builtin:{name}")).unwrap_or_else(|e| panic!("{name}: {e}"));
            let peak = s.frames.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak > 0.1 && s.seconds() > 0.01 && s.seconds() < 10.0, "{name}: peak {peak}, {} s", s.seconds());
        }
        for genre in ["Drum & Bass", "Liquid DnB", "Techno", "Hardgroove", "IDM", "House", "Electronic"] {
            assert!(BUILTIN.iter().filter(|(n, _)| kit_folder(n) == genre).count() >= 10, "{genre}");
        }
    }
}
