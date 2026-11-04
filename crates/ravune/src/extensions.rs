//! Extensions (`.ravunext`) and the update check.
//!
//! An extension is a zip: `extension.json` (name, version, author, about) and any of
//! `themes/*.ravune-theme`, `scripts/*.luau`, `templates/*.ravune` and `sounds/…` (WAV or
//! FLAC, in folders as deep as it likes). It unpacks into Documents/Ravune/Extensions/<name>;
//! its sounds go to Documents/Ravune/Library/<name>, where the browser finds them. The
//! gallery is `gallery/index.json` in Ravune's repository. The gallery and the update check
//! go online only when asked (or, for updates, at start when that is turned on).
//!
//! A copy built by the release workflow updates itself from the release's files: it swaps its
//! binaries (on macOS the whole Ravune.app) for the new ones in place. A running program can
//! be renamed, so the old ones become `*.old`, cleared next start. A copy installed for all
//! users on Windows (Program Files, not writable) runs the new installer instead; builds from
//! source, and copies in other folders Ravune can't write to, get a link to the release page.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use eframe::egui::{self, RichText, Ui};
use serde::Deserialize;

use crate::theme::{THEME_EXTENSIONS, Theme};
use crate::{App, icons, kit};
use crate::shell::Tone;

pub const EXTENSION: &str = "ravunext";
/// Extensions packed while Ravune was still called Phasor install too.
pub const EXTENSIONS: [&str; 2] = [EXTENSION, "phasorext"];
const GALLERY: &str = "https://raw.githubusercontent.com/g1lg1l/ravune/main/gallery/index.json";
/// Ravune's releases, betas included: GitHub's feed of them. Its API would do too, but
/// allows 60 calls an hour to everyone behind one address (an office shares them).
const RELEASES: &str = "https://github.com/g1lg1l/ravune/releases.atom";
/// Where a release's files are, by its tag (the release workflow names them).
const DOWNLOADS: &str = "https://github.com/g1lg1l/ravune/releases/download";
/// What an extension may hold unpacked, and what a download may be.
const MAX_BYTES: u64 = 256 << 20;
/// The folders an extension may have, and the files each takes.
const KINDS: [(&str, &[&str]); 4] = [("themes", &THEME_EXTENSIONS), ("scripts", &["luau", "lua"]), ("templates", &crate::files::EXTENSIONS), ("sounds", &["wav", "flac"])];

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub about: String,
}

/// A gallery entry: what it is and where its package is.
#[derive(Clone, Debug, Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub manifest: Manifest,
    pub url: String,
}

/// Documents/Ravune/Extensions; a separate profile (`RAVUNE_DATA`) keeps its own.
pub fn dir() -> PathBuf {
    match std::env::var_os("RAVUNE_DATA") {
        Some(data) => PathBuf::from(data).join("Extensions"),
        None => crate::browser::library_dir().with_file_name("Extensions"),
    }
}

/// A name that is safe as a folder name.
fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 64 && !name.starts_with('.') && name.chars().all(|c| c.is_alphanumeric() || " -_.&'".contains(c))
}

/// Unpacks a `.ravunext` into `root/<name>` (its sounds into `library/<name>`), replacing
/// an older copy. Paths that would land outside, unknown folders and other files are skipped.
pub fn install(bytes: &[u8], root: &Path, library: &Path) -> Result<Manifest, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| tr!("not an extension: {}", e))?;
    let mut text = String::new();
    zip.by_name("extension.json").map_err(|_| tr!("not an extension: no extension.json"))?.read_to_string(&mut text).map_err(|e| e.to_string())?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(|e| tr!("extension.json: {}", e))?;
    if !valid_name(&manifest.name) {
        return Err(tr!("\"{}\" can't be an extension's name", manifest.name).to_string());
    }
    let (home, sounds) = (root.join(&manifest.name), library.join(&manifest.name));
    remove(&manifest.name, root, library).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&home).and_then(|()| std::fs::write(home.join("extension.json"), &text)).map_err(|e| e.to_string())?;
    let mut total = 0;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(path) = file.enclosed_name().filter(|_| file.is_file()) else { continue };
        let Some((kind, exts)) = KINDS.iter().find(|(kind, _)| path.starts_with(kind)) else { continue };
        if !path.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.contains(&e.to_ascii_lowercase().as_str())) {
            continue;
        }
        let to = if *kind == "sounds" { sounds.join(path.strip_prefix(kind).unwrap()) } else { home.join(&path) };
        total += file.size();
        if total > MAX_BYTES {
            return Err(tr!("too big: an extension holds 256 MB at most").to_string());
        }
        std::fs::create_dir_all(to.parent().unwrap()).map_err(|e| e.to_string())?;
        let mut out = std::fs::File::create(&to).map_err(|e| e.to_string())?;
        std::io::copy(&mut file, &mut out).map_err(|e| e.to_string())?;
    }
    Ok(manifest)
}

