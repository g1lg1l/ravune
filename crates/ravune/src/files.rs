//! Project files (`.ravune`, RON text), autosave and crash recovery.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use ravune_core::{Project, Session};

use crate::shell::Tone;
use crate::{App, icons, kit};

pub const EXTENSION: &str = "ravune";
/// What Ravune opens: its own projects, and those saved while it was still called Phasor.
pub const EXTENSIONS: [&str; 2] = [EXTENSION, "phasor"];
const AUTOSAVE_SECONDS: f64 = 30.0;
/// Starting points (File → New from Template), eight bars each; built by
/// `ravune-core/examples/make_templates.rs`.
pub const TEMPLATES: [(&str, &str); 7] = [
    ("Techno · 130 BPM", include_str!("../../../assets/templates/techno.ravune")),
    ("House · 124 BPM", include_str!("../../../assets/templates/house.ravune")),
    ("Drum & Bass · 174 BPM", include_str!("../../../assets/templates/drum-and-bass.ravune")),
    ("Liquid DnB · 174 BPM", include_str!("../../../assets/templates/liquid-dnb.ravune")),
    ("Hardgroove · 145 BPM", include_str!("../../../assets/templates/hardgroove.ravune")),
    ("IDM · 120 BPM", include_str!("../../../assets/templates/idm.ravune")),
    ("Hip-hop · 90 BPM", include_str!("../../../assets/templates/hip-hop.ravune")),
];

/// An action waiting for "save changes?" confirmation.
pub enum Pending {
    New,
    Open(PathBuf),
    /// A project bundled with the app: one of [`TEMPLATES`].
    Bundled(&'static str),
    /// A DAWproject from another DAW.
    Import(PathBuf),
    /// A template an extension brought: opened as a new, unsaved song.
    Template(PathBuf),
    /// Close and open the update that's ready.
    Restart,
    /// Close Ravune (the window's close button).
    Quit,
    /// An earlier version of the open song (File → Versions…).
    Version(PathBuf),
}

/// Where your own templates live: Documents/Ravune/Templates, beside the library.
pub fn templates_dir() -> PathBuf {
    crate::browser::library_dir().with_file_name("Templates")
}

/// Your templates, by name.
pub fn user_templates() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(templates_dir()).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == EXTENSION)).collect();
    found.sort_by_key(|p| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()));
    found
}

/// A name that's safe as a file name everywhere.
fn file_safe(name: &str) -> String {
    name.trim().chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c }).collect()
}

/// How many versions a song keeps (the oldest go first).
const MAX_VERSIONS: usize = 200;

/// One save of a song, kept beside it.
pub struct Version {
    pub path: PathBuf,
    /// Seconds since 1970 when it was saved.
    pub at: u64,
    /// What changed since the version before.
    pub note: String,
}

/// Where the versions of `project` live: `.<name>.versions/` beside it, so they travel with it.
fn versions_dir(project: &Path) -> PathBuf {
    let name = project.file_name().map_or("song".into(), |n| n.to_string_lossy().into_owned());
    project.with_file_name(format!(".{name}.versions"))
}

/// How many versions the song at `project` keeps (without reading them).
pub fn version_count(project: &Path) -> usize {
    std::fs::read_dir(versions_dir(project)).map_or(0, |d| d.flatten().filter(|e| e.path().extension().is_some_and(|x| x == EXTENSION)).count())
}

/// The versions of the song saved at `project`, newest first.
pub fn versions(project: &Path) -> Vec<Version> {
    let Ok(entries) = std::fs::read_dir(versions_dir(project)) else { return Vec::new() };
    let mut list: Vec<Version> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let at = path.file_stem()?.to_str()?.parse().ok()?;
            let text = std::fs::read_to_string(&path).ok()?;
            let note = text.lines().next().and_then(|l| l.strip_prefix("// ")).unwrap_or("").to_string();
            Some(Version { path, at, note })
        })
        .collect();
    list.sort_by_key(|v| std::cmp::Reverse(v.at));
    list
}

