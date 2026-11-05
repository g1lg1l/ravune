//! Online sounds (M6): search free sounds on Openverse, Wikimedia Commons and ccMixter (no
//! account), Freesound (free API key) or a source of your own (a JSON search API) from the
//! browser, preview them, and keep the ones you use in "Online Sounds" with their license and
//! author (`credits.json` beside them, "Export credits" per project).
//! Network work runs on one background thread; the UI polls its replies.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::io::Read;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, Align2, FontId, Id, Key, Rect, RichText, Ui, pos2, vec2};
use ravune_core::Instrument;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::browser::SampleDrag;
use crate::theme::Theme;
use crate::{App, icons, kit, widgets};
use crate::shell::Tone;

const PAGE: usize = 20;
/// Downloads bigger than this are refused (a sample, not an album).
const MAX_BYTES: u64 = 64 << 20;
const CREDITS: &str = "credits.json";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    #[default]
    Openverse,
    Freesound,
    Commons,
    CcMixter,
    /// Your own: [`Custom`].
    Custom,
}

impl Provider {
    const ALL: [(Provider, &'static str); 5] = [
        (Provider::Openverse, "Openverse: millions of CC sounds gathered from many sites, no account"),
        (Provider::Freesound, "Freesound: the biggest CC sound library (a free API key)"),
        (Provider::Commons, "Wikimedia Commons: recordings of instruments, voices, places, no account"),
        (Provider::CcMixter, "ccMixter: stems, a cappellas and loops made for remixing, no account"),
        (Provider::Custom, "Your own source: any search API that answers in JSON"),
    ];

    fn name(self) -> &'static str {
        match self {
            Provider::Openverse => "Openverse",
            Provider::Freesound => "Freesound",
            Provider::Commons => "Wikimedia Commons",
            Provider::CcMixter => "ccMixter",
            Provider::Custom => "Custom",
        }
    }

    /// In file and cache names.
    fn slug(self) -> &'static str {
        match self {
            Provider::Openverse => "openverse",
            Provider::Freesound => "freesound",
            Provider::Commons => "commons",
            Provider::CcMixter => "ccmixter",
            Provider::Custom => "custom",
        }
    }
}

/// A sound found online.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub provider: Provider,
    pub id: String,
    pub title: String,
    pub author: String,
    /// "CC0", "PD", "BY", "BY-SA", "BY-NC", "BY-NC-SA", with the version after a space.
    pub license: String,
    pub license_url: String,
    /// The sound's page on the web.
    pub page: String,
    /// The audio file to download.
    pub audio: String,
    pub ext: String,
    /// 0 when unknown.
    pub seconds: f32,
    pub tags: Vec<String>,
}

impl Hit {
    fn key(&self) -> String {
        format!("{}-{}", self.provider.slug(), self.id)
    }

    /// One line of the credits file.
    pub fn credit(&self) -> String {
        tr!("\"{}\" by {} · {} {} · {} (via {})", self.title, self.author, license_label(&self.license), self.license_url, self.page, crate::i18n::t(self.provider.name()))
    }

    /// The title without a file extension.
    fn name(&self) -> &str {
        self.title.rsplit_once('.').filter(|(_, ext)| ext.len() <= 4).map_or(self.title.as_str(), |(stem, _)| stem)
    }

    fn file_name(&self) -> String {
        let id: String = self.id.chars().take(8).collect(); // Openverse ids are long UUIDs
        let name: String = format!("{} - {} ({} {id})", self.name(), self.author, self.provider.name())
            .chars()
            .map(|c| if c.is_alphanumeric() || " -_().,'&".contains(c) { c } else { '_' })
            .collect();
        format!("{}.{}", name.chars().take(120).collect::<String>().trim(), self.ext)
    }
}

/// "BY-NC 4.0" → "CC BY-NC 4.0"; CC0 and public-domain marks as they are.
fn license_label(license: &str) -> String {
    if license.starts_with("CC0") || license.starts_with("PD") { license.to_string() } else { format!("CC {license}") }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Length {
    #[default]
    Any,
    /// Up to 2 s: drums, hits.
    OneShot,
    /// Up to 10 s: loops, phrases.
    Short,
    /// Longer: textures, recordings.
    Long,
}

impl Length {
    const ALL: [(Length, &'static str); 4] = [(Length::Any, "Any length"), (Length::OneShot, "One-shots"), (Length::Short, "Under 10 s"), (Length::Long, "Longer")];

    fn fits(self, seconds: f32) -> bool {
        seconds <= 0.0
            || match self {
                Length::Any => true,
                Length::OneShot => seconds <= 2.0,
                Length::Short => seconds <= 10.0,
                Length::Long => seconds > 10.0,
            }
    }
}

/// What the Online tab remembers (Settings).
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub provider: Provider,
    pub length: Length,
    /// CC BY-SA too (fine to use; derivatives must keep the license).
    pub share_alike: bool,
    /// NC licenses too: never in anything you sell.
    pub non_commercial: bool,
    pub freesound_key: String,
    /// Where kept sounds go; empty = Documents/Ravune/Online Sounds.
    pub folder: String,
    pub custom: Custom,
}

/// A search API of your own: the URL (with `{query}`, `{page}` and `{key}` in it) and where
/// each thing is in its JSON answer (JSON pointers like `/results`, inside each result
/// `/title`). Out of the box it searches Openverse's CC0 sounds, as an example.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Custom {
    pub url: String,
    pub key: String,
    /// The list of results; empty = the answer is the list.
    pub results: String,
    /// How many there are in all; empty = unknown.
    pub total: String,
    pub title: String,
    pub audio: String,
    pub author: String,
    /// A CC license URL or a short name (cc0, by, by-sa…).
    pub license: String,
    pub page: String,
    pub seconds: String,
}

impl Default for Custom {
    fn default() -> Self {
        let s = |s: &str| s.to_string();
        Self {
            url: s("https://api.openverse.org/v1/audio/?q={query}&page={page}&license=cc0"),
            key: String::new(),
            results: s("/results"),
            total: s("/result_count"),
            title: s("/title"),
            audio: s("/url"),
            author: s("/creator"),
            license: s("/license_url"),
            page: s("/foreign_landing_url"),
            seconds: String::new(),
        }
    }
}

impl Prefs {
    pub fn folder(&self) -> PathBuf {
        if !self.folder.is_empty() {
            return PathBuf::from(&self.folder);
        }
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default();
        home.join("Documents").join("Ravune").join("Online Sounds")
    }
}

// ---------------------------------------------------------------------------
// Parsing (separate from the network, so it's testable)
// ---------------------------------------------------------------------------

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn openverse_hits(body: &Value) -> Vec<Hit> {
    let Some(results) = body["results"].as_array() else { return Vec::new() };
    results
        .iter()
        .filter_map(|r| {
            let audio = text(&r["url"]);
            let ext = r["filetype"].as_str().unwrap_or_else(|| audio.rsplit('.').next().unwrap_or("mp3")).to_lowercase();
            let license = match r["license"].as_str()? {
                "cc0" => "CC0".to_string(),
                "pdm" => "PD".to_string(),
                other => other.to_uppercase(),
            };
            Some(Hit {
                provider: Provider::Openverse,
                id: text(&r["id"]),
                title: text(&r["title"]),
                author: text(&r["creator"]),
                license: format!("{license} {}", text(&r["license_version"])).trim().to_string(),
                license_url: text(&r["license_url"]),
                page: text(&r["foreign_landing_url"]),
                ext,
                seconds: r["duration"].as_f64().unwrap_or(0.0) as f32 / 1000.0,
                tags: r["tags"].as_array().map(|t| t.iter().filter_map(|t| t["name"].as_str().map(String::from)).collect()).unwrap_or_default(),
                audio,
            })
            .filter(|h| !h.audio.is_empty())
        })
        .collect()
}

/// Freesound gives licenses as URLs.
fn freesound_license(url: &str) -> String {
    let u = url.to_lowercase();
    let version = u.trim_end_matches('/').rsplit('/').next().filter(|v| v.contains('.')).unwrap_or("").to_string();
    let kind = if u.contains("publicdomain/zero") {
        "CC0"
    } else if u.contains("/by-nc-sa/") {
        "BY-NC-SA"
    } else if u.contains("/by-nc/") {
        "BY-NC"
    } else if u.contains("/by-sa/") {
        "BY-SA"
    } else if u.contains("/by/") {
        "BY"
    } else if u.contains("sampling+") {
        "Sampling+"
    } else {
        "other"
    };
    format!("{kind} {version}").trim().to_string()
}

/// What a license is, from a CC URL or a short name.
fn license_from(text: &str) -> String {
    let t = text.trim();
    if t.contains("creativecommons.org") {
        return freesound_license(t);
    }
    match t.to_lowercase().as_str() {
        "cc0" => "CC0".into(),
        "pdm" | "pd" | "public domain" => "PD".into(),
        "" => "unknown".into(),
        other => other.trim_start_matches("cc ").to_uppercase(),
    }
}

/// CC0, public domain and BY always; share-alike and non-commercial when asked; nothing else.
fn allowed(prefs: &Prefs, license: &str) -> bool {
    match license.split(' ').next().unwrap_or("") {
        "CC0" | "PD" | "BY" => true,
        "BY-SA" => prefs.share_alike,
        "BY-NC" | "BY-NC-SA" => prefs.non_commercial,
        _ => false,
    }
}

/// Commons gives the artist as HTML.
fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in html.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            c if !inside => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&#039;", "'").replace("&quot;", "\"").trim().to_string()
}