/// Takes an extension out: its folder and its sounds.
pub fn remove(name: &str, root: &Path, library: &Path) -> std::io::Result<()> {
    for dir in [root.join(name), library.join(name)] {
        match std::fs::remove_dir_all(dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// The installed extensions, A–Z, with their folders.
pub fn installed() -> Vec<(Manifest, PathBuf)> {
    let mut all: Vec<(Manifest, PathBuf)> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((serde_json::from_str(&std::fs::read_to_string(e.path().join("extension.json")).ok()?).ok()?, e.path())))
        .collect();
    all.sort_by_key(|a| a.0.name.to_lowercase());
    all
}

/// Every installed extension's files of one kind ("scripts", "templates", "themes"), A–Z.
pub fn files(kind: &str) -> Vec<PathBuf> {
    let exts = KINDS.iter().find(|(k, _)| *k == kind).map_or(&[][..], |(_, e)| e);
    installed()
        .into_iter()
        .flat_map(|(_, folder)| {
            let mut found: Vec<PathBuf> = std::fs::read_dir(folder.join(kind)).into_iter().flatten().flatten().map(|e| e.path()).collect();
            found.retain(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.contains(&e)));
            found.sort();
            found
        })
        .collect()
}

fn themes_in(folder: &Path) -> Vec<Theme> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(folder.join("themes")).into_iter().flatten().flatten().map(|e| e.path()).collect();
    paths.sort();
    paths.iter().filter_map(|p| ron::from_str(&std::fs::read_to_string(p).ok()?).ok()).collect()
}

fn get(url: &str) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        // an update is ~35 MB: minutes on a slow line, but a dead one fails fast
        .timeout_global(Some(Duration::from_secs(600)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .user_agent(concat!("Ravune/", env!("CARGO_PKG_VERSION"), " (+https://github.com/g1lg1l/ravune)"))
        .build()
        .into();
    let mut response = agent.get(url).call().map_err(|e| e.to_string())?;
    response.body_mut().with_config().limit(MAX_BYTES).read_to_vec().map_err(|e| e.to_string())
}

/// The programs a release ships, as this OS names them.
fn binaries() -> [String; 3] {
    ["ravune", "ravune-host", "ravune-mcp"].map(|b| format!("{b}{}", std::env::consts::EXE_SUFFIX))
}

/// The name a copy's programs go by: "ravune", or "phasor" for one that updated itself
/// from Phasor 0.8, whose updater only knew those names and kept them.
fn name_of(exe: &Path) -> &'static str {
    if exe.file_stem().is_some_and(|s| s == "phasor") { "phasor" } else { "ravune" }
}

/// A program shipped beside this one (`"host"`, `"mcp"`), named like it.
pub fn sibling(part: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.with_file_name(format!("{}-{part}{}", name_of(&exe), std::env::consts::EXE_SUFFIX))
}

/// What an update replaces: (its name in the release's archive, where it is now). macOS
/// replaces the whole Ravune.app, the others the binaries beside Ravune.
fn replaced(exe: &Path) -> Vec<(String, PathBuf)> {
    if cfg!(target_os = "macos") {
        // …/Ravune.app/Contents/MacOS/ravune
        let app = exe.ancestors().nth(3).filter(|a| a.extension().is_some_and(|e| e == "app"));
        app.map(|a| vec![("Ravune.app".to_string(), a.to_path_buf())]).unwrap_or_default()
    } else {
        binaries().into_iter().map(|b| (b.clone(), exe.with_file_name(b.replacen("ravune", name_of(exe), 1)))).collect()
    }
}

/// Whether Ravune may write into `dir` (not Program Files, a mounted disk image…).
fn writable(dir: &Path) -> bool {
    let probe = dir.join(".ravune-probe");
    std::fs::write(&probe, b"").is_ok() && std::fs::remove_file(&probe).is_ok()
}

/// How this copy updates: the end of the release file's name it takes. None: it doesn't (a
/// build from source, or a folder it can't write to and no installer to fall back on).
fn asset_suffix(exe: &Path) -> Option<&'static str> {
    option_env!("RAVUNE_RELEASE")?;
    let folder = replaced(exe).first()?.1.parent()?.to_path_buf();
    match std::env::consts::OS {
        _ if !writable(&folder) => (cfg!(windows) && exe.with_file_name("unins000.exe").exists()).then_some("-windows-x64-setup.exe"),
        "windows" => Some("-windows-x64.zip"),
        "linux" => Some("-linux-x64.tar.gz"),
        "macos" => Some("-macos-arm64.tar.gz"),
        _ => None,
    }
}