/// Keeps `project` as a new version of the song at `path`, unless it's the same as the last
/// one. Its first line says what changed.
fn keep_version(path: &Path, project: &Project) -> Result<(), String> {
    let list = versions(path);
    let last = list.first().and_then(|v| read(&v.path).ok());
    if last.as_ref() == Some(project) {
        return Ok(());
    }
    let note = last.as_ref().map_or_else(|| tr!("First save").to_string(), |old| describe(old, project));
    let dir = versions_dir(path);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    // two saves in one second: the later one a second on
    let at = list.first().map_or(now, |v| now.max(v.at + 1));
    let text = ron::ser::to_string_pretty(project, ron::ser::PrettyConfig::default()).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{at}.{EXTENSION}")), format!("// {note}\n{text}")).map_err(|e| e.to_string())?;
    for old in list.iter().skip(MAX_VERSIONS - 1) {
        let _ = std::fs::remove_file(&old.path);
    }
    Ok(())
}

/// What changed from `old` to `new`, in a few words: "Added Bass · Tempo 120 → 124 · Notes".
fn describe(old: &Project, new: &Project) -> String {
    let names = |p: &Project| p.channels.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    let (before, after) = (names(old), names(new));
    let mut parts: Vec<String> = Vec::new();
    parts.extend(after.iter().filter(|n| !before.contains(n)).map(|n| tr!("Added {}", n)));
    parts.extend(before.iter().filter(|n| !after.contains(n)).map(|n| tr!("Removed {}", n)));
    if old.tempo != new.tempo {
        parts.push(tr!("Tempo {} → {}", old.tempo, new.tempo));
    }
    if parts.is_empty() && old.channels != new.channels {
        parts.push(tr!("Sounds").into());
    }
    if old.patterns != new.patterns {
        parts.push(tr!("Notes").into());
    }
    if old.clips != new.clips || old.markers != new.markers {
        parts.push(tr!("Arrangement").into());
    }
    if old.mixer != new.mixer {
        parts.push(tr!("Mixer").into());
    }
    if parts.is_empty() {
        parts.push(tr!("Small changes").into());
    }
    parts.join(" · ")
}

/// "just now", "5 min ago", "3 h ago", "2 days ago".
pub fn ago(at: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    match now.saturating_sub(at) {
        s if s < 60 => tr!("just now").into(),
        s if s < 3600 => tr!("{} min ago", s / 60),
        s if s < 86_400 => tr!("{} h ago", s / 3600),
        s => tr!("{} days ago", s / 86_400),
    }
}

/// Where Ravune keeps its settings, autosave and crash lock: `RAVUNE_DATA` when set (a
/// separate profile, e.g. a second copy for testing), else the OS app-data folder.
pub fn data_dir() -> Option<PathBuf> {
    std::env::var_os("RAVUNE_DATA").map(PathBuf::from).or_else(|| eframe::storage_dir("Ravune"))
}

/// Ravune was called Phasor until 0.9: the first time it runs, it moves Phasor's folders
/// over (plugins, settings and autosave, then Documents), so nothing seems lost.
pub fn move_phasor_folders() {
    if std::env::var_os("RAVUNE_DATA").is_some() {
        return;
    }
    let documents = crate::browser::library_dir().parent().map(Path::to_path_buf);
    let pairs = [
        (Some(crate::plugins::config_dir_named("Phasor")), Some(crate::plugins::config_dir_named("Ravune"))),
        (eframe::storage_dir("Phasor"), eframe::storage_dir("Ravune")),
        (documents.as_ref().map(|d| d.with_file_name("Phasor")), documents),
    ];
    for (old, new) in pairs.into_iter().filter_map(|(o, n)| Some((o?, n?))) {
        if old.exists() && !new.exists() {
            let _ = new.parent().map(std::fs::create_dir_all);
            let _ = std::fs::rename(&old, &new);
        }
    }
}

