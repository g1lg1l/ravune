//! Project files (`.ravune`, RON text), autosave and crash recovery.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use ravune_core::{Project, Session};

use crate::App;
use crate::shell::Tone;

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

    fn load_project(&mut self, project: Project, file: Option<PathBuf>, saved: Project) {
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

    pub fn open_dialog(&mut self) {
        let dialog = rfd::FileDialog::new().add_filter(tr!("Ravune project"), &EXTENSIONS).add_filter(tr!("DAWproject (from another DAW)"), &["dawproject"]);
        if let Some(path) = dialog.pick_file() {
            let daw = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dawproject"));
            self.request(if daw { Pending::Import(path) } else { Pending::Open(path) });
        }
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
    pub fn file_modals(&mut self, ctx: &egui::Context) {
        if let Some(action) = self.pending.take() {
            let mut choice = None;
            egui::Modal::new(egui::Id::unique("unsaved")).show(ctx, |ui| {
                ui.label(RichText::new(tr!("Save changes to {}?", self.file_name())).heading());
                ui.label(tr!("Your changes will be lost if you don't save them."));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    choice = [(tr!("Save"), 0), (tr!("Don't Save"), 1), (tr!("Cancel"), 2)].into_iter().find_map(|(label, n)| ui.button(label).clicked().then_some(n));
                });
            });
            match choice {
                Some(0) if self.save() => self.perform(action),
                Some(1) => self.perform(action),
                Some(2) => {}
                _ => self.pending = Some(action), // still open (or save was cancelled)
            }
        }
        if let Some(report) = self.crash.take() {
            let mut choice = None;
            egui::Modal::new(egui::Id::unique("crashed")).show(ctx, |ui| {
                ui.set_max_width(440.0);
                ui.label(RichText::new(tr!("Ravune closed unexpectedly")).heading());
                ui.label(tr!("Sorry about that. A report of what went wrong stayed on this computer: sending it helps fix it. It holds Ravune's version, your system and where the code failed, nothing from your projects."));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    choice = [(tr!("Report on GitHub"), 0), (tr!("Copy the report"), 1), (tr!("Close"), 2)].into_iter().find_map(|(label, k)| ui.button(label).clicked().then_some(k));
                });
            });
            match choice {
                Some(0) => ctx.open_url(egui::OpenUrl::new_tab(crash_issue_url(&report))),
                Some(1) => {
                    ctx.copy_text(report);
                    self.say_as(Tone::Done, tr!("Report copied"));
                }
                Some(_) => {}
                None => self.crash = Some(report),
            }
            return;
        }
        if let Some(project) = self.recovery.take() {
            let mut choice = None;
            egui::Modal::new(egui::Id::unique("recover")).show(ctx, |ui| {
                ui.label(RichText::new(tr!("Restore unsaved work?")).heading());
                ui.label(tr!("Ravune closed with unsaved changes last time. You can pick up where you left off."));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    choice = [(tr!("Restore"), true), (tr!("Discard"), false)].into_iter().find_map(|(label, b)| ui.button(label).clicked().then_some(b));
                });
            });
            match choice {
                Some(true) => self.load_project(project, None, Project::default()),
                Some(false) => {}
                None => self.recovery = Some(project),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