/// The binaries in a release zip, wherever they sit in it, written into `to`.
fn unzip_binaries(bytes: &[u8], to: &Path) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let names = binaries();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(name) = file.enclosed_name().and_then(|p| Some(p.file_name()?.to_string_lossy().into_owned())).filter(|n| names.contains(n)) else { continue };
        let mut out = std::fs::File::create(to.join(name)).map_err(|e| e.to_string())?;
        std::io::copy(&mut file, &mut out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Puts what `staging` holds in place of `replaced`. Each old one is renamed (that works
/// while it runs) to `<name>.<pid>.old`, unique so one still running from an earlier update
/// (ravune-mcp under an agent) can't block it, and comes back if its new one can't go in.
fn swap(staging: &Path, replaced: &[(String, PathBuf)]) -> std::io::Result<()> {
    if !replaced.first().is_some_and(|(name, _)| staging.join(name).exists()) {
        return Err(std::io::Error::other(tr!("the download has no Ravune in it")));
    }
    for (name, to) in replaced {
        let new = staging.join(name);
        if !new.exists() {
            continue;
        }
        let old = to.with_file_name(format!("{}.{}.old", to.file_name().unwrap().to_string_lossy(), std::process::id()));
        if to.exists() {
            std::fs::rename(to, &old)?;
        }
        if let Err(e) = std::fs::rename(&new, to) {
            let _ = std::fs::rename(&old, to);
            return Err(e);
        }
    }
    Ok(())
}

/// Clears what earlier updates left: the old copies (those no longer running) and an
/// unpacked download.
fn tidy(replaced: &[(String, PathBuf)]) {
    let Some(folder) = replaced.first().and_then(|(_, p)| p.parent()) else { return };
    let names: Vec<String> = replaced.iter().filter_map(|(_, p)| Some(format!("{}.", p.file_name()?.to_string_lossy()))).collect();
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".old") && names.iter().any(|n| name.starts_with(n)) {
            let _ = std::fs::remove_file(entry.path()).or_else(|_| std::fs::remove_dir_all(entry.path()));
        }
    }
    let _ = std::fs::remove_dir_all(folder.join(".ravune-update"));
}

/// An update, downloaded and ready.
struct Staged {
    /// Where Ravune is (found before the swap: on Linux the running one reads "(deleted)" after).
    exe: PathBuf,
    /// A copy installed for all users (Program Files): the new installer, run when Ravune
    /// restarts. None: the update is already in place.
    setup: Option<PathBuf>,
}

/// Downloads `url` (a file of the release) and puts it in place of the Ravune at `exe`.
fn stage(url: &str, exe: PathBuf) -> Result<Staged, String> {
    let bytes = get(url)?;
    if url.ends_with(".exe") {
        let setup = std::env::temp_dir().join("ravune-setup.exe");
        std::fs::write(&setup, bytes).map_err(|e| e.to_string())?;
        return Ok(Staged { exe, setup: Some(setup) });
    }
    let replaced = replaced(&exe);
    // unpacked beside what it replaces, not in the temp folder: a rename can't cross drives
    let staging = replaced.first().ok_or("Ravune isn't where an update can find it")?.1.with_file_name(".ravune-update");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    if url.ends_with(".zip") {
        unzip_binaries(&bytes, &staging)?;
    } else {
        // tar keeps the Unix permissions and the Mac bundle; every Linux and macOS has it
        let tarball = staging.join("update.tar.gz");
        std::fs::write(&tarball, bytes).map_err(|e| e.to_string())?;
        let ok = std::process::Command::new("tar").arg("-xzf").arg(&tarball).arg("-C").arg(&staging).arg("--strip-components=1").status().is_ok_and(|s| s.success());
        if !ok {
            return Err(tr!("couldn't unpack the download").to_string());
        }
    }
    let swapped = swap(&staging, &replaced).map_err(|e| e.to_string());
    let _ = std::fs::remove_dir_all(&staging);
    swapped.map(|()| Staged { exe, setup: None })
}