/// `RAVUNE_CAPTURE_TO=shot.png`: once the window has settled (5 s, past the "Opened…"
/// notice), saves what it shows as a PNG and closes. Docs screenshots, with no
/// screen-recording permission needed.
pub fn capture_to_file(ctx: &egui::Context) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static ASKED: AtomicBool = AtomicBool::new(false);
    let Some(path) = std::env::var_os("RAVUNE_CAPTURE_TO") else { return };
    if ctx.input(|i| i.time) > 5.0 && !ASKED.swap(true, Ordering::Relaxed) {
        let done = ctx.clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |image| {
            if let Err(e) = save_png(Path::new(&path), &image) {
                eprintln!("ravune: couldn't save the capture: {e}");
            }
            done.send_viewport_cmd(egui::ViewportCommand::Close);
            done.request_repaint();
        })));
    }
    ctx.request_repaint();
}

fn save_png(path: &Path, image: &egui::ColorImage) -> std::io::Result<()> {
    let mut png = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?), image.size[0] as u32, image.size[1] as u32);
    png.set_color(png::ColorType::Rgba);
    png.set_depth(png::BitDepth::Eight);
    png.write_header().map_err(std::io::Error::other)?.write_image_data(image.as_raw()).map_err(std::io::Error::other)
}

fn data_file(name: &str) -> Option<PathBuf> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(name))
}

pub fn read(path: &Path) -> Result<Project, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    ron::from_str(&text).map_err(|e| tr!("{} is not a valid project: {}", path.display(), e))
}

/// Writes to a temp file first, so a crash mid-save never corrupts the project.
pub fn write(path: &Path, project: &Project) -> Result<(), String> {
    let text = ron::ser::to_string_pretty(project, ron::ser::PrettyConfig::default().compact_arrays(true)).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("ravune-tmp");
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Unsaved work from a session that crashed or closed with changes, if any.
/// Also marks this session as running.
/// Where a crash leaves its report for the next start.
const CRASH_FILE: &str = "crash.txt";

/// Panics write a report (version, system, what happened, where) next to the settings; the
/// next start offers to send it. Nothing leaves the computer unless the user sends it, and
/// their home folder is replaced by `~` so the report doesn't name them.
pub fn install_crash_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let report = format!("Ravune {} on {} {}\n\n{info}\n\n{backtrace}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
        let report = match std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            Ok(home) if !home.is_empty() => report.replace(&home, "~"),
            _ => report,
        };
        if let Some(path) = data_file(CRASH_FILE) {
            let _ = std::fs::write(path, report);
        }
        default(info);
    }));
}

/// The report of a crash last time, if there was one (taken: it shows once).
pub fn take_crash() -> Option<String> {
    let path = data_file(CRASH_FILE)?;
    let report = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(path);
    Some(report)
}