fn commons_hits(body: &Value) -> Vec<Hit> {
    let Some(pages) = body["query"]["pages"].as_object() else { return Vec::new() };
    let mut pages: Vec<&Value> = pages.values().collect();
    pages.sort_by_key(|p| p["index"].as_u64().unwrap_or(0));
    pages
        .into_iter()
        .filter_map(|p| {
            let info = &p["videoinfo"][0];
            let meta = |k: &str| strip_tags(&text(&info["extmetadata"][k]["value"]));
            // what Ravune can open (no MIDI, no video)
            let ext = match info["mime"].as_str()? {
                "audio/mpeg" => "mp3",
                "audio/wav" | "audio/x-wav" => "wav",
                "audio/flac" | "audio/x-flac" => "flac",
                "audio/ogg" | "application/ogg" => "ogg",
                _ => return None,
            };
            let short = meta("LicenseShortName");
            let license = if short.contains("CC0") {
                "CC0".to_string()
            } else if short.to_lowercase().starts_with("public domain") {
                "PD".to_string()
            } else if let Some(cc) = short.strip_prefix("CC ") {
                cc.split(',').next().unwrap_or(cc).to_string()
            } else {
                "other".to_string()
            };
            Some(Hit {
                provider: Provider::Commons,
                id: text(&p["pageid"]),
                title: text(&p["title"]).trim_start_matches("File:").to_string(),
                author: meta("Artist"),
                license,
                license_url: meta("LicenseUrl"),
                page: text(&info["descriptionurl"]),
                audio: text(&info["url"]),
                ext: ext.into(),
                seconds: info["duration"].as_f64().unwrap_or(0.0) as f32,
                tags: Vec::new(),
            })
        })
        .collect()
}

fn ccmixter_hits(body: &Value) -> Vec<Hit> {
    let Some(uploads) = body.as_array() else { return Vec::new() };
    uploads
        .iter()
        .filter_map(|u| {
            // the first plain audio file (the zips hold the stems)
            let file = u["files"].as_array()?.iter().find(|f| {
                let info = &f["file_format_info"];
                info["media-type"] == "audio" && matches!(info["default-ext"].as_str(), Some("mp3" | "wav" | "flac" | "ogg"))
            })?;
            let info = &file["file_format_info"];
            // "6:46"
            let seconds = info["ps"].as_str().map_or(0.0, |ps| ps.split(':').fold(0.0, |acc, part| acc * 60.0 + part.parse::<f32>().unwrap_or(0.0)));
            let license_url = text(&u["license_url"]);
            let author = text(&u["user_real_name"]);
            Some(Hit {
                provider: Provider::CcMixter,
                id: text(&u["upload_id"]),
                title: text(&u["upload_name"]),
                author: if author.is_empty() { text(&u["user_name"]) } else { author },
                license: freesound_license(&license_url),
                license_url,
                page: text(&u["file_page_url"]),
                audio: text(&file["download_url"]),
                ext: text(&info["default-ext"]),
                seconds,
                tags: text(&u["upload_extra"]["usertags"]).split(',').filter(|t| !t.is_empty()).map(String::from).collect(),
            })
        })
        .collect()
}

fn custom_hits(c: &Custom, body: &Value) -> Vec<Hit> {
    let list = if c.results.trim().is_empty() { Some(body) } else { body.pointer(c.results.trim()) };
    let Some(items) = list.and_then(Value::as_array) else { return Vec::new() };
    items
        .iter()
        .filter_map(|r| {
            let get = |ptr: &str| r.pointer(ptr.trim()).map(text).unwrap_or_default();
            let audio = get(&c.audio);
            if audio.is_empty() {
                return None;
            }
            let ext = audio.split(['?', '#']).next().unwrap_or("").rsplit('.').next().filter(|e| (2..=4).contains(&e.len()) && !e.contains('/')).unwrap_or("mp3").to_lowercase();
            let id = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                audio.hash(&mut h);
                format!("{:016x}", h.finish())
            };
            let license_text = get(&c.license);
            Some(Hit {
                provider: Provider::Custom,
                id,
                title: get(&c.title),
                author: get(&c.author),
                license: license_from(&license_text),
                license_url: if license_text.starts_with("http") { license_text } else { String::new() },
                page: get(&c.page),
                audio,
                ext,
                seconds: get(&c.seconds).parse().unwrap_or(0.0),
                tags: Vec::new(),
            })
        })
        .collect()
}

