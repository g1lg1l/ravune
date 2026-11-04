//! Plugins in the app (M7): the scan database behind the Plugins tab, one host process per
//! plugin instrument or effect in the project (started in the background, attached to the
//! engine when ready), their states saved with the project, editors, parameters, and what
//! happens when one crashes: its sound stops, Ravune carries on, Reload brings it back.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, UNIX_EPOCH};

use eframe::egui::{self, Align2, FontId, Id, Key, RichText, Ui, pos2, vec2};
use ravune_bridge::{Command, Format, Found, Hosted, ParamInfo, Reply};
use ravune_core::{DeviceKind, Instrument, PluginFormat, PluginRef};
use ravune_engine::{Msg, PluginTarget};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::theme::Theme;
use crate::{App, icons, kit};
use crate::shell::Tone;

const LOAD: Duration = Duration::from_secs(20);
const SCAN: Duration = Duration::from_secs(30);
const ASK: Duration = Duration::from_secs(3);
/// Parameters shown on a device card (the rest through the filter).
const SHOWN: usize = 24;

/// ravune-host, next to the app.
fn host_exe() -> PathBuf {
    crate::extensions::sibling("host")
}

fn format(f: PluginFormat) -> Format {
    match f {
        PluginFormat::Clap => Format::Clap,
        PluginFormat::Vst3 => Format::Vst3,
    }
}

pub fn plugin_ref(found: &Found) -> PluginRef {
    let format = match found.format {
        Format::Clap => PluginFormat::Clap,
        Format::Vst3 => PluginFormat::Vst3,
    };
    PluginRef { format, path: found.path.clone(), id: found.id.clone(), name: found.name.clone(), state: String::new() }
}

/// Where Ravune keeps its own files (the plugin database).
fn config_dir() -> PathBuf {
    config_dir_named("Ravune")
}

pub(crate) fn config_dir_named(app: &str) -> PathBuf {
    let base = std::env::var_os("APPDATA").or_else(|| std::env::var_os("XDG_CONFIG_HOME")).map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    base.unwrap_or_default().join(app)
}

fn db_path() -> PathBuf {
    config_dir().join("plugins.json")
}