/// A new GitHub issue with the report filled in (cut to what a URL can carry).
pub fn crash_issue_url(report: &str) -> String {
    let encode = |text: &str| -> String {
        text.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    let first = report.lines().nth(2).unwrap_or("a crash").chars().take(80).collect::<String>();
    let body: String = format!("What I was doing:\n\n\n```\n{}\n```", report.chars().take(5000).collect::<String>());
    format!("https://github.com/g1lg1l/ravune/issues/new?title={}&body={}", encode(&format!("Crash: {first}")), encode(&body))
}

pub fn take_recovery() -> Option<Project> {
    let lock = data_file("session.lock")?;
    let recovered = lock.exists().then(|| data_file("autosave.ravune").and_then(|p| read(&p).ok())).flatten();
    let _ = std::fs::write(&lock, b"running");
    recovered
}

impl App {
    pub fn dirty(&self) -> bool {
        self.session.project != self.saved
    }

    pub fn file_name(&self) -> String {
        self.file.as_deref().and_then(Path::file_stem).map_or(tr!("Untitled").into(),|s| s.to_string_lossy().into_owned())
    }

    pub fn load_project(&mut self, project: Project, file: Option<PathBuf>, saved: Project) {
        if let Some(e) = &mut self.engine {
            e.send(ravune_engine::Msg::AllNotesOff);
        }
        self.session = Session::with_project(project);
        self.saved = saved;
        self.file = file;
    }

    /// New/Open: asks first if there are unsaved changes.
    pub fn request(&mut self, action: Pending) {
        if self.dirty() {
            self.pending = Some(action);
        } else {
            self.perform(action);
        }
    }

    fn perform(&mut self, action: Pending) {
        match action {
            Pending::New => self.load_project(Project::default(), None, Project::default()),
            Pending::Bundled(text) => match ron::from_str::<Project>(text) {
                Ok(project) => self.load_project(project.clone(), None, project),
                Err(e) => self.say_as(Tone::Error, tr!("this template is broken: {}", e)),
            },
            Pending::Import(path) => {
                let name = path.file_stem().map_or("Imported".into(), |s| s.to_string_lossy().into_owned());
                let audio = crate::browser::library_dir().with_file_name("Imported").join(&name);
                match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|bytes| crate::dawproject::import(&bytes, &audio)) {
                    Ok(project) => {
                        self.load_project(project, None, Project::default());
                        self.say_as(Tone::Done, tr!("Brought in {}: note tracks became VA-1 channels, audio is in Documents/Ravune/Imported", name));
                    }
                    Err(e) => self.say_as(Tone::Error, tr!("Couldn't bring in {}: {}", name, e)),
                }
            }
            Pending::Restart => self.ext.restart = true,
            Pending::Quit => {
                // saved, or "Don't Save": nothing to bring back next time
                self.saved = self.session.project.clone();
                self.quit = true;
            }
            Pending::Version(path) => match read(&path) {
                // the song's file stays the same, and what's on disk stays what's saved: saving keeps this version
                Ok(project) => {
                    let (file, saved) = (self.file.clone(), self.saved.clone());
                    self.load_project(project, file, saved);
                    self.say_as(Tone::Done, tr!("Back to an earlier version: save to keep it, or open another"));
                }
                Err(e) => self.say_as(Tone::Error, e),
            },
            Pending::Template(path) => match read(&path) {
                Ok(project) => self.load_project(project.clone(), None, project),
                Err(e) => self.say_as(Tone::Error, e),
            },
            Pending::Open(path) => match read(&path) {
                Ok(project) => {
                    self.remember(&path);
                    self.load_project(project.clone(), Some(path), project);
                    self.say_as(Tone::Done, tr!("Opened {}", self.file_name()));
                }
                Err(e) => {
                    // gone or broken: off the recent list
                    self.settings.recent.retain(|r| Path::new(r) != path);
                    self.say_as(Tone::Error, e);
                }
            },
        }
    }

    /// Puts `path` first in File → Open Recent (ten at most).
    fn remember(&mut self, path: &Path) {
        let path = path.to_string_lossy().into_owned();
        self.settings.recent.retain(|r| *r != path);
        self.settings.recent.insert(0, path);
        self.settings.recent.truncate(10);
    }

    /// File → Open: false when nothing was picked.
    pub fn open_dialog(&mut self) -> bool {
        let dialog = rfd::FileDialog::new()
            .add_filter(tr!("Ravune project"), &EXTENSIONS)
            .add_filter(tr!("DAWproject (from another DAW)"), &["dawproject"])
            .add_filter(tr!("MIDI file (into this song)"), &["mid", "midi"]);
        if let Some(path) = dialog.pick_file() {
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi")) {
                self.import_midi(&path);
                return true;
            }
            let daw = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dawproject"));
            self.request(if daw { Pending::Import(path) } else { Pending::Open(path) });
            return true;
        }
        false
    }

    /// File → Export → DAWproject: the song for Bitwig, Studio One, Cubase and others.
    pub fn export_dawproject(&mut self) {
        let name = self.file_name();
        let Some(path) = rfd::FileDialog::new().add_filter("DAWproject", &["dawproject"]).set_file_name(format!("{name}.dawproject")).save_file() else { return };
        let library = &mut self.library;
        match crate::dawproject::export(&self.session.project, &name, |sample| crate::browser::sample_audio(library, sample)).and_then(|zip| std::fs::write(&path, zip).map_err(|e| e.to_string())) {
            Ok(()) => self.say_as(Tone::Done, tr!("Exported {}", path.display())),
            Err(e) => self.say_as(Tone::Error, tr!("Export failed: {}", e)),
        }
    }

    pub fn save(&mut self) -> bool {
        match self.file.clone() {
            Some(path) => self.save_to(path),
            None => self.save_as(),
        }
    }

    pub fn save_as(&mut self) -> bool {
        let dialog = rfd::FileDialog::new().add_filter(tr!("Ravune project"), &[EXTENSION]).set_file_name(format!("{}.{EXTENSION}", self.file_name()));
        match dialog.save_file() {
            Some(path) => self.save_to(path.with_extension(EXTENSION)),
            None => false,
        }
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        self.store_plugin_states();
        match write(&path, &self.session.project) {
            Ok(()) => {
                if let Err(e) = keep_version(&path, &self.session.project) {
                    self.say_as(Tone::Error, tr!("Saved, but couldn't keep a version: {}", e));
                }
                self.saved = self.session.project.clone();
                self.remember(&path);
                self.file = Some(path);
                self.say_as(Tone::Done, tr!("Saved {}", self.file_name()));
                true
            }
            Err(e) => {
                self.say_as(Tone::Error, e);
                false
            }
        }
    }

    /// Every 30 s, snapshot unsaved work for crash recovery (nothing unsaved, nothing to snapshot).
    pub fn autosave(&mut self, now: f64) {
        // while "restore unsaved work?" is unanswered, the old snapshot is what we'd overwrite
        if now - self.last_autosave < AUTOSAVE_SECONDS || self.recovery.is_some() {
            return;
        }
        self.last_autosave = now;
        if self.dirty()
            && self.session.project != self.autosaved
            && let Some(path) = data_file("autosave.ravune")
            && write(&path, &self.session.project).is_ok()
        {
            self.autosaved = self.session.project.clone();
        }
    }

    /// On a clean exit with nothing unsaved, drop the lock; otherwise keep the work for next time.
    pub fn exit_cleanup(&mut self) {
        if self.recovery.is_some() {
            return; // never answered: offer it again next time
        }
        if self.dirty() {
            if let Some(path) = data_file("autosave.ravune") {
                let _ = write(&path, &self.session.project);
            }
        } else {
            for name in ["session.lock", "autosave.ravune"] {
                if let Some(path) = data_file(name) {
                    let _ = std::fs::remove_file(path); // nothing to recover next time
                }
            }
        }
    }

    /// "Save changes?" and "restore unsaved work?" dialogs.
    /// File → Save as Template…: asks for a name (the song's, to start with).
    pub fn ask_template_name(&mut self) {
        self.rename = Some(crate::shell::Renaming::new(crate::shell::Rename::Template, self.file_name()));
    }

    /// Keeps the song as a template called `name`: File → New from Template and the start
    /// page offer it from then on. One of the same name is replaced.
    pub fn save_template(&mut self, name: &str) {
        self.store_plugin_states();
        let name = file_safe(name);
        let path = templates_dir().join(format!("{name}.{EXTENSION}"));
        let replaced = path.exists();
        let saved = std::fs::create_dir_all(templates_dir()).map_err(|e| e.to_string()).and_then(|_| write(&path, &self.session.project));
        match saved {
            Ok(()) if replaced => self.say_as(Tone::Done, tr!("Replaced the template \"{}\"", name)),
            Ok(()) => self.say_as(Tone::Done, tr!("Saved the template \"{}\": it's in New from Template and on the start page", name)),
            Err(e) => self.say_as(Tone::Error, tr!("Couldn't save the template: {}", e)),
        }
    }

    /// Asks before deleting one of your templates (the file goes to nowhere: it's your copy).
    pub fn template_modals(&mut self, ctx: &egui::Context) {
        let Some(path) = self.doomed_template.take() else { return };
        let t = self.theme.clone();
        let name = path.file_stem().map_or(String::new(), |s| s.to_string_lossy().into_owned());
        let body = tr!("It's deleted from the disk for good. Songs you started from it keep everything they have.");
        match kit::confirm(ctx, &t, "delete-template", icons::TRASH, t.meter_hot, &tr!("Delete the template \"{}\"?", name), body, &[tr!("Delete"), tr!("Cancel")]) {
            Some(0) => match std::fs::remove_file(&path) {
                Ok(()) => {
                    self.say_as(Tone::Done, tr!("Deleted the template \"{}\"", name));
                    if let Some(page) = &mut self.start {
                        page.forget_template(&path);
                    }
                }
                Err(e) => self.say_as(Tone::Error, tr!("Couldn't delete it: {}", e)),
            },
            Some(_) => {}
            None => self.doomed_template = Some(path),
        }
    }

    /// File → Versions…
    pub fn show_versions(&mut self) {
        match &self.file {
            Some(path) => self.versions = Some(versions(path)),
            None => self.say_as(Tone::Info, tr!("Save the song first: every save after that is kept as a version")),
        }
    }

    pub fn file_modals(&mut self, ctx: &egui::Context) {
        if let Some(list) = self.versions.take() {
            let t = self.theme.clone();
            let mut open = None;
            let title = tr!("Versions of {}", self.file_name());
            let (_, close) = kit::modal_flush(ctx, &t, "versions", &title, 520.0, |ui| {
                egui::Frame::new().inner_margin(t.pad * 1.5).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(tr!("Every save is kept here. Open one to hear it: saving it makes it the song again, and the versions after it stay.")).color(t.fg_muted)).wrap());
                });
                if list.is_empty() {
                    egui::Frame::new().inner_margin(t.pad * 1.5).show(ui, |ui| ui.label(tr!("No versions yet: they start with the next save")));
                }
                egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (k, v) in list.iter().enumerate() {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 46.0), egui::Sense::hover());
                        let resp = ui.interact(rect, egui::Id::unique(("version", v.at)), egui::Sense::click());
                        let hot = ui.rect_contains_pointer(rect);
                        let painter = ui.painter_at(rect);
                        if hot {
                            painter.rect_filled(rect, 0.0, crate::theme::Theme::mix(t.bg_device, t.fg_text, 0.05));
                        }
                        painter.hline(rect.x_range(), rect.top() + 0.5, egui::Stroke::new(1.0, t.line.gamma_multiply(0.6)));
                        // a dot on a line: the newest one lit
                        let x = rect.left() + t.pad * 2.0;
                        painter.vline(x, rect.y_range(), egui::Stroke::new(1.0, t.line));
                        painter.circle_filled(egui::pos2(x, rect.center().y), 4.5, if k == 0 { t.accent } else { t.fg_muted });
                        let when = if k == 0 { format!("{} · {}", ago(v.at), tr!("latest")) } else { ago(v.at) };
                        painter.text(egui::pos2(x + 16.0, rect.center().y - 1.0), egui::Align2::LEFT_BOTTOM, if v.note.is_empty() { tr!("A save") } else { &v.note }, crate::theme::Theme::body(), t.fg_text);
                        painter.text(egui::pos2(x + 16.0, rect.center().y + 2.0), egui::Align2::LEFT_TOP, when, egui::FontId::proportional(crate::theme::Theme::TEXT_SM), t.fg_muted);
                        if hot {
                            let button = egui::Rect::from_min_max(egui::pos2(rect.right() - 120.0, rect.top()), egui::pos2(rect.right() - t.pad * 1.5, rect.bottom()));
                            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(button).layout(egui::Layout::right_to_left(egui::Align::Center)));
                            if kit::action(&mut child, &t, kit::Variant::Secondary, Some(icons::ARROW_COUNTER_CLOCKWISE), tr!("Open")).clicked() {
                                open = Some(v.path.clone());
                            }
                        }
                        if resp.double_clicked() {
                            open = Some(v.path.clone());
                        }
                    }
                });
            });
            match open {
                Some(path) => self.request(Pending::Version(path)),
                None if !close => self.versions = Some(list),
                None => {}
            }
        }
        let t = self.theme.clone();
        if let Some(action) = self.pending.take() {
            let body = tr!("Your changes will be lost if you don't save them.");
            match kit::confirm(ctx, &t, "unsaved", icons::FLOPPY_DISK, t.accent, &tr!("Save changes to {}?", self.file_name()), body, &[tr!("Save"), tr!("Don't Save"), tr!("Cancel")]) {
                Some(0) if self.save() => self.perform(action),
                Some(1) => self.perform(action),
                Some(2) => {}
                _ => self.pending = Some(action), // still open (or save was cancelled)
            }
        }
        if let Some(report) = self.crash.take() {
            let body = tr!("Sorry about that. A report of what went wrong stayed on this computer: sending it helps fix it. It holds Ravune's version, your system and where the code failed, nothing from your projects.");
            match kit::confirm(ctx, &t, "crashed", icons::BUG, t.meter_hot, tr!("Ravune closed unexpectedly"), body, &[tr!("Report on GitHub"), tr!("Copy the report"), tr!("Close")]) {
                Some(0) => ctx.open_url(egui::OpenUrl::new_tab(crash_issue_url(&report))),
                Some(1) => {
                    ctx.copy_text(report);
                    self.say_as(Tone::Done, tr!("Report copied"));
                }
                Some(_) => {}
                None => self.crash = Some(report),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each save that changes something is a version, newest first, saying what changed;
    /// a version opens as a project.
    #[test]
    fn saves_keep_versions() {
        let dir = std::env::temp_dir().join(format!("ravune-versions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.ravune");
        let mut project: Project = ron::from_str(TEMPLATES[0].1).unwrap();
        keep_version(&path, &project).unwrap();
        keep_version(&path, &project).unwrap();
        assert_eq!(versions(&path).len(), 1, "the same song twice is one version");
        project.tempo = 124.0;
        keep_version(&path, &project).unwrap();
        let list = versions(&path);
        assert_eq!(list.len(), 2);
        assert!(list[0].at > list[1].at);
        assert_eq!(list[0].note, "Tempo 130 → 124");
        assert_eq!(list[1].note, "First save");
        assert_eq!(read(&list[0].path).unwrap(), project);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A template's name becomes a file name that works on every system.
    #[test]
    fn template_names_are_file_safe() {
        assert_eq!(file_safe("  Techno: Dark/Hard?  "), "Techno_ Dark_Hard_");
        assert_eq!(file_safe("Ma chérie"), "Ma chérie");
    }

    #[test]
    fn a_crash_report_becomes_an_issue_link() {
        let url = crash_issue_url("Ravune 0.7.0 on windows x86_64\n\npanicked at src/mixer.rs:12:5:\nindex out of bounds\n\nbacktrace…");
        assert!(url.starts_with("https://github.com/g1lg1l/ravune/issues/new?title=Crash%3A%20panicked%20at%20src%2Fmixer.rs"), "{url}");
        assert!(url.contains("&body=What%20I%20was%20doing") && !url.contains(' ') && !url.contains('\n'));
    }

    #[test]
    fn projects_round_trip_through_ron_files() {
        let dir = std::env::temp_dir().join("ravune-files-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.ravune");
        let mut project = Project::default();
        project.patterns[0].set_step(0, 4, 99);
        project.patterns[0].notes_mut(5).push(ravune_core::Note { start: 0, length: 24, key: 60, velocity: 90, ..Default::default() });
        project.tempo = 93.5;
        write(&path, &project).unwrap();
        assert_eq!(read(&path).unwrap(), project);
        assert!(!dir.join("song.ravune-tmp").exists(), "temp file renamed away");
        std::fs::write(&path, "not ron").unwrap();
        assert!(read(&path).is_err());
    }

    #[test]
    fn templates_load_and_their_sounds_exist() {
        for (name, text) in TEMPLATES {
            let p: Project = ron::from_str(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(p.song_end(), 8 * ravune_core::BAR, "{name}");
            let sounds = p.channels.iter().filter_map(|c| match &c.instrument {
                ravune_core::Instrument::Sampler { sample } => Some(sample.clone()),
                _ => None,
            });
            let loops = p.clips.iter().filter_map(|c| match &c.kind {
                ravune_core::ClipKind::Audio { sample } => Some(sample.clone()),
                _ => None,
            });
            for s in sounds.chain(loops) {
                let file = s.strip_prefix("builtin:").unwrap();
                assert!(crate::browser::BUILTIN.iter().any(|(n, _)| *n == file), "{name}: {file} is not in the library");
            }
        }
    }
}