/// For a search term inside a URL.
fn encode(text: &str) -> String {
    text.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn freesound_hits(body: &Value) -> Vec<Hit> {
    let Some(results) = body["results"].as_array() else { return Vec::new() };
    results
        .iter()
        .filter_map(|r| {
            let audio = r["previews"]["preview-hq-mp3"].as_str()?.to_string();
            let license_url = text(&r["license"]);
            Some(Hit {
                provider: Provider::Freesound,
                id: text(&r["id"]),
                title: text(&r["name"]),
                author: text(&r["username"]),
                license: freesound_license(&license_url),
                license_url,
                page: text(&r["url"]),
                audio,
                ext: "mp3".into(),
                seconds: r["duration"].as_f64().unwrap_or(0.0) as f32,
                tags: r["tags"].as_array().map(|t| t.iter().filter_map(|t| t.as_str().map(String::from)).collect()).unwrap_or_default(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The worker thread
// ---------------------------------------------------------------------------

enum Job {
    Search { prefs: Box<Prefs>, query: String, page: usize },
    Fetch { hit: Box<Hit>, to: PathBuf },
}

enum Reply {
    Found { page: usize, result: Result<(Vec<Hit>, usize), String> },
    Fetched { key: String, result: Result<PathBuf, String> },
}

/// The download running now: its hit's key, and how much is in (0..1; None while the size
/// is unknown). The worker fetches one at a time; the others wait their turn.
type Progress = Arc<Mutex<Option<(String, Option<f32>)>>>;

fn worker(jobs: Receiver<Job>, replies: Sender<Reply>, ctx: egui::Context, progress: Progress) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        // shared across a host's addresses, so one unreachable address can't eat it all
        .timeout_connect(Some(Duration::from_secs(6)))
        .user_agent(concat!("Ravune/", env!("CARGO_PKG_VERSION"), " (+https://github.com/g1lg1l/ravune)"))
        .build()
        .into();
    for job in jobs {
        let reply = match job {
            Job::Search { prefs, query, page } => Reply::Found { page, result: search(&agent, &prefs, &query, page) },
            Job::Fetch { hit, to } => {
                let key = hit.key();
                let result = fetch(&agent, &hit.audio, &to, |done| {
                    *progress.lock().unwrap() = Some((key.clone(), done));
                    ctx.request_repaint();
                });
                *progress.lock().unwrap() = None;
                Reply::Fetched { key, result: result.map(|()| to) }
            }
        };
        if replies.send(reply).is_err() {
            break;
        }
        ctx.request_repaint();
    }
}

fn get_json(request: ureq::RequestBuilder<ureq::typestate::WithoutBody>) -> Result<Value, String> {
    let mut response = request.call().map_err(|e| match e {
        ureq::Error::StatusCode(429) => tr!("too many searches for now (the free limit): try again in a minute").to_string(),
        ureq::Error::StatusCode(401) => tr!("the API key was refused").to_string(),
        e => e.to_string(),
    })?;
    response.body_mut().read_json().map_err(|e| e.to_string())
}

fn search(agent: &ureq::Agent, prefs: &Prefs, query: &str, page: usize) -> Result<(Vec<Hit>, usize), String> {
    let page_text = page.to_string();
    let (hits, total) = match prefs.provider {
        Provider::Openverse => {
            // no ND licenses ever: sampling a sound makes a derivative
            let mut licenses = vec!["cc0", "pdm", "by"];
            if prefs.share_alike {
                licenses.push("by-sa");
            }
            if prefs.non_commercial {
                licenses.extend(["by-nc", "by-nc-sa"]);
            }
            let mut req = agent.get("https://api.openverse.org/v1/audio/").query("q", query).query("license", licenses.join(",")).query("page_size", PAGE.to_string()).query("page", &page_text);
            if prefs.length != Length::Any {
                req = req.query("length", if prefs.length == Length::Long { "short" } else { "shortest" });
            }
            let body = get_json(req)?;
            (openverse_hits(&body), body["result_count"].as_u64().unwrap_or(0) as usize)
        }
        Provider::Freesound => {
            if prefs.freesound_key.trim().is_empty() {
                return Err(tr!("Freesound needs a free API key: paste it below").into());
            }
            let mut licenses = vec!["\"Creative Commons 0\"", "\"Attribution\""];
            if prefs.non_commercial {
                licenses.push("\"Attribution NonCommercial\"");
            }
            let mut filter = format!("license:({})", licenses.join(" OR "));
            filter += match prefs.length {
                Length::Any => "",
                Length::OneShot => " duration:[0 TO 2]",
                Length::Short => " duration:[0 TO 10]",
                Length::Long => " duration:[10 TO *]",
            };
            let req = agent
                .get("https://freesound.org/apiv2/search/text/")
                .query("query", query)
                .query("filter", &filter)
                .query("fields", "id,name,username,license,duration,previews,url,tags")
                .query("page_size", PAGE.to_string())
                .query("page", &page_text)
                .query("token", prefs.freesound_key.trim());
            let body = get_json(req)?;
            (freesound_hits(&body), body["count"].as_u64().unwrap_or(0) as usize)
        }
        Provider::Commons => {
            let req = agent
                .get("https://commons.wikimedia.org/w/api.php")
                .query("action", "query")
                .query("format", "json")
                .query("generator", "search")
                .query("gsrnamespace", "6")
                .query("gsrsearch", format!("{query} filemime:audio -filemime:audio/midi -filemime:audio/webm"))
                .query("gsrlimit", PAGE.to_string())
                .query("gsroffset", ((page - 1) * PAGE).to_string())
                .query("gsrinfo", "totalhits")
                .query("prop", "videoinfo")
                .query("viprop", "url|size|mime|extmetadata")
                .query("viextmetadatafilter", "LicenseShortName|Artist|LicenseUrl");
            let body = get_json(req)?;
            let hits = commons_hits(&body).into_iter().filter(|h| allowed(prefs, &h.license)).collect();
            (hits, body["query"]["searchinfo"]["totalhits"].as_u64().unwrap_or(0) as usize)
        }
        Provider::CcMixter => {
            let page_query = |f: &str| agent.get("https://ccmixter.org/api/query").query("f", f).query("search", query).query("tags", "sample");
            // "js", not "json": that one repeats the whole answer in a header
            let body = get_json(page_query("js").query("limit", PAGE.to_string()).query("offset", ((page - 1) * PAGE).to_string()))?;
            let total = get_json(page_query("count")).ok().and_then(|c| c[0].as_u64()).unwrap_or(0) as usize;
            (ccmixter_hits(&body).into_iter().filter(|h| allowed(prefs, &h.license)).collect(), total)
        }
        Provider::Custom => {
            let c = &prefs.custom;
            if !c.url.contains("{query}") {
                return Err(tr!("your source needs a search URL with {query} in it: set it up below").into());
            }
            let url = c.url.trim().replace("{query}", &encode(query)).replace("{page}", &page_text).replace("{key}", &encode(c.key.trim()));
            let body = get_json(agent.get(&url))?;
            let hits = custom_hits(c, &body);
            // unknown: there's more as long as pages come back full
            let total = body.pointer(c.total.trim()).and_then(Value::as_u64).map_or((page - 1) * hits.len().max(1) + hits.len() + usize::from(!hits.is_empty()), |t| t as usize);
            (hits, total)
        }
    };
    Ok((hits.into_iter().filter(|h| prefs.length.fits(h.seconds)).collect(), total))
}

/// Downloads `url` to `to`, telling `done` how much is in as it goes.
fn fetch(agent: &ureq::Agent, url: &str, to: &Path, mut done: impl FnMut(Option<f32>)) -> Result<(), String> {
    let mut response = agent.get(url).call().map_err(|e| e.to_string())?;
    let size = response.body().content_length();
    let mut reader = response.body_mut().with_config().limit(MAX_BYTES).reader();
    let mut bytes = Vec::with_capacity(size.unwrap_or(0).min(MAX_BYTES) as usize);
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let n = reader.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        done(size.map(|size| (bytes.len() as f64 / size.max(1) as f64).min(1.0) as f32));
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let part = to.with_extension("part");
    std::fs::write(&part, bytes).and_then(|()| std::fs::rename(&part, to)).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Online {
    worker: Option<(Sender<Job>, Receiver<Reply>)>,
    query: String,
    /// What the results are for (the field may have changed since).
    searched: String,
    hits: Vec<Hit>,
    total: usize,
    page: usize,
    searching: bool,
    error: Option<String>,
    fetching: HashSet<String>,
    /// How far the download in progress is (shared with the worker).
    progress: Progress,
    /// Downloaded previews, by hit key.
    fetched: HashMap<String, PathBuf>,
    selected: Option<String>,
    /// Play this one as soon as it's downloaded.
    play: Option<String>,
    /// …and add it as a channel.
    add: Option<String>,
}

impl Online {
    fn send(&mut self, ctx: &egui::Context, job: Job) {
        let (jobs, _) = self.worker.get_or_insert_with(|| {
            let (jobs, rx) = channel();
            let (tx, replies) = channel();
            let (ctx, progress) = (ctx.clone(), self.progress.clone());
            std::thread::Builder::new().name("online".into()).spawn(move || worker(rx, tx, ctx, progress)).expect("spawn the online thread");
            (jobs, replies)
        });
        let _ = jobs.send(job);
    }

    fn cached(hit: &Hit) -> PathBuf {
        std::env::temp_dir().join("ravune-online").join(format!("{}.{}", hit.key(), hit.ext))
    }
}

/// Copies a downloaded sound into `folder` and records its credit there.
fn keep(cached: &Path, hit: &Hit, folder: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let dest = folder.join(hit.file_name());
    if !dest.exists() {
        std::fs::copy(cached, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    let mut credits = read_credits(folder);
    credits.insert(hit.file_name(), hit.clone());
    let text = serde_json::to_string_pretty(&credits).map_err(|e| e.to_string())?;
    std::fs::write(folder.join(CREDITS), text).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// A folder's credits written back (gone with the last of them).
fn write_credits(folder: &Path, credits: &HashMap<String, Hit>) {
    let file = folder.join(CREDITS);
    let _ = if credits.is_empty() { std::fs::remove_file(file) } else { serde_json::to_string_pretty(credits).map_err(std::io::Error::other).and_then(|text| std::fs::write(file, text)) };
}

/// A kept sound moved from folder `from` (named `old`) to `to` (named `new`): its credit goes
/// with it.
pub fn move_credit(from: &Path, to: &Path, old: &str, new: &str) {
    let mut there = read_credits(from);
    let Some(hit) = there.remove(old) else { return };
    let mut here = read_credits(to);
    here.insert(new.to_string(), hit);
    write_credits(from, &there);
    write_credits(to, &here);
}

/// A kept sound deleted: its credit too.
pub fn drop_credit(folder: &Path, name: &str) {
    let mut credits = read_credits(folder);
    if credits.remove(name).is_some() {
        write_credits(folder, &credits);
    }
}

fn read_credits(folder: &Path) -> HashMap<String, Hit> {
    std::fs::read_to_string(folder.join(CREDITS)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Credits for the online sounds among `paths` (looked up in each folder's credits.json).
pub fn credits_for<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<Hit> {
    let mut folders: HashMap<PathBuf, HashMap<String, Hit>> = HashMap::new();
    let mut out: Vec<Hit> = Vec::new();
    for path in paths {
        let path = Path::new(path);
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { continue };
        let credits = folders.entry(dir.to_path_buf()).or_insert_with(|| read_credits(dir));
        if let Some(hit) = credits.get(&*name.to_string_lossy())
            && !out.contains(hit)
        {
            out.push(hit.clone());
        }
    }
    out
}

/// Free multi-sample libraries to download and drop into the browser.
const LIBRARIES: [(&str, &str, &str, &str); 5] = [
    ("Versilian Community Sample Library", "CC0", "SFZ · orchestral, percussion, keys", "https://github.com/sgossner/VCSL"),
    ("Salamander Grand Piano", "CC BY 3.0", "SFZ · a Yamaha C5, 16 velocity layers", "https://sfzinstruments.github.io/pianos/salamander"),
    ("SFZ Instruments", "free, see each", "SFZ · pianos, guitars, drums, strings", "https://sfzinstruments.github.io"),
    ("Pianobook", "free, see each", "Decent Sampler · community instruments", "https://www.pianobook.co.uk"),
    ("FreePats", "free, see each", "SFZ · general MIDI sounds", "https://freepats.zenvoid.org"),
];

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

impl App {
    /// Handles finished searches and downloads (called every frame, cheap when idle).
    pub fn poll_online(&mut self) {
        let Some((_, replies)) = &self.online.worker else { return };
        let replies: Vec<Reply> = replies.try_iter().collect();
        for reply in replies {
            match reply {
                Reply::Found { page, result } => {
                    let o = &mut self.online;
                    o.searching = false;
                    match result {
                        Ok((hits, total)) => {
                            if page == 1 {
                                o.hits.clear();
                            }
                            let fresh: Vec<Hit> = hits.into_iter().filter(|h| !o.hits.iter().any(|x| x.key() == h.key())).collect();
                            o.hits.extend(fresh);
                            (o.total, o.page, o.error) = (total, page, None);
                        }
                        Err(e) => o.error = Some(e),
                    }
                }
                Reply::Fetched { key, result } => {
                    self.online.fetching.remove(&key);
                    match result {
                        Ok(path) => {
                            self.online.fetched.insert(key.clone(), path.clone());
                            if self.online.play.as_ref() == Some(&key) {
                                self.online.play = None;
                                self.preview(&path.to_string_lossy());
                            }
                            if self.online.add.as_ref() == Some(&key) {
                                self.online.add = None;
                                self.add_online(&key);
                            }
                        }
                        Err(e) => self.say_as(Tone::Error, tr!("Download failed: {}", e)),
                    }
                }
            }
        }
    }

    /// Searches `query` online (from the Sounds tab when nothing local matches).
    pub fn search_online(&mut self, ctx: &egui::Context, query: &str) {
        self.online.query = query.to_string();
        self.start_search(ctx, 1);
    }

    fn start_search(&mut self, ctx: &egui::Context, page: usize) {
        let query = self.online.query.trim().to_string();
        if query.is_empty() {
            return;
        }
        self.online.searched.clone_from(&query);
        self.online.searching = true;
        self.online.error = None;
        if page == 1 {
            // a new search: the old results go now, not when the new ones land
            self.online.hits.clear();
            self.online.total = 0;
        }
        let prefs = Box::new(self.settings.online.clone());
        self.online.send(ctx, Job::Search { prefs, query, page });
    }

    /// Downloads a hit's audio (once); `play` previews it when it arrives.
    fn fetch_hit(&mut self, ctx: &egui::Context, hit: &Hit, play: bool) {
        let key = hit.key();
        if let Some(path) = self.online.fetched.get(&key).cloned() {
            if play {
                self.preview(&path.to_string_lossy());
            }
            return;
        }
        let cached = Online::cached(hit);
        if cached.exists() {
            self.online.fetched.insert(key, cached.clone());
            if play {
                self.preview(&cached.to_string_lossy());
            }
            return;
        }
        if play {
            self.online.play = Some(key.clone());
        }
        if self.online.fetching.insert(key) {
            self.online.send(ctx, Job::Fetch { hit: Box::new(hit.clone()), to: cached });
        }
    }

    /// Keeps a downloaded hit in Online Sounds; returns its new path.
    fn keep_hit(&mut self, key: &str) -> Option<String> {
        let hit = self.online.hits.iter().find(|h| h.key() == key)?.clone();
        let cached = self.online.fetched.get(key)?.clone();
        let folder = self.settings.online.folder();
        match keep(&cached, &hit, &folder) {
            Ok(path) => {
                let folder = folder.to_string_lossy().into_owned();
                if !self.settings.folders.contains(&folder) {
                    self.settings.folders.push(folder);
                }
                self.library.refresh();
                Some(path.to_string_lossy().into_owned())
            }
            Err(e) => {
                self.say_as(Tone::Error, tr!("Can't keep it: {}", e));
                None
            }
        }
    }

    fn add_online(&mut self, key: &str) {
        let Some(name) = self.online.hits.iter().find(|h| h.key() == key).map(|h| h.name().to_string()) else { return };
        if let Some(path) = self.keep_hit(key) {
            self.run("channel.add", json!({ "sample": path, "name": name }));
            self.say_as(Tone::Done, tr!("Added \"{}\" · its credit is saved for File → Export credits", name));
        }
    }

    /// The browser's Online tab.
    pub fn online_tab(&mut self, ui: &mut Ui, t: &Theme) {
        let ctx = ui.ctx().clone();
        kit::bar(ui, t, |ui| {
            let current = self.settings.online.provider;
            let mut picked = None;
            let r = egui::ComboBox::from_id_salt("online-provider").selected_text(format!("{}  {}", icons::GLOBE, crate::i18n::t(current.name()))).width(ui.available_width()).show_ui(ui, |ui| {
                for (provider, about) in Provider::ALL {
                    if ui.selectable_label(provider == current, crate::i18n::t(provider.name())).on_hover_text(crate::i18n::t(about)).clicked() {
                        picked = Some(provider);
                    }
                }
            });
            self.hint(&r.response, tr!("Where to search: each has its own sounds and licenses."));
            if let Some(provider) = picked.filter(|p| *p != current) {
                self.settings.online.provider = provider;
                self.start_search(&ctx, 1);
            }
        });
        kit::bar(ui, t, |ui| {
            let edit = kit::field(&mut self.online.query)
                .hint_text(format!("{}  {}", icons::GLOBE, tr!("Search free sounds…")))
                .desired_width(ui.available_width() - 26.0 - t.gap);
            let r = ui.add(edit);
            if std::mem::take(&mut self.browser.focus_search) {
                r.request_focus();
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            self.hint(&r, tr!("Type and press Enter: kick, vocal chop, pad, field recording…"));
            // while it searches, the button is the spinner
            let go = if self.online.searching {
                let (rect, r) = ui.allocate_exact_size(vec2(26.0, 24.0), egui::Sense::hover());
                egui::Spinner::new().size(14.0).color(t.select).paint_at(ui, Rect::from_center_size(rect.center(), vec2(14.0, 14.0)));
                self.hint(&r, tr!("Searching {}…", crate::i18n::t(self.settings.online.provider.name())));
                false
            } else {
                kit::icon_button(ui, t, icons::MAGNIFYING_GLASS, tr!("Search"), false).clicked()
            };
            if enter || go {
                self.start_search(&ctx, 1);
            }
        });
        kit::bar(ui, t, |ui| {
            // the whole bar: the key's popup opens under it, exactly as wide as the panel
            let bar = ui.max_rect().expand2(vec2(t.pad, 0.0));
            let mut changed = false;
            // right to left first, so the length menu gets only the room the buttons leave
            kit::right(ui, |ui| {
                // once Freesound has its key, it waits behind an icon (to read, copy or change it)
                if self.settings.online.provider == Provider::Freesound && !self.settings.online.freesound_key.trim().is_empty() {
                    let r = kit::icon_button(ui, t, icons::KEY, tr!("Your Freesound key"), false);                    let frame = egui::Frame::popup(ui.style());
                    let width = bar.width() - frame.total_margin().sum().x;
                    egui::Popup::from_toggle_button_response(&r)
                        .anchor(bar)
                        .align(egui::RectAlign::BOTTOM_START)
                        .align_alternatives(&[])
                        .frame(frame)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .show(|ui| {
                            ui.set_width(width);
                            kit::caps(ui, t, tr!("FREESOUND KEY"));
                            Self::key_field(ui, t, &mut self.settings.online.freesound_key);
                        });
                }
                let o = &mut self.settings.online;
                let r = kit::button(ui, t, None, "NC", o.non_commercial);
                if r.clicked() {
                    o.non_commercial = !o.non_commercial;
                    changed = true;
                }
                let nc = r.on_hover_text(tr!("Include non-commercial licenses: never in music you sell or monetize"));
                let r = kit::button(ui, t, None, "SA", o.share_alike);
                if r.clicked() {
                    o.share_alike = !o.share_alike;
                    changed = true;
                }
                let sa = r.on_hover_text(tr!("Include CC BY-SA: usable, but what you make from it must keep the same license"));
                self.hint(&sa, tr!("CC0 and CC BY are always in. SA: share-alike licenses too."));
                self.hint(&nc, tr!("NC: non-commercial licenses too. Leave it off for anything you might release."));
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let o = &mut self.settings.online;
                    let label = Length::ALL.iter().find(|l| l.0 == o.length).map_or("", |l| crate::i18n::t(l.1));
                    egui::ComboBox::from_id_salt("online-length").selected_text(label).width(ui.available_width()).truncate().show_ui(ui, |ui| {
                        for (length, name) in Length::ALL {
                            changed |= ui.selectable_value(&mut o.length, length, crate::i18n::t(name)).clicked();
                        }
                    });
                });
            });
            if changed {
                self.start_search(&ctx, 1);
            }
        });
        if self.settings.online.provider == Provider::Freesound && self.settings.online.freesound_key.trim().is_empty() {
            self.freesound_key(ui, t);
        }
        if self.settings.online.provider == Provider::Custom {
            self.custom_source(ui, t);
        }
        kit::scroll(ui, t, "browser-online", false, t.bg_panel, |ui| {
            if self.online.searching && self.online.hits.is_empty() {
                kit::padded(ui, t, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0).color(t.select));
                        ui.label(RichText::new(tr!("Searching {}…", crate::i18n::t(self.settings.online.provider.name()))).color(t.fg_muted));
                    })
                });
            }
            if let Some(e) = &self.online.error {
                let e = e.clone();
                kit::padded(ui, t, |ui| ui.label(RichText::new(e).color(t.meter_hot)));
            }
            if self.online.hits.is_empty() && !self.online.searching {
                self.online_intro(ui, t);
                return;
            }
            let hits = self.online.hits.clone();
            for hit in &hits {
                self.hit_row(ui, t, &ctx, hit);
            }
            let more = self.online.hits.len() < self.online.total && !self.online.searching;
            if hits.is_empty() {
                return;
            }
            kit::padded(ui, t, |ui| {
                ui.horizontal(|ui| {
                    let shown = tr!("{} of {} for \"{}\"", self.online.hits.len(), self.online.total, self.online.searched);
                    ui.label(RichText::new(shown).small().color(t.fg_muted));
                    if self.online.searching {
                        ui.add(egui::Spinner::new().size(12.0).color(t.select));
                    } else if more && kit::button(ui, t, Some(icons::CARET_DOWN), tr!("More"), false).clicked() {
                        let next = self.online.page + 1;
                        self.start_search(&ctx, next);
                    }
                });
            });
        });
    }

    /// Freesound's API key, until there is one: what it is and where to paste it.
    fn freesound_key(&mut self, ui: &mut Ui, t: &Theme) {
        kit::padded(ui, t, |ui| {
            ui.label(RichText::new(tr!("Freesound needs a free API key (an account, then \"Apply for a key\").")).small().color(t.fg_muted));
            Self::key_field(ui, t, &mut self.settings.online.freesound_key);
        });
    }

    /// The key's field, in plain text with a copy button, and where to get one.
    fn key_field(ui: &mut Ui, t: &Theme, key: &mut String) {
        ui.horizontal(|ui| {
            // exactly what the copy button takes: more and a popup sized to its content widens every frame
            let button = 26.0 + ui.spacing().item_spacing.x;
            ui.add(kit::field(key).hint_text(tr!("Paste your API key")).font(egui::TextStyle::Monospace).desired_width((ui.available_width() - button).max(80.0)));
            if kit::icon_button(ui, t, icons::COPY, tr!("Copy the key"), false).clicked() {
                ui.ctx().copy_text(key.trim().to_string());
            }
        });
        ui.hyperlink_to(format!("{}  {}", icons::ARROW_SQUARE_OUT, tr!("Get a key at freesound.org")), "https://freesound.org/apiv2/apply");
    }

    /// Your own source: the search URL and where things are in its answer.
    fn custom_source(&mut self, ui: &mut Ui, t: &Theme) {
        kit::padded(ui, t, |ui| {
            egui::CollapsingHeader::new(RichText::new(tr!("YOUR SOURCE")).small().strong()).id_salt("online-custom").default_open(true).show(ui, |ui| {
                ui.label(RichText::new(tr!("A search URL with {query} (and {page}, {key}); then where things are in its JSON answer, as JSON pointers. Licenses are taken as the source gives them.")).small().color(t.fg_muted));
                let c = &mut self.settings.online.custom;
                let blank = Custom::default();
                let fields: [(&str, &mut String, &str); 10] = [
                    ("Search URL", &mut c.url, &blank.url),
                    ("Key", &mut c.key, tr!("optional, goes where {key} is")),
                    ("Results", &mut c.results, &blank.results),
                    ("Total", &mut c.total, &blank.total),
                    ("Title", &mut c.title, &blank.title),
                    ("Audio file", &mut c.audio, &blank.audio),
                    ("Author", &mut c.author, &blank.author),
                    ("License", &mut c.license, &blank.license),
                    ("Page", &mut c.page, &blank.page),
                    ("Seconds", &mut c.seconds, "/duration"),
                ];
                egui::Grid::new("online-custom-fields").num_columns(2).spacing(vec2(t.gap, 4.0)).show(ui, |ui| {
                    for (label, value, hint) in fields {
                        ui.label(RichText::new(crate::i18n::t(label)).small().color(t.fg_muted));
                        let edit = kit::field(value).hint_text(hint).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY);
                        ui.add(if label == "Key" { edit.password(true) } else { edit });
                        ui.end_row();
                    }
                });
                if kit::button(ui, t, Some(icons::ARROW_COUNTER_CLOCKWISE), tr!("Reset to the example"), false).clicked() {
                    *c = Custom::default();
                }
            });
        });
    }

    /// Before the first search: what this is, where kept sounds go, free instrument libraries.
    fn online_intro(&mut self, ui: &mut Ui, t: &Theme) {
        kit::padded(ui, t, |ui| {
            ui.label(RichText::new(tr!("Free sounds from Openverse, Wikimedia Commons and ccMixter (no account), Freesound (free key) or your own source. Click to preview, double-click to add; what you use is kept with its license.")).small().line_height(Some(15.0)).color(t.fg_muted));
            ui.label(RichText::new(tr!("Kept in {}", self.settings.online.folder().display())).small().line_height(Some(15.0)).color(t.fg_muted));
            ui.add_space(t.pad);
            kit::caps(ui, t, tr!("FREE INSTRUMENTS"));
        });
        for (name, license, what, url) in LIBRARIES {
            let r = kit::row(ui, t, Id::unique(("online-library", name)), false);
            let mut painter = ui.painter_at(r.rect);
            painter.multiply_opacity(kit::row_alpha(ui, &r));
            let y = r.rect.center().y;
            painter.text(pos2(r.rect.left() + t.pad + 7.0, y), Align2::CENTER_CENTER, icons::PIANO_KEYS, FontId::proportional(14.0), t.fg_muted);
            let note = painter.text(r.rect.right_center() - vec2(t.pad, 0.0), Align2::RIGHT_CENTER, crate::i18n::t(license), FontId::proportional(11.0), t.fg_muted);
            let x = r.rect.left() + t.pad + 22.0;
            if kit::fit_text(&painter, pos2(x, y), Align2::LEFT_CENTER, name, FontId::proportional(12.0), t.fg_text, note.left() - t.gap * 2.0 - x).1 {
                r.clone().on_hover_text(name);
            }
            if r.clicked() || (r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
            self.hint(&r, tr!("{} · {} · opens {}: download it, unzip, then add the folder in Sounds", crate::i18n::t(what), crate::i18n::t(license), url));
        }
    }

    fn hit_row(&mut self, ui: &mut Ui, t: &Theme, ctx: &egui::Context, hit: &Hit) {
        let key = hit.key();
        let selected = self.online.selected.as_ref() == Some(&key);
        let r = kit::row(ui, t, Id::unique(("online-hit", &key)), selected);
        let mut painter = ui.painter_at(r.rect);
        painter.multiply_opacity(kit::row_alpha(ui, &r));
        let y = r.rect.center().y;
        let icon_at = Rect::from_center_size(pos2(r.rect.left() + t.pad + 7.0, y), vec2(13.0, 13.0));
        // downloading: a ring filling up as it comes in (a spinner while it waits its turn or
        // the size is unknown)
        let now = self.online.progress.lock().unwrap().clone().filter(|(k, _)| *k == key).map(|(_, done)| done);
        if let Some(Some(done)) = now {
            widgets::progress_ring(&painter, t, icon_at.center(), 6.0, done);
            r.clone().on_hover_text(tr!("Downloading: {}%", format!("{:.0}", done * 100.0)));
        } else if self.online.fetching.contains(&key) {
            egui::Spinner::new().color(t.select).paint_at(ui, icon_at);
        } else {
            let downloaded = self.online.fetched.contains_key(&key);
            painter.text(icon_at.center(), Align2::CENTER_CENTER, icons::WAVEFORM, FontId::proportional(14.0), if downloaded { t.fg_text } else { t.fg_muted });
        }
        // the license on the right, colored by how free it is
        let color = if hit.license.contains("NC") {
            t.meter_hot
        } else if hit.license.contains("SA") {
            t.meter_warn
        } else if hit.license.starts_with("BY") {
            t.select
        } else {
            t.meter_ok
        };
        let short = hit.license.split(' ').next().unwrap_or("");
        let badge = painter.text(r.rect.right_center() - vec2(t.pad, 0.0), Align2::RIGHT_CENTER, short, FontId::monospace(10.0), color);
        let mut right = badge.left() - t.gap * 2.0;
        if hit.seconds > 0.0 {
            let len = painter.text(pos2(right, y), Align2::RIGHT_CENTER, format!("{:.1}s", hit.seconds), FontId::monospace(10.0), t.fg_muted);
            right = len.left() - t.gap * 2.0;
        }
        let name_rect = Rect::from_min_max(pos2(r.rect.left() + t.pad + 22.0, r.rect.top()), pos2(right, r.rect.bottom()));
        if kit::fit_text(&painter, pos2(name_rect.left(), y), Align2::LEFT_CENTER, hit.title.as_str(), FontId::proportional(12.0), t.fg_text, name_rect.width()).1 {
            r.clone().on_hover_text(hit.title.as_str());
        }

        let enter = r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if r.drag_started() {
            match self.online.fetched.contains_key(&key).then(|| self.keep_hit(&key)).flatten() {
                Some(path) => r.dnd_set_drag_payload(SampleDrag(path)),
                None => {
                    self.fetch_hit(ctx, hit, false);
                    self.say(tr!("Downloading it first: drag again in a moment"));
                }
            }
        } else if r.double_clicked() || enter {
            if self.online.fetched.contains_key(&key) {
                self.add_online(&key);
            } else {
                self.online.add = Some(key.clone());
                self.fetch_hit(ctx, hit, false);
            }
        } else if r.clicked() || r.gained_focus() {
            self.online.selected = Some(key.clone());
            self.fetch_hit(ctx, hit, true);
        }
        let tags = hit.tags.iter().take(6).cloned().collect::<Vec<_>>().join(", ");
        self.hint(&r, tr!("{} · by {} · {} · {} · click previews, double-click adds, drag to the rack or playlist", hit.title, hit.author, license_label(&hit.license), tags));
    }

    /// File → Export credits: every online sound the project uses, with its license.
    pub fn export_credits(&mut self) {
        let p = &self.session.project;
        let mut paths: Vec<&str> = p
            .channels
            .iter()
            .filter_map(|c| match &c.instrument {
                Instrument::Sampler { sample } => Some(sample.as_str()),
                _ => None,
            })
            .collect();
        paths.extend(p.clips.iter().filter_map(|c| match &c.kind {
            ravune_core::ClipKind::Audio { sample } => Some(sample.as_str()),
            _ => None,
        }));
        let credits = credits_for(paths);
        if credits.is_empty() {
            self.say(tr!("No online sounds in this project, nothing to credit"));
            return;
        }
        let dialog = rfd::FileDialog::new().add_filter(tr!("Text"), &["txt"]).set_file_name(tr!("{} credits.txt", self.file_name()));
        let Some(path) = dialog.save_file() else { return };
        let mut text = tr!("Sounds used in {}", self.file_name()) + "\n\n";
        for hit in &credits {
            text += &format!("{}\n", hit.credit());
        }
        match std::fs::write(&path, text) {
            Ok(()) => self.say_as(Tone::Done, tr!("Credits for {} sounds saved to {}", credits.len(), path.display())),
            Err(e) => self.say_as(Tone::Error, tr!("Can't write {}: {}", path.display(), e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commons_ccmixter_and_custom() {
        let commons = json!({"query": {"searchinfo": {"totalhits": 50}, "pages": {
            "2": {"pageid": 2, "index": 2, "title": "File:Snare.mid", "videoinfo": [{"mime": "audio/midi", "url": "https://x/Snare.mid"}]},
            "1": {"pageid": 1, "index": 1, "title": "File:Kick drum.ogg", "videoinfo": [{"mime": "audio/ogg", "duration": 1.5,
                "url": "https://upload.wikimedia.org/k.ogg", "descriptionurl": "https://commons.wikimedia.org/wiki/File:Kick_drum.ogg",
                "extmetadata": {"Artist": {"value": "<a href=\"//x\">Jo &amp; Al</a>"}, "LicenseShortName": {"value": "CC BY-SA 3.0,2.5"}, "LicenseUrl": {"value": "https://creativecommons.org/licenses/by-sa/3.0"}}}]}}}});
        let hits = commons_hits(&commons);
        assert_eq!(hits.len(), 1, "MIDI isn't sound");
        assert_eq!((hits[0].name(), hits[0].author.as_str(), hits[0].license.as_str(), hits[0].ext.as_str()), ("Kick drum", "Jo & Al", "BY-SA 3.0", "ogg"));
        let prefs = Prefs::default();
        assert!(!allowed(&prefs, "BY-SA 3.0") && allowed(&prefs, "BY 4.0") && !allowed(&prefs, "other"));

        let ccmixter = json!([{"upload_id": 7, "upload_name": "Vox", "user_name": "ann", "user_real_name": "", "license_url": "https://creativecommons.org/licenses/by/4.0/",
            "file_page_url": "https://ccmixter.org/files/ann/7", "upload_extra": {"usertags": "vocal,dry"},
            "files": [{"download_url": "https://ccmixter.org/x.zip", "file_format_info": {"media-type": "archive", "default-ext": "zip"}},
                      {"download_url": "https://ccmixter.org/x.mp3", "file_format_info": {"media-type": "audio", "default-ext": "mp3", "ps": "1:05"}}]}]);
        let hits = ccmixter_hits(&ccmixter);
        assert_eq!((hits[0].audio.as_str(), hits[0].seconds, hits[0].license.as_str(), hits[0].author.as_str()), ("https://ccmixter.org/x.mp3", 65.0, "BY 4.0", "ann"));

        let custom = json!({"result_count": 1, "results": [{"title": "Pad", "url": "https://x.org/a/pad.WAV?dl=1", "creator": "me", "license_url": "https://creativecommons.org/publicdomain/zero/1.0/"}]});
        let hits = custom_hits(&Custom::default(), &custom);
        assert_eq!((hits[0].title.as_str(), hits[0].ext.as_str(), hits[0].license.as_str()), ("Pad", "wav", "CC0 1.0"));
        assert_eq!(encode("vocal chop & co"), "vocal%20chop%20%26%20co");
    }

    #[test]
    #[ignore = "needs the network; run with --ignored"]
    fn searches_the_sources_that_need_no_account() {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent("Ravune tests (+https://github.com/g1lg1l/ravune)")
            .build()
            .into();
        for provider in [Provider::Openverse, Provider::Commons, Provider::CcMixter, Provider::Custom] {
            let prefs = Prefs { provider, share_alike: true, non_commercial: true, ..Prefs::default() };
            let (hits, total) = search(&agent, &prefs, "piano", 1).unwrap_or_else(|e| panic!("{}: {e}", provider.name()));
            assert!(!hits.is_empty() && total >= hits.len(), "{}: {} of {total}", provider.name(), hits.len());
            let h = &hits[0];
            assert!(h.audio.starts_with("http") && !h.title.is_empty(), "{}: {h:?}", provider.name());
            eprintln!("{}: {} of {total}, first \"{}\" by {} ({}, {:.0} s)", provider.name(), hits.len(), h.title, h.author, h.license, h.seconds);
        }
    }

    #[test]
    fn parses_both_providers() {
        let openverse = json!({"result_count": 240, "results": [
            {"id": "58196449", "title": "soccer kick 02.wav", "creator": "volivieri", "license": "by", "license_version": "4.0",
             "license_url": "https://creativecommons.org/licenses/by/4.0/", "foreign_landing_url": "https://freesound.org/people/volivieri/sounds/37157",
             "url": "https://cdn.freesound.org/previews/37/37157_374422-hq.mp3", "filetype": "mp3", "duration": 1148, "tags": [{"name": "kick"}]},
            {"id": "x", "title": "no audio", "license": "cc0", "url": null}
        ]});
        let hits = openverse_hits(&openverse);
        assert_eq!(hits.len(), 1, "results without a file are skipped");
        let h = &hits[0];
        assert_eq!((h.license.as_str(), h.seconds, h.ext.as_str()), ("BY 4.0", 1.148, "mp3"));
        assert_eq!(h.file_name(), "soccer kick 02 - volivieri (Openverse 58196449).mp3");
        assert_eq!(h.name(), "soccer kick 02");
        assert!(h.credit().contains("CC BY 4.0") && h.credit().contains("volivieri"));

        let freesound = json!({"count": 2, "results": [
            {"id": 331589, "name": "Kick J.Zealous Hard Club.aif", "username": "J_Zealous", "license": "http://creativecommons.org/publicdomain/zero/1.0/",
             "duration": 2.24, "url": "https://freesound.org/people/J_Zealous/sounds/331589/", "tags": ["kick"],
             "previews": {"preview-hq-mp3": "https://cdn.freesound.org/previews/331/331589_5820980-hq.mp3"}}
        ]});
        let hits = freesound_hits(&freesound);
        assert_eq!((hits[0].id.as_str(), hits[0].license.as_str()), ("331589", "CC0 1.0"));
        assert_eq!(freesound_license("https://creativecommons.org/licenses/by-nc/3.0/"), "BY-NC 3.0");
        assert!(Length::OneShot.fits(1.5) && !Length::OneShot.fits(3.0) && Length::OneShot.fits(0.0));
    }

    #[test]
    fn kept_sounds_carry_their_credits() {
        let dir = std::env::temp_dir().join("ravune-online-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cached = dir.join("download.mp3");
        std::fs::write(&cached, b"fake").unwrap();
        let hit = Hit {
            provider: Provider::Freesound,
            id: "7".into(),
            title: "Vocal: chop/1".into(),
            author: "me".into(),
            license: "CC0 1.0".into(),
            license_url: "u".into(),
            page: "p".into(),
            audio: "a".into(),
            ext: "mp3".into(),
            seconds: 1.0,
            tags: Vec::new(),
        };
        let kept = keep(&cached, &hit, &dir.join("Online Sounds")).unwrap();
        assert!(kept.ends_with("Vocal_ chop_1 - me (Freesound 7).mp3"), "{}", kept.display());
        let credits = credits_for([kept.to_str().unwrap(), "builtin:kick.wav"]);
        assert_eq!(credits, vec![hit]);
    }

    /// A download reports how much is in as it goes, ending at all of it.
    #[test]
    fn fetch_reports_progress() {
        use std::io::Write;
        let body = vec![7u8; 100_000];
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/sound.mp3", server.local_addr().unwrap());
        let sent = body.clone();
        std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            write!(stream, "HTTP/1.1 200 OK
Content-Length: {}
Connection: close

", sent.len()).unwrap();
            stream.write_all(&sent).unwrap();
        });
        let to = std::env::temp_dir().join("ravune-fetch-test").join("sound.mp3");
        let mut seen = Vec::new();
        fetch(&ureq::Agent::new_with_defaults(), &url, &to, |done| seen.push(done)).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), body);
        assert!(seen.len() > 1, "{seen:?}");
        assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
        assert_eq!(seen.last(), Some(&Some(1.0)));
    }
}