fn modified(path: &str) -> u64 {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// One scanned plugin file.
#[derive(Clone, Serialize, Deserialize)]
pub struct Scanned {
    pub path: String,
    pub modified: u64,
    pub found: Vec<Found>,
    pub error: Option<String>,
}

/// What a plugin is doing, for its card.
pub enum Status {
    Loading,
    Running,
    Crashed,
    Failed(String),
}

type Spec = (Format, String, String);

fn spec(p: &PluginRef) -> Spec {
    (format(p.format), p.path.clone(), p.id.clone())
}

struct Instance {
    hosted: Hosted,
    spec: Spec,
    rate: u32,
    crashed: bool,
    /// The engine has its bridge.
    attached: bool,
    params: Option<Vec<ParamInfo>>,
    editor: bool,
}

struct Loading {
    spec: Spec,
    rate: u32,
    rx: Receiver<Result<Hosted, String>>,
}

#[derive(Default)]
pub struct Plugins {
    db: Option<Vec<Scanned>>,
    /// A scan running: its results, how many files, how many done.
    scan: Option<(Receiver<Scanned>, usize, usize)>,
    pub query: String,
    running: HashMap<PluginTarget, Instance>,
    loading: HashMap<PluginTarget, Loading>,
    /// Load errors, so a broken plugin isn't retried every frame (Reload clears it).
    failed: HashMap<PluginTarget, (Spec, String)>,
    /// Hosts let go of, kept a moment so the audio thread is surely done with them.
    retired: Vec<(Instant, Hosted)>,
    pub param_filter: String,
    /// Plugins just added: their window opens once they've loaded.
    open_on_load: std::collections::HashSet<PluginTarget>,
}

impl Plugins {
    pub fn db(&mut self) -> &[Scanned] {
        self.db.get_or_insert_with(|| std::fs::read_to_string(db_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default())
    }

    fn save_db(&self) {
        if let Some(db) = &self.db
            && let Ok(text) = serde_json::to_string_pretty(db)
        {
            let _ = std::fs::create_dir_all(config_dir());
            let _ = std::fs::write(db_path(), text);
        }
    }

    pub fn scanning(&self) -> Option<(usize, usize)> {
        self.scan.as_ref().map(|s| (s.2, s.1))
    }
}

impl App {
    fn send_engine(&mut self, msg: Msg) {
        if let Some(e) = &mut self.engine {
            e.send(msg);
        }
    }

    /// The plugins the project wants running.
    fn plugins_wanted(&self) -> Vec<(PluginTarget, PluginRef)> {
        let p = &self.session.project;
        let channels = p.channels.iter().filter_map(|c| match &c.instrument {
            Instrument::Plugin { plugin } => Some((PluginTarget::Channel(c.slot), plugin.clone())),
            _ => None,
        });
        let devices = p.mixer.iter().flat_map(|t| &t.devices).filter_map(|d| match (&d.kind, &d.plugin) {
            (DeviceKind::Plugin, Some(plugin)) => Some((PluginTarget::Device(d.id), plugin.clone())),
            _ => None,
        });
        channels.chain(devices).collect()
    }

    /// Starts, attaches, retires and watches the plugin hosts (every frame; cheap without plugins).
    pub fn sync_plugins(&mut self) {
        let Some(rate) = self.engine.as_ref().map(|e| e.sample_rate) else { return };
        let wanted = self.plugins_wanted();
        let pl = &mut self.plugins;
        if wanted.is_empty() && pl.running.is_empty() && pl.loading.is_empty() && pl.retired.is_empty() {
            return;
        }
        pl.retired.retain(|(at, _)| at.elapsed() < Duration::from_secs(2));
        // gone from the project (or now a different plugin / sample rate): let go
        let stale: Vec<PluginTarget> = pl.running.iter().filter(|(t, i)| !wanted.iter().any(|(w, p)| w == *t && spec(p) == i.spec && i.rate == rate)).map(|(t, _)| *t).collect();
        for target in stale {
            if let Some(i) = self.plugins.running.remove(&target) {
                self.plugins.retired.push((Instant::now(), i.hosted));
            }
            self.send_engine(Msg::Plugin { target, bridge: None });
        }
        let pl = &mut self.plugins;
        pl.loading.retain(|t, l| wanted.iter().any(|(w, p)| w == t && spec(p) == l.spec && l.rate == rate));
        pl.failed.retain(|t, (s, _)| wanted.iter().any(|(w, p)| w == t && spec(p) == *s));
        // finished loading
        let done: Vec<(PluginTarget, Result<Hosted, String>)> = pl.loading.iter().filter_map(|(t, l)| l.rx.try_recv().ok().map(|r| (*t, r))).collect();
        for (target, result) in done {
            let Some(l) = self.plugins.loading.remove(&target) else { continue };
            match result {
                Ok(hosted) => {
                    let name = hosted.name.clone();
                    self.plugins.running.insert(target, Instance { hosted, spec: l.spec, rate: l.rate, crashed: false, attached: false, params: None, editor: false });
                    if self.plugins.open_on_load.remove(&target) {
                        self.toggle_plugin_editor(target, &name);
                    }
                }
                Err(e) => {
                    self.say_as(Tone::Error, tr!("Couldn't load {}: {}", l.spec.1.rsplit(['/', '\\']).next().unwrap_or_default(), e));
                    self.plugins.failed.insert(target, (l.spec, e));
                }
            }
        }
        // start what's missing
        for (target, plugin) in &wanted {
            let pl = &mut self.plugins;
            if pl.running.contains_key(target) || pl.loading.contains_key(target) || pl.failed.contains_key(target) {
                continue;
            }
            let (tx, rx) = channel();
            let (fmt, path, id, state) = (format(plugin.format), plugin.path.clone(), plugin.id.clone(), Some(plugin.state.clone()).filter(|s| !s.is_empty()));
            std::thread::spawn(move || {
                let _ = tx.send(Hosted::spawn(&host_exe(), fmt, &path, &id, rate as f64, state, LOAD));
            });
            pl.loading.insert(*target, Loading { spec: spec(plugin), rate, rx });
        }
        // attach new ones, notice crashes, collect events
        let mut crashed = Vec::new();
        let mut attach = Vec::new();
        for (target, i) in &mut self.plugins.running {
            if !i.crashed && !i.hosted.running() {
                i.crashed = true;
                crashed.push(i.hosted.name.clone());
            }
            for reply in i.hosted.poll() {
                match reply {
                    Reply::Param { id, value } => {
                        if let Some(p) = i.params.as_mut().and_then(|ps| ps.iter_mut().find(|p| p.id == id)) {
                            p.value = value;
                        }
                    }
                    Reply::GuiClosed => i.editor = false,
                    _ => {}
                }
            }
            if !i.attached {
                i.attached = true;
                attach.push((*target, i.hosted.bridge.clone()));
            }
        }
        for (target, bridge) in attach {
            self.send_engine(Msg::Plugin { target, bridge: Some(bridge) });
        }
        for name in crashed {
            self.say_as(Tone::Error, tr!("{} crashed: its sound is off, Ravune is fine. Reload it from its device.", name));
        }
    }

    /// A new audio engine knows no plugins yet.
    pub fn detach_plugins(&mut self) {
        for i in self.plugins.running.values_mut() {
            i.attached = false;
        }
    }

    /// Before saving: every plugin's state into the project (not an undo step).
    pub fn store_plugin_states(&mut self) {
        let targets: Vec<PluginTarget> = self.plugins.running.iter().filter(|(_, i)| !i.crashed).map(|(t, _)| *t).collect();
        for target in targets {
            let Some(i) = self.plugins.running.get_mut(&target) else { continue };
            let Ok(Reply::State { data }) = i.hosted.ask(&Command::State, ASK) else { continue };
            let props = match target {
                PluginTarget::Channel(slot) => match self.session.project.channels.iter().position(|c| c.slot == slot) {
                    Some(c) => json!({ "channel": c, "state": data }),
                    None => continue,
                },
                PluginTarget::Device(id) => json!({ "id": id, "state": data }),
            };
            self.run("plugin.store", props);
        }
    }

    pub fn plugin_status(&self, target: PluginTarget) -> Status {
        if let Some(i) = self.plugins.running.get(&target) {
            return if i.crashed { Status::Crashed } else { Status::Running };
        }
        match self.plugins.failed.get(&target) {
            Some((_, e)) => Status::Failed(e.clone()),
            None => Status::Loading,
        }
    }

    /// Restarts a crashed or failed plugin (from the project's saved state).
    fn reload_plugin(&mut self, target: PluginTarget) {
        if let Some(i) = self.plugins.running.remove(&target) {
            self.plugins.retired.push((Instant::now(), i.hosted));
        }
        self.plugins.failed.remove(&target);
        self.send_engine(Msg::Plugin { target, bridge: None });
    }

    /// Status, editor and parameters of a plugin instrument or effect (device view, mixer).
    pub fn plugin_card(&mut self, ui: &mut Ui, t: &Theme, target: PluginTarget, plugin: &PluginRef) {
        ui.horizontal(|ui| {
            match self.plugin_status(target) {
                Status::Loading => {
                    ui.add(egui::Spinner::new().size(12.0).color(t.select));
                    ui.label(RichText::new(tr!("Starting the plugin…")).small().color(t.fg_muted));
                }
                Status::Running => {
                    let editor = self.plugins.running.get(&target).is_some_and(|i| i.editor);
                    let r = kit::button(ui, t, Some(icons::APP_WINDOW), "Editor", editor);
                    if r.clicked() {
                        self.toggle_plugin_editor(target, &plugin.name);
                    }
                    self.hint(&r, tr!("The plugin's own window (it runs in its own process: if it crashes, Ravune doesn't)"));
                    let late = self.plugins.running.get(&target).map_or(0, |i| i.hosted.bridge.late());
                    let note = if late > 0 { tr!("{} · {} late blocks", format!("{:?}", plugin.format), late) } else { format!("{:?}", plugin.format) };
                    ui.label(RichText::new(note.to_uppercase()).small().color(if late > 0 { t.meter_warn } else { t.fg_muted }));
                }
                Status::Crashed => {
                    ui.label(RichText::new(format!("{}  {}", icons::WARNING, tr!("Crashed: its sound is off"))).color(t.meter_hot));
                    if kit::button(ui, t, Some(icons::ARROW_CLOCKWISE), tr!("Reload"), false).clicked() {
                        self.reload_plugin(target);
                    }
                }
                Status::Failed(e) => {
                    ui.add(egui::Label::new(RichText::new(format!("{}  {e}", icons::WARNING)).color(t.meter_hot)).truncate());
                    if kit::button(ui, t, Some(icons::ARROW_CLOCKWISE), tr!("Retry"), false).clicked() {
                        self.reload_plugin(target);
                    }
                }
            }
        });
        self.plugin_params(ui, t, target);
    }

    pub fn toggle_plugin_editor(&mut self, target: PluginTarget, name: &str) {
        let Some(i) = self.plugins.running.get_mut(&target) else { return };
        let show = !i.editor;
        match i.hosted.ask(&Command::Gui { show, title: format!("{name} · Ravune") }, ASK) {
            Ok(Reply::Ok) => i.editor = show,
            Ok(Reply::Error { message }) => self.say_as(Tone::Error, message),
            Ok(_) => {}
            Err(e) => self.say_as(Tone::Error, e),
        }
    }

    fn plugin_params(&mut self, ui: &mut Ui, t: &Theme, target: PluginTarget) {
        let Some(i) = self.plugins.running.get_mut(&target).filter(|i| !i.crashed) else { return };
        if i.params.is_none() {
            i.params = Some(match i.hosted.ask(&Command::Params, ASK) {
                Ok(Reply::Params { params }) => params,
                _ => Vec::new(),
            });
        }
        let params = i.params.clone().unwrap_or_default();
        if params.is_empty() {
            return;
        }
        ui.horizontal(|ui| {
            kit::caps(ui, t, &tr!("{} PARAMETERS", params.len()));
            if params.len() > SHOWN {
                ui.add(kit::field(&mut self.plugins.param_filter).hint_text(format!("{}  {}", icons::FUNNEL, tr!("Filter"))).desired_width(140.0));
            }
        });
        let filter = self.plugins.param_filter.to_lowercase();
        let shown: Vec<&ParamInfo> = params.iter().filter(|p| filter.is_empty() || p.name.to_lowercase().contains(&filter)).take(SHOWN).collect();
        egui::Grid::new(("plugin-params", format!("{target:?}"))).num_columns(4).spacing(vec2(10.0, 2.0)).show(ui, |ui| {
            for (n, p) in shown.iter().enumerate() {
                ui.add(egui::Label::new(RichText::new(&p.name).small().color(t.fg_muted)).truncate());
                let mut value = p.value;
                let mut slider = egui::Slider::new(&mut value, p.min..=p.max).show_value(false);
                if p.stepped {
                    slider = slider.step_by(1.0);
                }
                let r = ui.add_sized([110.0, 18.0], slider);
                if r.changed() {
                    self.set_plugin_param(target, p.id, value);
                }
                if n % 2 == 1 {
                    ui.end_row();
                }
            }
        });
    }

    fn set_plugin_param(&mut self, target: PluginTarget, id: u32, value: f64) {
        let Some(i) = self.plugins.running.get_mut(&target) else { return };
        let _ = i.hosted.send(&Command::SetParam { id, value });
        if let Some(p) = i.params.as_mut().and_then(|ps| ps.iter_mut().find(|p| p.id == id)) {
            p.value = value;
        }
    }

    // ---- scanning and the Plugins tab ---------------------------------------

    /// Scans the plugin folders in the background; unchanged files keep their last result.
    pub fn start_scan(&mut self, force: bool) {
        let files = crate::browser::plugin_files();
        let known: HashMap<String, u64> = self.plugins.db().iter().map(|s| (s.path.clone(), s.modified)).collect();
        let todo: Vec<String> = files.into_iter().map(|p| p.to_string_lossy().into_owned()).filter(|p| force || known.get(p) != Some(&modified(p))).collect();
        let keep: Vec<String> = self.plugins.db().iter().map(|s| s.path.clone()).filter(|p| std::path::Path::new(p).exists() && !todo.contains(p)).collect();
        if let Some(db) = &mut self.plugins.db {
            db.retain(|s| keep.contains(&s.path));
        }
        let (tx, rx) = channel();
        let total = todo.len();
        std::thread::spawn(move || {
            for path in todo {
                let result = ravune_bridge::scan(&host_exe(), std::path::Path::new(&path), SCAN);
                let modified = modified(&path);
                let scanned = match result {
                    Ok(found) => Scanned { path, modified, found, error: None },
                    Err(e) => Scanned { path, modified, found: Vec::new(), error: Some(e) },
                };
                if tx.send(scanned).is_err() {
                    return;
                }
            }
        });
        self.plugins.scan = Some((rx, total, 0));
    }

    pub fn poll_scan(&mut self) {
        let Some((rx, total, done)) = &mut self.plugins.scan else { return };
        let mut got = Vec::new();
        for s in rx.try_iter() {
            got.push(s);
            *done += 1;
        }
        let finished = *done >= *total;
        if !got.is_empty() {
            let db = self.plugins.db.get_or_insert_with(Vec::new);
            db.retain(|s| !got.iter().any(|g| g.path == s.path));
            db.extend(got);
            db.sort_by_key(|s| s.path.to_lowercase());
        }
        if finished {
            self.plugins.scan = None;
            self.plugins.save_db();
        }
    }

    /// The browser's Plugins tab: instruments and effects from the scan, ready to add.
    pub fn plugins_tab(&mut self, ui: &mut Ui, t: &Theme) {
        if self.plugins.db.is_none() && self.plugins.db().is_empty() {
            self.start_scan(false); // first time: find what's installed
        }
        kit::bar(ui, t, |ui| {
            let search = kit::field(&mut self.plugins.query).hint_text(format!("{}  {}", icons::MAGNIFYING_GLASS, tr!("Search plugins"))).desired_width(ui.available_width() - 26.0 - t.gap);
            ui.add(search);
            let r = kit::icon_button(ui, t, icons::ARROW_COUNTER_CLOCKWISE, tr!("Scan again (new and changed plugins; Shift: everything)"), self.plugins.scan.is_some());
            if r.clicked() && self.plugins.scan.is_none() {
                let shift = ui.input(|i| i.modifiers.shift);
                self.start_scan(shift);
            }
        });
        if let Some((done, total)) = self.plugins.scanning() {
            kit::padded(ui, t, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.0).color(t.select));
                    ui.label(RichText::new(tr!("Scanning {} / {}", done, total)).small().color(t.fg_text));
                });
                // wraps to the panel (a long line in a row would widen it)
                ui.label(RichText::new(tr!("Each plugin opens in its own process, so a bad one can't crash Ravune.")).small().color(t.fg_muted));
            });
        }
        let query = self.plugins.query.to_lowercase();
        let db = self.plugins.db().to_vec();
        let matches = |f: &&Found| query.is_empty() || f.name.to_lowercase().contains(&query) || f.vendor.to_lowercase().contains(&query) || f.features.iter().any(|x| x.to_lowercase().contains(&query));
        let all: Vec<&Found> = db.iter().flat_map(|s| &s.found).filter(matches).collect();
        let failed: Vec<&Scanned> = db.iter().filter(|s| s.error.is_some()).collect();
        kit::scroll(ui, t, "browser-plugins", false, t.bg_panel, |ui| {
            if all.is_empty() && self.plugins.scan.is_none() {
                kit::padded(ui, t, |ui| ui.label(RichText::new(tr!("No CLAP or VST3 plugins in the standard folders yet. Free ones to try: Surge XT, Vital, Dexed, OB-Xf, Airwindows.")).color(t.fg_muted)));
            }
            // Instruments and Effects, each by maker; every level folds (a search opens them)
            for (instrument, title, icon) in [(true, "Instruments", icons::PIANO_KEYS), (false, "Effects", icons::PLUG)] {
                let list: Vec<&&Found> = all.iter().filter(|f| f.instrument == instrument).collect();
                if list.is_empty() || !self.section(ui, t, &format!("plugins:{title}"), icon, crate::i18n::t(title), &list.len().to_string()) && query.is_empty() {
                    continue;
                }
                let mut vendors: std::collections::BTreeMap<String, Vec<&Found>> = std::collections::BTreeMap::new();
                for f in list {
                    let vendor = if f.vendor.trim().is_empty() { tr!("Other").to_string() } else { f.vendor.clone() };
                    vendors.entry(vendor).or_default().push(f);
                }
                for (vendor, found) in vendors {
                    let key = format!("plugins:{title}:{vendor}");
                    let open = !query.is_empty() || self.browser.is_open(&key, true);
                    let r = kit::row(ui, t, Id::unique(("plugin-vendor", &key)), false);
                    crate::browser::paint_row(ui, t, &r, 1, Some(open), icons::FOLDER, &vendor, false, &found.len().to_string());
                    if r.clicked() {
                        self.browser.toggle(&key);
                    }
                    self.hint(&r, tr!("{}: click to fold or unfold", vendor));
                    if open {
                        for f in found {
                            self.plugin_row(ui, t, f, 2);
                        }
                    }
                }
            }
            if !failed.is_empty() && self.section(ui, t, "plugins:failed", icons::WARNING, tr!("Couldn't load"), &failed.len().to_string()) {
                for s in failed {
                    let r = kit::row(ui, t, Id::unique(("plugin-failed", &s.path)), false);
                    let painter = ui.painter_at(r.rect);
                    let name = std::path::Path::new(&s.path).file_stem().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    painter.text(pos2(r.rect.left() + t.pad + 22.0, r.rect.center().y), Align2::LEFT_CENTER, name, FontId::proportional(12.0), t.fg_muted);
                    painter.text(pos2(r.rect.left() + t.pad + 7.0, r.rect.center().y), Align2::CENTER_CENTER, icons::WARNING, FontId::proportional(13.0), t.meter_warn);
                    self.hint(&r, format!("{} · {}", s.path, s.error.clone().unwrap_or_default()));
                }
            }
        });
    }

    fn plugin_row(&mut self, ui: &mut Ui, t: &Theme, f: &Found, depth: usize) {
        let key = format!("{}:{}", f.path, f.id);
        let selected = self.browser.selected.as_deref() == Some(key.as_str());
        let r = kit::row(ui, t, Id::unique(("plugin", &key)), selected);
        let mut painter = ui.painter_at(r.rect);
        painter.multiply_opacity(kit::row_alpha(ui, &r));
        let y = r.rect.center().y;
        let x = r.rect.left() + t.pad + 14.0 + depth as f32 * 14.0;
        let icon = if f.instrument { icons::PIANO_KEYS } else { icons::PLUG };
        painter.text(pos2(x + 7.0, y), Align2::CENTER_CENTER, icon, FontId::proportional(14.0), t.fg_muted);
        let badge = painter.text(r.rect.right_center() - vec2(t.pad, 0.0), Align2::RIGHT_CENTER, f.format.name(), FontId::monospace(10.0), t.fg_muted);
        if kit::fit_text(&painter, pos2(x + 20.0, y), Align2::LEFT_CENTER, f.name.as_str(), FontId::proportional(12.0), t.fg_text, badge.left() - t.gap * 2.0 - x - 20.0).1 {
            r.clone().on_hover_text(f.name.as_str());
        }
        if r.drag_started() {
            r.dnd_set_drag_payload(crate::browser::PluginDrag(f.clone()));
        }
        let enter = r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if r.double_clicked() || enter {
            self.add_plugin(f);
        } else if r.clicked() || r.gained_focus() {
            self.browser.selected = Some(key);
        }
        let what = if f.instrument {
            tr!("double-click or Enter adds it as a channel · drag it onto the rack, onto a pattern on the playlist, or onto empty playlist space for a new pattern")
        } else {
            tr!("double-click or Enter adds it to the selected mixer track · drag it onto the rack (the selected channel's track)")
        };
        self.hint(&r, format!("{} · {} {} · {} · {what}", f.name, f.vendor, f.version, f.features.join(", ")));
    }

    /// Instruments become channels; effects go on the mixer track shown in the Mixer. Either
    /// way its window opens as soon as it has loaded.
    pub fn add_plugin(&mut self, f: &Found) {
        let plugin = plugin_ref(f);
        if f.instrument {
            self.run("channel.add", json!({ "plugin": plugin }));
            self.say_as(Tone::Done, tr!("Added {} · F8 shows it, its editor and parameters", f.name));
            self.open_when_loaded_channel();
        } else {
            let track = self.mixer_track;
            self.add_effect_plugin(f, track);
        }
    }

    fn add_effect_plugin(&mut self, f: &Found, track: usize) {
        self.run("device.add", json!({ "track": track, "kind": "plugin", "plugin": plugin_ref(f) }));
        let name = self.session.project.mixer.get(track).map_or(String::new(), |t| t.name.clone());
        self.say_as(Tone::Done, tr!("Added {} to {}", f.name, name));
        if let Some(d) = self.session.project.mixer.get(track).and_then(|t| t.devices.last()) {
            self.plugins.open_on_load.insert(PluginTarget::Device(d.id));
        }
    }

    /// The selected channel's plugin opens its window once it has loaded.
    fn open_when_loaded_channel(&mut self) {
        if let Some(c) = self.session.selected_channel() {
            self.plugins.open_on_load.insert(PluginTarget::Channel(c.slot));
        }
    }

    /// Dropped on a channel's row: the plugin or sound takes over that channel and keeps its
    /// notes (an effect goes on the channel's mixer track instead).
    pub fn replace_instrument(&mut self, channel: usize, plugin: Option<&Found>, sample: Option<&str>) {
        match (plugin, sample) {
            (Some(f), _) if !f.instrument => {
                let track = self.session.project.channels.get(channel).map_or(0, |c| c.track);
                self.add_effect_plugin(f, track);
            }
            (Some(f), _) => {
                self.run("channel.replace", json!({ "channel": channel, "plugin": plugin_ref(f) }));
                self.say_as(Tone::Done, tr!("{} plays this channel's notes now", f.name));
                self.open_when_loaded_channel();
            }
            (None, Some(path)) => {
                self.run("channel.replace", json!({ "channel": channel, "sample": path }));
            }
            (None, None) => {}
        }
    }

    /// A plugin dropped on the rack: instruments become channels, effects go on the mixer
    /// track the selected channel plays into.
    pub fn drop_plugin_on_rack(&mut self, f: &Found) {
        if f.instrument {
            self.add_plugin(f);
            return;
        }
        let track = self.session.selected_channel().map_or(0, |c| c.track);
        self.add_effect_plugin(f, track);
    }

    /// Effect plugins for a "+ Effect" menu.
    pub fn effect_plugins(&mut self) -> Vec<Found> {
        self.plugins.db().iter().flat_map(|s| &s.found).filter(|f| !f.instrument).cloned().collect()
    }
}