/// Whether release tag `tag` ("v0.8.0") is newer than `current` ("0.7.0").
/// A beta ("0.8.1-beta") comes before its release ("0.8.1").
fn newer(tag: &str, current: &str) -> bool {
    let parse = |v: &str| {
        let v = v.trim_start_matches('v');
        let (numbers, beta) = v.split_once('-').map_or((v, false), |(n, _)| (n, true));
        (numbers.split('.').map(|n| n.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>(), !beta)
    };
    parse(tag) > parse(current)
}

struct Release {
    tag: String,
    page: String,
    /// The file this copy updates from (see [`asset_suffix`]).
    download: Option<String>,
}

/// The newest release in the feed (by its version, whatever the feed's order). `suffix`:
/// the file this copy updates from, `ravune-<version><suffix>` in that release.
fn latest(body: &[u8], suffix: Option<&str>) -> Result<Release, String> {
    let text = std::str::from_utf8(body).map_err(|e| e.to_string())?;
    let feed = roxmltree::Document::parse(text).map_err(|e| e.to_string())?;
    let pages = feed.descendants().filter(|n| n.has_tag_name("entry")).filter_map(|e| e.children().find(|c| c.has_tag_name("link"))?.attribute("href"));
    let (tag, page) = pages
        .filter_map(|page| Some((page.rsplit_once("/releases/tag/")?.1.to_string(), page.to_string())))
        .reduce(|a, b| if newer(&b.0, &a.0) { b } else { a })
        .ok_or("no releases yet")?;
    let download = suffix.map(|s| format!("{DOWNLOADS}/{tag}/ravune-{}{s}", tag.trim_start_matches('v')));
    Ok(Release { tag, page, download })
}

enum Reply {
    Gallery(Result<Vec<Listing>, String>),
    Package(String, Result<Vec<u8>, String>),
    Release(Result<Release, String>),
    Update(Result<Staged, String>),
}

/// What the Extensions tab and the update check are waiting for and have heard.
#[derive(Default)]
pub struct Online {
    waiting: Vec<Receiver<Reply>>,
    pub gallery: Option<Result<Vec<Listing>, String>>,
    /// Packages on their way, by name.
    downloading: Vec<String>,
    /// The latest release, or why it couldn't be read.
    release: Option<Result<Release, String>>,
    checking: bool,
    /// The new version on its way (None), ready, or why it isn't.
    update: Option<Option<Result<Staged, String>>>,
    /// Restart into the update: asked for, Ravune closes and [`App::finish_update`] opens it.
    pub restart: bool,
    started: bool,
}

impl Online {
    fn spawn(&mut self, ctx: &egui::Context, job: impl FnOnce() -> Reply + Send + 'static) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
        self.waiting.push(rx);
    }
}

impl App {
    /// Hands over what came back from GitHub; at the first frame, the update check if it's on.
    pub fn poll_extensions(&mut self, ctx: &egui::Context) {
        if !std::mem::replace(&mut self.ext.started, true) {
            if let Ok(exe) = std::env::current_exe() {
                tidy(&replaced(&exe));
            }
            if self.settings.check_updates {
                self.check_updates(ctx);
            }
        }
        if self.ext.restart {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let mut replies = Vec::new();
        self.ext.waiting.retain(|rx| match rx.try_recv() {
            Ok(reply) => {
                replies.push(reply);
                false
            }
            Err(e) => e == std::sync::mpsc::TryRecvError::Empty,
        });
        for reply in replies {
            match reply {
                Reply::Gallery(list) => self.ext.gallery = Some(list),
                Reply::Package(name, bytes) => {
                    self.ext.downloading.retain(|n| *n != name);
                    match bytes {
                        Ok(bytes) => self.install_extension(ctx, &bytes),
                        Err(e) => self.say_as(Tone::Error, tr!("Couldn't download {}: {}", name, e)),
                    }
                }
                Reply::Release(release) => {
                    self.ext.checking = false;
                    if let Ok(r) = &release
                        && newer(&r.tag, env!("CARGO_PKG_VERSION"))
                        && self.ext.update.is_none()
                    {
                        self.say_as(Tone::Action, tr!("Ravune {} is available: Preferences → Extensions", r.tag));
                    }
                    self.ext.release = Some(release);
                }
                Reply::Update(staged) => {
                    match &staged {
                        Ok(_) => self.say_as(Tone::Action, tr!("The update is ready: restart Ravune (Preferences → Extensions)")),
                        Err(e) => self.say_as(Tone::Error, tr!("Couldn't update: {}", e)),
                    }
                    self.ext.update = Some(Some(staged));
                }
            }
        }
    }

    /// Asks GitHub for the latest release.
    pub fn check_updates(&mut self, ctx: &egui::Context) {
        self.ext.checking = true;
        self.ext.spawn(ctx, || {
            let suffix = std::env::current_exe().ok().and_then(|exe| asset_suffix(&exe));
            Reply::Release(get(RELEASES).and_then(|bytes| latest(&bytes, suffix)))
        });
    }

    /// At exit, after a restart was asked for: opens the new Ravune, or for a copy installed
    /// for all users runs the installer first (cmd waits for it while it asks for admin rights).
    pub fn finish_update(&self) {
        let Some(Some(Ok(staged))) = &self.ext.update else { return };
        if !self.ext.restart {
            return;
        }
        let Some(setup) = &staged.setup else {
            let _ = std::process::Command::new(&staged.exe).args(&self.file).spawn();
            return;
        };
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let open = self.file.as_ref().map_or(String::new(), |f| format!(" \"{}\"", f.display()));
            let line = format!("/c start \"\" /wait \"{}\" /SILENT /SUPPRESSMSGBOXES /NORESTART /ALLUSERS && start \"\" \"{}\"{open}", setup.display(), staged.exe.display());
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let _ = std::process::Command::new("cmd").raw_arg(line).creation_flags(CREATE_NO_WINDOW).spawn();
        }
        #[cfg(not(windows))]
        let _ = setup;
    }

    /// File → Install Extension…
    pub fn install_extension_file(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new().add_filter(tr!("Ravune extension"), &EXTENSIONS).pick_file() else { return };
        match std::fs::read(&path) {
            Ok(bytes) => self.install_extension(ctx, &bytes),
            Err(e) => self.say_as(Tone::Error, tr!("Can't read {}: {}", path.display(), e)),
        }
    }

    fn install_extension(&mut self, ctx: &egui::Context, bytes: &[u8]) {
        match install(bytes, &dir(), &crate::browser::library_dir()) {
            Ok(m) => {
                let themes = themes_in(&dir().join(&m.name));
                for theme in &themes {
                    self.settings.themes.retain(|t| t.name != theme.name);
                    self.settings.themes.push(theme.clone());
                }
                self.library.refresh();
                self.say_as(Tone::Done, tr!("Installed {} {}", m.name, m.version));
                if let Some(theme) = themes.into_iter().next() {
                    self.use_theme(ctx, theme);
                }
            }
            Err(e) => self.say_as(Tone::Error, tr!("Couldn't install: {}", e)),
        }
    }

    fn remove_extension(&mut self, name: &str, folder: &Path) {
        let names: Vec<String> = themes_in(folder).into_iter().map(|t| t.name).collect();
        match remove(name, &dir(), &crate::browser::library_dir()) {
            Ok(()) => {
                self.settings.themes.retain(|t| !names.contains(&t.name));
                self.library.refresh();
                self.say_as(Tone::Done, tr!("Removed {}", name));
            }
            Err(e) => self.say_as(Tone::Error, tr!("Couldn't remove {}: {}", name, e)),
        }
    }

    /// Preferences → Extensions: what's installed, the gallery, updates.
    pub fn prefs_extensions(&mut self, ui: &mut Ui, t: &Theme) {
        let installed = installed();
        kit::caps(ui, t, tr!("INSTALLED"));
        if installed.is_empty() {
            ui.label(RichText::new(tr!("Themes, scripts, sounds and templates others made. Install one from the gallery below or from a .ravunext file.")).small().color(t.fg_muted));
        }
        let mut drop = None;
        for (m, folder) in &installed {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}  {}", icons::PUZZLE_PIECE, m.name)).strong());
                ui.label(RichText::new(format!("{} · {}", m.version, m.author)).small().color(t.fg_muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if kit::icon_button(ui, t, icons::TRASH, tr!("Remove"), false).clicked() {
                        drop = Some((m.name.clone(), folder.clone()));
                    }
                });
            });
            if !m.about.is_empty() {
                ui.label(RichText::new(&m.about).small().color(t.fg_muted));
            }
        }
        if let Some((name, folder)) = drop {
            self.remove_extension(&name, &folder);
        }
        if kit::button(ui, t, Some(icons::FOLDER_OPEN), tr!("Install from a file…"), false).clicked() {
            self.install_extension_file(ui.ctx());
        }

        kit::section(ui, t, tr!("GALLERY"), false, |_| {});
        match &self.ext.gallery {
            None => {
                ui.label(RichText::new(tr!("The gallery lives on GitHub: Ravune goes online only when you ask.")).small().color(t.fg_muted));
            }
            Some(Err(e)) => {
                ui.label(RichText::new(tr!("Couldn't reach the gallery: {}", e)).small().color(t.meter_hot));
            }
            Some(Ok(list)) => {
                let mut get_it = None;
                for l in list {
                    let have = installed.iter().find(|(m, _)| m.name == l.manifest.name).map(|(m, _)| m.version.clone());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}  {}", icons::PUZZLE_PIECE, l.manifest.name)).strong());
                        ui.label(RichText::new(format!("{} · {}", l.manifest.version, l.manifest.author)).small().color(t.fg_muted));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if self.ext.downloading.contains(&l.manifest.name) {
                                ui.spinner();
                            } else if have.as_deref() == Some(l.manifest.version.as_str()) {
                                ui.label(RichText::new(tr!("Installed")).small().color(t.select));
                            } else {
                                let label = if have.is_some() { tr!("Update") } else { tr!("Install") };
                                if kit::button(ui, t, Some(icons::DOWNLOAD_SIMPLE), label, false).clicked() {
                                    get_it = Some(l.clone());
                                }
                            }
                        });
                    });
                    ui.label(RichText::new(&l.manifest.about).small().color(t.fg_muted));
                }
                if let Some(l) = get_it {
                    self.ext.downloading.push(l.manifest.name.clone());
                    self.ext.spawn(ui.ctx(), move || Reply::Package(l.manifest.name, get(&l.url)));
                }
            }
        }
        let label = if self.ext.gallery.is_some() { tr!("Refresh") } else { tr!("Browse the gallery") };
        if kit::button(ui, t, Some(icons::GLOBE), label, false).clicked() {
            self.ext.spawn(ui.ctx(), || Reply::Gallery(get(GALLERY).and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))));
        }

        kit::section(ui, t, tr!("UPDATES"), false, |_| {});
        kit::setting(ui, t, tr!("Check when Ravune starts"), "", |ui| kit::switch(ui, t, &mut self.settings.check_updates, tr!("Check when Ravune starts")));
        ui.horizontal(|ui| {
            if kit::button(ui, t, Some(icons::ARROW_CLOCKWISE), tr!("Check now"), false).clicked() && !self.ext.checking {
                self.check_updates(ui.ctx());
            }
            if self.ext.checking {
                ui.spinner();
            }
        });
        let (mut restart, mut download) = (false, None);
        match (&self.ext.release, &self.ext.update) {
            (Some(Ok(r)), Some(None)) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(tr!("Downloading Ravune {}…", r.tag)).small().color(t.fg_muted));
                });
            }
            (Some(Ok(r)), Some(Some(Ok(staged)))) => {
                let what = if staged.setup.is_some() { tr!("Ravune {} is downloaded: it installs when Ravune restarts", r.tag) } else { tr!("Ravune {} is installed: it opens next time", r.tag) };
                ui.label(RichText::new(what).small().color(t.select));
                restart = kit::button(ui, t, Some(icons::ARROW_CLOCKWISE), tr!("Restart now"), false).clicked();
            }
            (Some(Ok(r)), Some(Some(Err(e)))) => {
                ui.label(RichText::new(tr!("Couldn't update: {}", e)).small().color(t.meter_hot));
                ui.hyperlink_to(format!("{}  {}", icons::ARROW_SQUARE_OUT, tr!("Ravune {} is available: get it on GitHub", r.tag)), &r.page);
            }
            (Some(Ok(r)), None) if newer(&r.tag, env!("CARGO_PKG_VERSION")) => match &r.download {
                Some(url) => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(tr!("Ravune {} is available", r.tag)).small().color(t.select));
                        if kit::button(ui, t, Some(icons::DOWNLOAD_SIMPLE), tr!("Update"), false).clicked() {
                            download = Some(url.clone());
                        }
                        ui.hyperlink_to(RichText::new(tr!("What's new")).small(), &r.page);
                    });
                }
                None => {
                    ui.hyperlink_to(format!("{}  {}", icons::ARROW_SQUARE_OUT, tr!("Ravune {} is available: get it on GitHub", r.tag)), &r.page);
                }
            },
            (Some(Ok(_)), _) => {
                ui.label(RichText::new(tr!("Ravune {} is the latest", env!("CARGO_PKG_VERSION"))).small().color(t.fg_muted));
            }
            (Some(Err(e)), _) => {
                ui.label(RichText::new(tr!("Couldn't check: {}", e)).small().color(t.meter_hot));
            }
            (None, _) => {
                ui.label(RichText::new(tr!("Ravune {} · asks GitHub for the latest release, nothing else is sent", env!("CARGO_PKG_VERSION"))).small().color(t.fg_muted));
            }
        }
        if restart {
            self.request(crate::files::Pending::Restart);
        }
        // one download per run: a second swap would trip over the first's *.old
        if let Some(url) = download {
            self.ext.update = Some(None);
            self.ext.spawn(ui.ctx(), move || Reply::Update(std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| stage(&url, exe))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut zip, bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn installs_what_it_should_and_nothing_else() {
        let root = std::env::temp_dir().join("ravune-ext-test");
        let _ = std::fs::remove_dir_all(&root);
        let (home, library) = (root.join("Extensions"), root.join("Library"));
        let manifest = br#"{"name": "Night Pack", "version": "1.0.0", "author": "someone"}"#;
        let bytes = package(&[
            ("extension.json", manifest),
            ("scripts/ghost.luau", b"-- ghosts"),
            ("sounds/Hats/hat.wav", b"RIFF"),
            ("../evil.luau", b"x"),
            ("scripts/../../evil.luau", b"x"),
            ("bin/run.exe", b"MZ"),
            ("scripts/run.exe", b"MZ"),
        ]);
        let m = install(&bytes, &home, &library).unwrap();
        assert_eq!((m.name.as_str(), m.version.as_str()), ("Night Pack", "1.0.0"));
        assert!(home.join("Night Pack/scripts/ghost.luau").exists());
        assert!(library.join("Night Pack/Hats/hat.wav").exists(), "sounds go to the library");
        assert!(!home.join("Night Pack/bin").exists() && !home.join("Night Pack/scripts/run.exe").exists());
        assert!(!root.join("evil.luau").exists() && !home.join("evil.luau").exists());
        remove("Night Pack", &home, &library).unwrap();
        assert!(!home.join("Night Pack").exists() && !library.join("Night Pack").exists());
        let bad = package(&[("extension.json", br#"{"name": "../up", "version": "1"}"#)]);
        assert!(install(&bad, &home, &library).is_err());
        assert!(install(b"not a zip", &home, &library).is_err());
    }

    #[test]
    fn the_gallery_packages_install() {
        let index: Vec<Listing> = serde_json::from_str(include_str!("../../../gallery/index.json")).unwrap();
        assert!(!index.is_empty());
        let root = std::env::temp_dir().join("ravune-gallery-test");
        let _ = std::fs::remove_dir_all(&root);
        for l in &index {
            let file = l.url.rsplit('/').next().unwrap();
            let bytes = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../gallery").join(file)).unwrap();
            let m = install(&bytes, &root, &root.join("Library")).unwrap();
            assert_eq!(m, l.manifest, "{file}: the index says what the package says");
            for theme in std::fs::read_dir(root.join(&m.name).join("themes")).into_iter().flatten().flatten() {
                ron::from_str::<Theme>(&std::fs::read_to_string(theme.path()).unwrap()).unwrap();
            }
            for template in std::fs::read_dir(root.join(&m.name).join("templates")).into_iter().flatten().flatten() {
                ron::from_str::<ravune_core::Project>(&std::fs::read_to_string(template.path()).unwrap()).unwrap();
            }
            for script in std::fs::read_dir(root.join(&m.name).join("scripts")).into_iter().flatten().flatten() {
                let mut app = App::for_tests();
                app.session.selected = 3; // the closed hat, on the beat
                for step in [0, 4, 8, 12] {
                    app.session.run("step.set", serde_json::json!({ "channel": 3, "step": step, "velocity": 100 })).unwrap();
                }
                let before = app.session.project.clone();
                let script = crate::script::Script::from_file(script.path());
                app.run_script(&script);
                assert!(app.toast.as_ref().is_some_and(|t| t.text.starts_with("Ran")), "{}: {:?}", script.name, app.toast);
                assert_ne!(app.session.project, before, "{} changed nothing", script.name);
            }
        }
    }

    #[test]
    fn versions_compare() {
        assert!(newer("v0.8.0", "0.7.0") && newer("v1.0.0", "0.12.3") && newer("0.7.1", "0.7.0"));
        assert!(!newer("v0.7.0", "0.7.0") && !newer("v0.6.9", "0.7.0") && !newer("", "0.7.0"));
        assert!(newer("v0.8.1", "0.8.1-beta") && newer("v0.8.2-beta", "0.8.1") && !newer("v0.8.1-beta", "0.8.1-beta") && !newer("v0.8.1-beta", "0.8.1"));
        // GitHub's feed, trimmed: an older release listed first, as an edit can put it there
        let entry = |tag: &str| format!(r#"<entry><id>tag:github.com,2008:Repository/1/{tag}</id><link rel="alternate" type="text/html" href="https://github.com/g1lg1l/ravune/releases/tag/{tag}"/><title>{tag}</title></entry>"#);
        let feed = |entries: &str| format!(r#"<?xml version="1.0" encoding="UTF-8"?><feed xmlns="http://www.w3.org/2005/Atom"><title>Release notes from ravune</title>{entries}</feed>"#);
        let body = feed(&(entry("v0.8.2") + &entry("v0.8.3-beta") + &entry("v0.8.1-beta")));
        let r = latest(body.as_bytes(), Some("-windows-x64.zip")).unwrap();
        assert_eq!((r.tag.as_str(), r.page.as_str()), ("v0.8.3-beta", "https://github.com/g1lg1l/ravune/releases/tag/v0.8.3-beta"));
        assert_eq!(r.download.as_deref(), Some("https://github.com/g1lg1l/ravune/releases/download/v0.8.3-beta/ravune-0.8.3-beta-windows-x64.zip"));
        assert!(latest(body.as_bytes(), None).unwrap().download.is_none());
        assert_eq!(latest(feed("").as_bytes(), None).err().as_deref(), Some("no releases yet"));
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn a_copy_still_named_phasor_updates_under_that_name() {
        let named = |b: &str| Path::new("app").join(format!("{b}{}", std::env::consts::EXE_SUFFIX));
        let [ravune, host, _] = binaries();
        let r = replaced(&named("phasor"));
        assert_eq!((&r[0], &r[1]), (&(ravune.clone(), named("phasor")), &(host.clone(), named("phasor-host"))));
        assert_eq!(replaced(&named("ravune"))[1], (host, named("ravune-host")));
    }

    #[test]
    fn an_update_swaps_the_binaries() {
        let root = std::env::temp_dir().join("ravune-update-test");
        let _ = std::fs::remove_dir_all(&root);
        let staging = root.join(".ravune-update");
        std::fs::create_dir_all(&staging).unwrap();
        let [ravune, host, _] = binaries();
        let replaced: Vec<(String, PathBuf)> = binaries().into_iter().map(|b| (b.clone(), root.join(b))).collect();
        std::fs::write(root.join(&ravune), "old").unwrap();
        std::fs::write(root.join(&host), "old host").unwrap();
        assert!(swap(&staging, &replaced).is_err(), "nothing unpacked: nothing swapped");
        let zip = package(&[(&format!("ravune-9.0.0-windows-x64/{ravune}"), b"new"), ("ravune-9.0.0-windows-x64/README.md", b"#"), (&format!("../{host}"), b"evil")]);
        unzip_binaries(&zip, &staging).unwrap();
        assert!(!staging.join("README.md").exists() && !staging.join(&host).exists(), "only binaries, only from inside the zip");
        swap(&staging, &replaced).unwrap();
        let read = |name: &str| std::fs::read_to_string(root.join(name)).unwrap();
        let old = format!("{ravune}.{}.old", std::process::id());
        assert_eq!((read(&ravune).as_str(), read(&old).as_str()), ("new", "old"));
        assert_eq!(read(&host), "old host", "a binary the update lacks stays");
        tidy(&replaced);
        assert!(!root.join(&old).exists() && !staging.exists() && root.join(&ravune).exists());
    }

    /// The real feed: its newest release, and the file it names is there to download.
    /// `cargo test -p ravune real_feed -- --ignored`
    #[test]
    #[ignore]
    fn real_feed() {
        let r = latest(&get(RELEASES).unwrap(), Some("-windows-x64.zip")).unwrap();
        assert!(r.tag.starts_with('v') && r.page.ends_with(&r.tag), "{} {}", r.tag, r.page);
        assert!(get(r.download.as_deref().unwrap()).unwrap().len() > 1 << 20, "{:?}", r.download);
    }

    /// The real thing: downloads a published release's zip and swaps it into a folder.
    /// `cargo test -p ravune real_update -- --ignored`
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn real_update() {
        let root = std::env::temp_dir().join("ravune-real-update");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("ravune.exe"), "old").unwrap();
        let url = "https://github.com/g1lg1l/ravune/releases/download/v0.8.1-beta/ravune-0.8.1-beta-windows-x64.zip";
        let staged = stage(url, root.join("ravune.exe")).unwrap();
        assert!(staged.setup.is_none());
        for b in binaries() {
            assert!(std::fs::metadata(root.join(&b)).unwrap().len() > 100_000, "{b} is in place");
        }
        assert_eq!(std::fs::read_to_string(root.join(format!("ravune.exe.{}.old", std::process::id()))).unwrap(), "old");
    }
}
