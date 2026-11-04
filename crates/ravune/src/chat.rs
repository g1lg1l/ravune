//! The AI chat (Ctrl I), ported from Velo's AI panel and squared off for Ravune: conversations
//! like Claude's desktop app and Claude Code, full height on the right. The user asks for a
//! change, with pictures as references if they like (attached, dropped on the chat or pasted),
//! and the AI chosen under the box (ai.rs: Claude Code, Codex, a model behind an API, local or
//! not; its model picked there too) makes it through Ravune's tools; each reply undoes in one
//! step. Replies are Markdown (markdown.rs); a reply's commands fold into one row that opens; a
//! problem is a card with what the provider said and a retry. Its parts follow shadcn's AI
//! Elements (Conversation, Message, Tool rows, Suggestions, Prompt Input with attachments) and
//! RareUI (AI Status's shimmering status); its state is a Thinking Orb drawn in square pixels.
//! Conversations are kept in Ravune's data folder, their pictures beside them.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::Instant;

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Frame, Key, KeyboardShortcut, Label, Layout, Modifiers, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, UiBuilder, pos2, vec2};
use ravune_core::Project;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ai::{self, Event, Provider, Run};
use crate::orb::{self, State};
use crate::theme::Theme;
use crate::{App, icons, kit, markdown};
use crate::shell::Tone;

/// What the empty chat offers to try (and sends when clicked).
const EXAMPLES: [&str; 3] = ["Make the kick punchier", "Add a reverb bus for the snare", "Write a bassline in A minor"];
const FILE: &str = "chats.json";
/// Conversations kept, newest first.
const KEEP: usize = 50;
/// What can go with a message: pictures.
const PICTURES: [&str; 3] = ["png", "jpg", "jpeg"];

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum StepState {
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    id: String,
    label: String,
    state: StepState,
    /// What went wrong, for one that failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn all() -> usize {
    usize::MAX
}

/// A message the user sent: its words, and the pictures that went with it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Message {
    text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pictures: Vec<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Item {
    User(Message),
    /// The AI's words; `shown` characters typed in so far (a reopened chat shows them all).
    Text {
        text: String,
        #[serde(skip, default = "all")]
        shown: usize,
    },
    /// A run of commands: one row that opens.
    Steps(Vec<Step>),
    Problem(String),
    /// Something about the conversation itself (another AI took over, it was stopped).
    Note(String),
    /// The end of a reply: how long it took, how many commands.
    End {
        seconds: u32,
        steps: usize,
    },
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Conversation {
    title: String,
    provider: Provider,
    /// The CLI's session or thread: the next message carries on with it.
    session: Option<String>,
    /// A local model's messages so far.
    history: Vec<Value>,
    items: Vec<Item>,
    /// When it last changed (Unix seconds).
    updated: u64,
    #[serde(skip)]
    run: Option<Run>,
    #[serde(skip)]
    started: Option<Instant>,
    /// The project before the last reply, and after it while nothing else changed it.
    #[serde(skip)]
    before: Option<Project>,
    #[serde(skip)]
    after: Option<Project>,
}

/// What a worker found out for the chat.
enum News {
    /// A provider's test.
    Checked(Provider, Result<String, String>),
    /// An endpoint's models.
    Models(String, Result<Vec<String>, String>),
    /// The local servers answering, and their models.
    Found(Vec<(usize, Vec<String>)>),
}

#[derive(Default)]
pub struct Chat {
    chats: Vec<Conversation>,
    current: usize,
    pub draft: String,
    /// The pictures going with the next message.
    pub pictures: Vec<PathBuf>,
    /// Put the cursor in the box next frame (it was opened from the keyboard).
    pub focus: bool,
    loaded: bool,
    /// Rows of commands opened: (their conversation's `updated`, item index).
    open: HashSet<(u64, usize)>,
    /// When the typed-in text last moved.
    typed: Option<Instant>,
    /// The prompt bar's height last frame: the conversation scrolls under it.
    bar_h: f32,
    /// Scroll to the newest message next frame.
    jump: bool,
    /// Pictures as textures, by file (None: it can't be read).
    thumbs: HashMap<PathBuf, Option<TextureHandle>>,
    /// What the workers found, as it comes.
    news: Option<(Sender<News>, Receiver<News>)>,
    /// Each provider's last test, and the tests on their way.
    checks: Vec<(Provider, Result<String, String>)>,
    checking: Vec<Provider>,
    /// The endpoint's models, once asked for.
    models: Option<Result<Vec<String>, String>>,
    /// The local servers answering on their usual ports.
    found: Option<Vec<(usize, Vec<String>)>>,
    /// Codex's models (id, name), read when the box first shows and when the menu opens.
    codex_models: Vec<(String, String)>,
    /// Typed to narrow a long list of models.
    filter: String,
    /// The setup guide shows at the top of the conversation.
    guide: bool,
    /// An API key being typed (never kept: Save puts it in the system keychain).
    key: String,
    /// Whether the endpoint has a key in the keychain, looked up when the menu opens.
    has_key: Option<bool>,
    /// The chat last deleted, and where it was: its toast's Undo puts it back.
    deleted: Option<(usize, Conversation)>,
    /// Where the chat was last frame: pictures dropped there go with the next message.
    rect: Option<Rect>,
    /// Where the box was last frame: the effort's slider opens over it.
    composer: Option<Rect>,
}

impl Chat {
    fn check(&self, provider: Provider) -> Option<&Result<String, String>> {
        self.checks.iter().find(|c| c.0 == provider).map(|c| &c.1)
    }

    /// A reply is on its way (the agent button lights up).
    pub fn busy(&self) -> bool {
        self.chats.iter().any(|c| c.run.is_some())
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// "now", "5 min", "3 h", "2 d".
fn ago(when: u64) -> String {
    match now().saturating_sub(when) {
        s if s < 60 => tr!("now").into(),
        s if s < 3600 => format!("{} min", s / 60),
        s if s < 86_400 => format!("{} h", s / 3600),
        s => format!("{} d", s / 86_400),
    }
}

/// The time the orbs and shimmers run on.
fn seconds() -> f64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Who answers, as the box shows it: "Claude Code · Opus 5.5", "Ollama · qwen2.5:14b" (Codex's
/// models by the names in its list).
fn who(prefs: &ai::Prefs, codex: &[(String, String)]) -> String {
    let mut prefs = prefs.clone();
    let p = prefs.provider;
    let name = match p {
        Provider::Local => ai::preset(&prefs.endpoint).map_or_else(|| ai::host(&prefs.endpoint), |i| ai::PRESETS[i].0.to_string()),
        p => p.name().to_string(),
    };
    match p.model(&mut prefs).trim() {
        "" => name,
        model => format!("{name} · {}", ai::model_name(model, codex)),
    }
}

/// Small text on lines with room between them (egui's own are tight).
fn small(text: impl Into<String>) -> RichText {
    RichText::new(text).small().line_height(Some((Theme::TEXT_SM * 1.45).round()))
}

/// Small monospaced text, as [`small`].
fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text).font(Theme::mono(Theme::TEXT_XS)).line_height(Some((Theme::TEXT_XS * 1.5).round()))
}

/// A status light: a small square, Ravune's LEDs.
fn led(painter: &egui::Painter, center: Pos2, color: Color32) {
    painter.rect_filled(Rect::from_center_size(center, vec2(6.0, 6.0)), 1, color);
}

fn is_picture(path: &Path) -> bool {
    path.extension().is_some_and(|e| PICTURES.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// What a problem asks the user to do, when Ravune can tell.
fn help(provider: Provider, problem: &str) -> Option<String> {
    let p = problem.to_lowercase();
    if p.contains("isn't installed") {
        return Some(match provider {
            Provider::ClaudeCode => tr!("Install Claude Code (claude.com/claude-code), run claude auth login in a terminal and sign in, then send again.").into(),
            _ => tr!("Install Codex (npm install -g @openai/codex), run codex login once in a terminal, then send again.").into(),
        });
    }
    if ["authenticate", "oauth", "login", "log in", "sign in", "401", "unauthorized"].iter().any(|w| p.contains(w)) {
        return Some(match provider {
            Provider::ClaudeCode => tr!("Claude Code is signed out: run claude auth login in a terminal and sign in, then send again. The claude command signs in on its own: the Claude app's sign-in doesn't carry over.").into(),
            Provider::Codex => tr!("Codex is signed out: run codex login in a terminal, then send again.").into(),
            Provider::Local => tr!("The endpoint wants a key: paste it in the menu under the box (local models like Ollama and LM Studio need none).").into(),
        });
    }
    if provider == Provider::Local && (p.contains("connect") || p.contains("refused") || p.contains("didn't answer")) {
        return Some(tr!("Is the model running? Start Ollama (ollama serve) or LM Studio's server, and check the endpoint in the menu under the box.").into());
    }
    None
}

/// Text with a light sweeping across it (RareUI's AI Status, AI Elements' Shimmer).
fn shimmer(ui: &mut Ui, t: &Theme, text: &str) {
    let mut job = LayoutJob::default();
    let n = text.chars().count() as f32;
    let at = (seconds() * 0.9).fract() as f32 * (n + 10.0) - 5.0;
    for (i, c) in text.chars().enumerate() {
        let k = (-((i as f32 - at) / 3.0).powi(2)).exp();
        job.append(c.encode_utf8(&mut [0; 4]), 0.0, TextFormat::simple(Theme::small(), Theme::mix(t.fg_muted, t.fg_text, k)));
    }
    ui.label(job);
    ui.ctx().request_repaint();
}

/// A command's label is an operator (`mixer.set {…}`): it changes the project.
fn edits(label: &str) -> bool {
    label.split_whitespace().next().is_some_and(|op| op.contains('.'))
}

impl Conversation {
    /// The command running in the reply on its way, if any.
    fn running_step(&self) -> Option<&Step> {
        self.run.as_ref()?;
        match self.items.last() {
            Some(Item::Steps(steps)) => steps.iter().rev().find(|s| s.state == StepState::Running),
            _ => None,
        }
    }

    /// Takes in what a reply said; true once it's done.
    fn take(&mut self) -> bool {
        let Some(run) = &self.run else { return false };
        let (mut events, mut done) = (Vec::new(), false);
        loop {
            match run.events.try_recv() {
                Ok(e) => events.push(e),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    done = true;
                    break;
                }
            }
        }
        for event in events {
            match event {
                Event::Session(id) => self.session = Some(id),
                Event::History(h) => self.history = h,
                Event::Text(text) => self.items.push(Item::Text { text, shown: 0 }),
                Event::Tool(id, label) => {
                    let step = Step { id, label, state: StepState::Running, error: None };
                    match self.items.last_mut() {
                        Some(Item::Steps(steps)) => steps.push(step),
                        _ => self.items.push(Item::Steps(vec![step])),
                    }
                }
                Event::ToolDone(id, error) => {
                    let step = self.items.iter_mut().rev().filter_map(|i| if let Item::Steps(s) = i { Some(s) } else { None }).flatten().find(|s| s.id == id);
                    if let Some(step) = step {
                        step.state = if error.is_none() { StepState::Done } else { StepState::Failed };
                        // an error can run long: the start says what's wrong
                        step.error = error.map(|e| if e.chars().count() > 600 { format!("{}…", e.chars().take(600).collect::<String>()) } else { e });
                    }
                }
                // a failed reply says it once as text and once as the result
                Event::Problem(p) => {
                    if matches!(self.items.last(), Some(Item::Text { text, .. }) if *text == p) {
                        self.items.pop();
                    }
                    self.items.push(Item::Problem(p));
                }
            }
        }
        done
    }

    /// Where the last message is.
    fn last_message(&self) -> Option<usize> {
        self.items.iter().rposition(|i| matches!(i, Item::User(_)))
    }

    /// The items of the last reply (after the last message).
    fn last_reply(&self) -> &[Item] {
        &self.items[self.last_message().map_or(0, |i| i + 1)..]
    }

    /// The last message, to send again, once its reply has ended badly: a problem, or a
    /// command that failed.
    fn retry(&self) -> Option<&Message> {
        let failed = |i: &Item| matches!(i, Item::Problem(_)) || matches!(i, Item::Steps(steps) if steps.iter().any(|s| s.state == StepState::Failed));
        if self.run.is_some() || !self.last_reply().iter().any(failed) {
            return None;
        }
        self.items.iter().rev().find_map(|i| if let Item::User(m) = i { Some(m) } else { None })
    }
}

impl App {
    fn chats_file() -> Option<PathBuf> {
        crate::files::data_dir().map(|d| d.join(FILE))
    }

    /// The conversations from last time, once.
    fn load_chats(&mut self) {
        if std::mem::replace(&mut self.chat.loaded, true) {
            return;
        }
        if let Some(chats) = Self::chats_file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| serde_json::from_str::<Vec<Conversation>>(&t).ok()) {
            self.chat.chats = chats;
        }
    }

    fn save_chats(&self) {
        if !self.chat.loaded {
            return;
        }
        let keep: Vec<&Conversation> = self.chat.chats.iter().filter(|c| !c.items.is_empty()).take(KEEP).collect();
        if let (Some(file), Ok(text)) = (Self::chats_file(), serde_json::to_string(&keep)) {
            // a temp file first: a crash mid-write never loses the chats
            let tmp = file.with_extension("json-tmp");
            let _ = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &file));
        }
        // the pictures nothing shows any more go: no conversation, not the draft, not the chat
        // an Undo can bring back
        let chats = self.chat.chats.iter().chain(self.chat.deleted.as_ref().map(|d| &d.1));
        let sent = chats.flat_map(|c| &c.items).filter_map(|i| if let Item::User(m) = i { Some(&m.pictures) } else { None }).flatten();
        let kept: HashSet<&PathBuf> = sent.chain(&self.chat.pictures).collect();
        for file in std::fs::read_dir(ai::pictures_dir()).into_iter().flatten().flatten() {
            if !kept.contains(&file.path()) {
                let _ = std::fs::remove_file(file.path());
            }
        }
    }

    /// The conversation on screen (there's always one).
    fn conversation(&mut self) -> &mut Conversation {
        if self.chat.chats.is_empty() {
            self.chat.chats.push(Conversation { provider: self.settings.ai.provider, ..Default::default() });
        }
        self.chat.current = self.chat.current.min(self.chat.chats.len() - 1);
        &mut self.chat.chats[self.chat.current]
    }

    /// A fresh conversation, first in the list (an empty one is reused).
    fn new_chat(&mut self) {
        self.load_chats();
        if !self.chat.chats.first().is_some_and(|c| c.items.is_empty()) {
            self.chat.chats.insert(0, Conversation { provider: self.settings.ai.provider, ..Default::default() });
        }
        self.chat.current = 0;
        self.chat.guide = false;
        self.chat.focus = true;
    }

    /// The orb's state: waiting, thinking, reading, listening, or changing things.
    fn orb_state(&self) -> State {
        match self.chat.chats.iter().find(|c| c.run.is_some()) {
            None => State::Breathing,
            Some(c) => match c.running_step().map(|s| s.label.as_str()) {
                None => State::Working,
                Some(label) if label.starts_with("render") => State::Listening,
                Some(label) if edits(label) => State::Composing,
                Some(_) => State::Searching,
            },
        }
    }

    /// Sends `text` and its `pictures` with the AI chosen under the box. The agents' server
    /// comes on if it was off: every AI goes through it.
    fn chat_send(&mut self, ctx: &egui::Context, text: String, pictures: Vec<PathBuf>) {
        let text = text.trim().to_string();
        if text.is_empty() || self.chat.busy() {
            return;
        }
        self.load_chats();
        self.settings.agent.enabled = true;
        // what the user changed so far stays an undo step of its own
        self.end_agent_group();
        let prefs = self.settings.ai.clone();
        let project = self.session.project.clone();
        let c = self.conversation();
        // another AI doesn't know what this one said: it starts fresh, the conversation stays
        if c.provider != prefs.provider && (c.session.is_some() || !c.history.is_empty()) {
            c.items.push(Item::Note(tr!("{} takes over: it doesn't see what was said before.", prefs.provider.name())));
            (c.session, c.history) = (None, Vec::new());
        }
        c.provider = prefs.provider;
        if c.title.is_empty() {
            c.title = text.chars().take(48).collect();
        }
        c.items.push(Item::User(Message { text: text.clone(), pictures: pictures.clone() }));
        // a model that doesn't look at pictures gets the words alone
        let pictures = if pictures.is_empty() || prefs.provider.sees_images(&prefs) {
            pictures
        } else {
            c.items.push(Item::Note(tr!("{} doesn't look at pictures: they weren't sent.", who(&prefs, &[]))));
            Vec::new()
        };
        c.updated = now();
        match ai::start(&prefs, prefs.provider, &text, &pictures, c.session.as_deref(), c.history.clone(), ctx.clone()) {
            Ok(run) => {
                c.run = Some(run);
                c.started = Some(Instant::now());
                (c.before, c.after) = (Some(project), None);
            }
            Err(e) => {
                c.items.push(Item::Problem(e));
                c.items.push(Item::End { seconds: 0, steps: 0 });
            }
        }
        // the newest conversation first
        let current = self.chat.current;
        if current > 0 {
            let c = self.chat.chats.remove(current);
            self.chat.chats.insert(0, c);
            self.chat.current = 0;
        }
        self.chat.guide = false;
        self.chat.jump = true;
        self.save_chats();
    }

    /// Takes in what the reply on its way said (every frame, the chat open or not).
    pub fn chat_poll(&mut self) {
        let Some(i) = self.chat.chats.iter().position(|c| c.run.is_some()) else { return };
        if !self.chat.chats[i].take() {
            return;
        }
        let project = self.session.project.clone();
        let c = &mut self.chat.chats[i];
        c.run = None;
        for item in &mut c.items {
            if let Item::Steps(steps) = item {
                steps.iter_mut().filter(|s| s.state == StepState::Running).for_each(|s| s.state = StepState::Done);
            }
        }
        let reply = c.last_reply();
        let steps = reply.iter().map(|i| if let Item::Steps(s) = i { s.len() } else { 0 }).sum();
        if !reply.iter().any(|i| matches!(i, Item::Text { .. } | Item::Problem(_))) {
            c.items.push(Item::Problem(tr!("{} stopped without an answer.", c.provider.name())));
        }
        c.items.push(Item::End { seconds: c.started.take().map_or(0, |s| s.elapsed().as_secs() as u32), steps });
        c.updated = now();
        // what the reply changed, undoable as a whole while nothing else changes
        if c.before.as_ref().is_some_and(|b| *b != project) {
            c.after = Some(project);
        }
        self.end_agent_group();
        self.save_chats();
    }

    /// Takes back everything the last reply of conversation `i` changed.
    fn undo_reply(&mut self, i: usize) {
        let Some(before) = self.chat.chats.get_mut(i).and_then(|c| c.before.take()) else { return };
        self.chat.chats[i].after = None;
        self.end_agent_group();
        for _ in 0..64 {
            if self.session.project == before || self.session.run("edit.undo", json!({})).is_err() {
                break;
            }
        }
    }

    // ---- pictures ----------------------------------------------------------------------------

    /// A picture goes with the next message.
    fn chat_attach(&mut self, picture: Result<image::DynamicImage, String>, name: &str) {
        match picture.and_then(ai::keep_picture) {
            Ok(path) if !self.chat.pictures.contains(&path) => self.chat.pictures.push(path),
            Ok(_) => {}
            Err(e) => self.say_as(Tone::Error, tr!("Couldn't attach {}: {}", name, e)),
        }
        self.chat.focus = true;
    }

    /// Pictures from files go with the next message (shrunk, kept in Ravune's data folder).
    fn chat_attach_files(&mut self, files: &[PathBuf]) {
        for file in files {
            let name = file.file_name().map_or_else(|| file.display().to_string(), |n| n.to_string_lossy().to_string());
            self.chat_attach(image::open(file).map_err(|e| e.to_string()), &name);
        }
    }

    /// Files dropped on the window: the pictures dropped on the open chat go with the next
    /// message; the rest is for the other panels.
    pub fn chat_take_drop(&mut self, at: Option<Pos2>, dropped: Vec<PathBuf>) -> Vec<PathBuf> {
        if dropped.is_empty() || !self.settings.show_chat || !self.chat.rect.zip(at).is_some_and(|(r, p)| r.contains(p)) {
            return dropped;
        }
        let (pictures, rest): (Vec<PathBuf>, Vec<PathBuf>) = dropped.into_iter().partition(|p| is_picture(p));
        self.chat_attach_files(&pictures);
        rest
    }

    /// A picture as a texture, made once (None: it can't be read, gone or broken).
    fn thumb(&mut self, ctx: &egui::Context, path: &Path) -> Option<TextureHandle> {
        self.chat
            .thumbs
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                let picture = image::open(path).ok()?.thumbnail(320, 320).to_rgba8();
                let image = egui::ColorImage::from_rgba_unmultiplied([picture.width() as usize, picture.height() as usize], &picture);
                Some(ctx.load_texture(path.to_string_lossy(), image, egui::TextureOptions::LINEAR))
            })
            .clone()
    }

    // ---- what workers find out ---------------------------------------------------------------

    fn chat_work(&mut self, ctx: &egui::Context, job: impl FnOnce() -> News + Send + 'static) {
        let tx = self.chat.news.get_or_insert_with(channel).0.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
    }

    /// Tries a provider (installed and signed in, its endpoint answering); for an endpoint,
    /// asks for its models too.
    fn chat_check(&mut self, ctx: &egui::Context, provider: Provider) {
        if provider == Provider::Local {
            self.chat_models(ctx);
        }
        if self.chat.checking.contains(&provider) {
            return;
        }
        self.chat.checking.push(provider);
        let prefs = self.settings.ai.clone();
        self.chat_work(ctx, move || News::Checked(provider, ai::check(&prefs, provider)));
    }

    fn chat_models(&mut self, ctx: &egui::Context) {
        self.chat.models = None;
        let endpoint = self.settings.ai.endpoint.clone();
        self.chat_work(ctx, move || {
            let models = ai::models(&endpoint);
            News::Models(endpoint, models)
        });
    }

    fn chat_news(&mut self) {
        let Some((_, rx)) = &self.chat.news else { return };
        let news: Vec<News> = rx.try_iter().collect();
        for n in news {
            match n {
                News::Checked(p, result) => {
                    self.chat.checking.retain(|c| *c != p);
                    self.chat.checks.retain(|c| c.0 != p);
                    self.chat.checks.push((p, result));
                }
                News::Models(endpoint, models) if endpoint == self.settings.ai.endpoint => {
                    // no model chosen yet: the first it offers
                    let ai = &mut self.settings.ai;
                    if let (Ok(list), true) = (&models, ai.model.trim().is_empty())
                        && let Some(first) = list.first()
                    {
                        ai.model = first.clone();
                        ai.sees_images = ai::sees(first);
                    }
                    self.chat.models = Some(models);
                }
                News::Models(..) => {}
                News::Found(found) => self.chat.found = Some(found),
            }
        }
    }

    /// The top bar's button for the chat: a small orb in a square.
    pub fn chat_toggle(&mut self, ui: &mut Ui, t: &Theme) {
        let (rect, r) = ui.allocate_exact_size(vec2(26.0, 24.0), Sense::click());
        let k = kit::hover(ui, t, &r);
        let open = self.settings.show_chat;
        if open || k > 0.0 {
            let fill = if open { Theme::mix(t.bg_control, t.accent, 0.16 + 0.08 * k) } else { t.bg_control.gamma_multiply(k) };
            ui.painter().rect_filled(rect, t.radius_md, fill);
        }
        let ink = if open || self.chat.busy() { t.accent } else { Theme::mix(t.fg_muted, t.fg_text, k) };
        orb::paint(ui.painter(), rect.center(), 18.0, self.orb_state(), seconds(), ink);
        if self.chat.busy() {
            ui.ctx().request_repaint();
        }
        kit::focus_ring(ui, t, &r, t.radius_md);
        if r.on_hover_text(self.tip(tr!("AI chat"), "app.chat")).clicked() {
            self.command("app.chat");
        }
    }

    /// The chat: the conversation, scrolling under a floating prompt bar.
    pub fn chat_panel(&mut self, ui: &mut Ui) {
        self.chat_undo_delete();
        self.load_chats();
        self.chat_news();
        // the provider chosen is tried once: whether it's ready shows while the chat is empty
        let provider = self.settings.ai.provider;
        if self.chat.check(provider).is_none() && !self.chat.checking.contains(&provider) {
            self.chat_check(ui.ctx(), provider);
        }
        self.chat.rect = Some(ui.max_rect());
        let t = self.theme.clone();
        let state = self.orb_state();
        let title = self.conversation().title.clone();
        kit::region(ui);
        kit::bar(ui, &t, |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
            orb::paint(ui.painter(), rect.center(), 20.0, state, seconds(), t.accent);
            kit::right(ui, |ui| {
                if kit::icon_button(ui, &t, icons::CARET_RIGHT, &self.tip(tr!("Hide the chat"), "app.chat"), false).clicked() {
                    self.settings.show_chat = false;
                }
                if kit::icon_button(ui, &t, icons::PLUS, tr!("New chat"), false).clicked() {
                    self.new_chat();
                }
                let history = kit::icon_button(ui, &t, icons::CHAT_CIRCLE_TEXT, tr!("Chats"), false);
                egui::Popup::menu(&history).width(300.0).show(|ui| self.chats_menu(ui, &t));
                let name = if title.is_empty() { tr!("New chat").to_string() } else { title.clone() };
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| ui.add(Label::new(RichText::new(name).color(t.fg_text)).truncate()));
            });
        });
        let area = ui.available_rect_before_wrap();
        let bar_h = if self.chat.bar_h > 0.0 { self.chat.bar_h } else { 96.0 };
        let bar_top = area.bottom() - t.pad - bar_h;
        // the conversation, under the prompt bar: it follows its newest message; a new chat
        // reads from the top
        let empty = self.conversation().items.is_empty();
        let scroll = egui::ScrollArea::vertical().id_salt(("chat", self.chat.current)).auto_shrink(false).stick_to_bottom(!empty);
        let jump = std::mem::take(&mut self.chat.jump);
        let mut actions = Actions::default();
        let out = ui.scope_builder(UiBuilder::new().max_rect(area), |ui| {
            scroll.show(ui, |ui| {
                let side = t.pad as i8 + 4;
                Frame::new().inner_margin(egui::Margin { left: side, right: side, top: t.pad as i8, bottom: 0 }).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 10.0;
                    self.conversation_ui(ui, &t, state, &mut actions);
                    ui.add_space(bar_h + t.pad * 3.0);
                    // the end of the conversation, to scroll to
                    let end = ui.allocate_response(vec2(1.0, 1.0), Sense::hover());
                    if jump {
                        end.scroll_to_me(Some(Align::BOTTOM));
                    }
                });
            })
        });
        let scrolled = out.inner;
        let at_bottom = scrolled.state.clamped_offset().y + scrolled.inner_rect.height() >= scrolled.content_size.y - 4.0;
        // the fade: the conversation melts into the panel's color above the box
        let painter = ui.painter_at(area);
        let fade = Rect::from_min_max(pos2(area.left(), bar_top - 36.0), pos2(area.right(), bar_top));
        let mut mesh = egui::Mesh::default();
        for (p, c) in [(fade.left_top(), Theme::CLEAR), (fade.right_top(), Theme::CLEAR), (fade.right_bottom(), t.bg_panel), (fade.left_bottom(), t.bg_panel)] {
            mesh.colored_vertex(p, c);
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
        painter.rect_filled(Rect::from_min_max(pos2(area.left(), bar_top), area.max), 0, t.bg_panel);
        // back to the newest message (AI Elements' scroll button), square
        if !at_bottom {
            let at = pos2(area.center().x, bar_top - 22.0);
            let button = Rect::from_center_size(at, vec2(26.0, 26.0));
            let r = ui.interact(button, egui::Id::unique("chat-bottom"), Sense::click());
            let k = kit::hover(ui, &t, &r);
            painter.rect_filled(button, t.radius_md, Theme::mix(t.bg_device, t.bg_control, k));
            painter.rect_stroke(button, t.radius_md, Stroke::new(1.0, t.line), StrokeKind::Inside);
            arrow(&painter, at, t.fg_text, true);
            if r.on_hover_text(tr!("Newest message")).clicked() {
                self.chat.jump = true;
            }
        }
        // the prompt bar, floating at the bottom
        let bar = Rect::from_min_max(pos2(area.left() + t.pad, bar_top), pos2(area.right() - t.pad, area.bottom() - t.pad));
        let mut child = ui.new_child(UiBuilder::new().max_rect(bar).layout(Layout::top_down(Align::Min)));
        self.prompt_bar(&mut child, &t);
        self.chat.bar_h = child.min_rect().height();
        // pictures dragged in from another app: the chat says it takes them
        let panel = ui.max_rect();
        if ui.ctx().input(|i| !i.raw.hovered_files.is_empty()) && crate::os_cursor(ui.ctx()).is_some_and(|p| panel.contains(p)) {
            let over = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("chat-drop")));
            let zone = panel.shrink(8.0);
            over.rect_filled(zone, t.radius_md, t.bg_panel.gamma_multiply(0.94));
            over.rect_stroke(zone, t.radius_md, Stroke::new(1.5, t.accent), StrokeKind::Inside);
            kit::icon_at(&over, zone.center() - vec2(0.0, 16.0), icons::FILE_ARROW_DOWN, 28.0, t.accent);
            over.text(zone.center() + vec2(0.0, 14.0), Align2::CENTER_CENTER, tr!("Drop pictures to send with your message"), Theme::body(), t.fg_text);
            ui.ctx().request_repaint();
        }
        self.apply(ui.ctx(), actions);
    }

    /// The empty chat: the orb, what it does, suggestions, and whether the AI chosen is ready.
    fn empty_ui(&mut self, ui: &mut Ui, t: &Theme, state: State, actions: &mut Actions) {
        ui.add_space(t.pad * 3.0);
        ui.vertical_centered(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::hover());
            orb::paint(ui.painter(), rect.center(), 52.0, state, seconds(), t.fg_text);
            ui.add_space(t.gap);
            ui.label(RichText::new(tr!("Ask for a change")).size(Theme::TEXT_LG).color(t.fg_text));
            ui.label(small(tr!("The AI makes it in your project with Ravune's own commands, and listens to the result. Attach pictures as references. Each reply undoes in one step.")).color(t.fg_muted));
        });
        ui.add_space(t.pad);
        ui.spacing_mut().item_spacing.y = 6.0;
        for example in EXAMPLES {
            let text = crate::i18n::t(example);
            if suggestion(ui, t, text).clicked() {
                actions.send = Some((text.to_string(), std::mem::take(&mut self.chat.pictures)));
            }
        }
        ui.add_space(t.pad);
        // who answers, and whether it's ready
        let provider = self.settings.ai.provider;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            match self.chat.check(provider) {
                None => {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(small(tr!("Trying {}…", who(&self.settings.ai, &self.chat.codex_models))).color(t.fg_muted));
                }
                Some(result) => {
                    let (light, said) = match result {
                        Ok(s) => (t.meter_ok, s),
                        Err(e) => (t.meter_hot, e),
                    };
                    let (rect, _) = ui.allocate_exact_size(vec2(8.0, 16.0), Sense::hover());
                    led(ui.painter(), rect.center(), light);
                    ui.label(small(who(&self.settings.ai, &self.chat.codex_models)).color(t.fg_text));
                    ui.add(Label::new(small(said).color(t.fg_muted)).wrap());
                }
            }
        });
        ui.ctx().request_repaint(); // the big orb breathes
    }

    /// The messages, the guide or the empty state, and the status of a reply on its way.
    fn conversation_ui(&mut self, ui: &mut Ui, t: &Theme, state: State, actions: &mut Actions) {
        let empty = self.conversation().items.is_empty();
        let current = self.chat.current;
        if empty {
            self.empty_ui(ui, t, state, actions);
        }
        if self.chat.guide || (empty && matches!(self.chat.check(self.settings.ai.provider), Some(Err(_)))) {
            self.guide_ui(ui, t, actions);
        }
        // the conversation's pictures, as textures
        let ctx = ui.ctx().clone();
        let paths: Vec<PathBuf> = self.chat.chats[current].items.iter().filter_map(|i| if let Item::User(m) = i { Some(m.pictures.clone()) } else { None }).flatten().collect();
        let thumbs: HashMap<PathBuf, Option<TextureHandle>> = paths.into_iter().map(|p| (p.clone(), self.thumb(&ctx, &p))).collect();
        // the typed-in text moves at ~90 characters a second, faster when far behind
        let dt = self.chat.typed.replace(Instant::now()).map_or(0.0, |at| at.elapsed().as_secs_f32()).min(0.1);
        let width = ui.available_width();
        let changed = self.chat.chats[current].after.as_ref().is_some_and(|a| *a == self.session.project);
        let c = &mut self.chat.chats[current];
        let open = &mut self.chat.open;
        let last_end = c.items.iter().rposition(|i| matches!(i, Item::End { .. }));
        let last_message = c.last_message();
        let retry = c.retry().cloned();
        // a reply that ended in a problem offers its retry on the problem's card
        let retry_on_card = c.last_reply().iter().any(|i| matches!(i, Item::Problem(_)));
        let provider = c.provider;
        let (mut typing, mut said) = (false, false);
        for (n, item) in c.items.iter_mut().enumerate() {
            said |= matches!(item, Item::Text { .. });
            match item {
                Item::User(message) => {
                    said = false;
                    // a new exchange: room above it (AI Elements' Message, py-4)
                    ui.add_space(if n == 0 { 4.0 } else { 18.0 });
                    if !message.pictures.is_empty() {
                        ui.with_layout(Layout::right_to_left(Align::Min).with_main_wrap(true), |ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                            for p in message.pictures.iter().rev() {
                                picture(ui, t, thumbs.get(p).cloned().flatten().as_ref(), 72.0);
                            }
                        });
                    }
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        Frame::new().fill(t.bg_control).corner_radius(CornerRadius::same(t.radius_md)).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                            ui.set_max_width(width * 0.85);
                            // the bubble sits on the right, its text reads from the left
                            ui.with_layout(Layout::top_down(Align::Min), |ui| prose(ui, &message.text, t.fg_text, true));
                        });
                    });
                    ui.add_space(4.0);
                }
                Item::Text { text, shown } => {
                    let total = text.chars().count();
                    if typing {
                        *shown = 0; // one text types in at a time, in order
                    } else if *shown < total {
                        let behind = (total - *shown) as f32;
                        *shown = (*shown + ((90.0 + behind * 2.0) * dt).ceil() as usize).min(total);
                        typing = true;
                    }
                    if *shown > 0 {
                        let visible: String = if *shown >= total { text.clone() } else { text.chars().take(*shown).collect() };
                        actions.copied |= markdown::show(ui, t, &visible, (current, n));
                    }
                }
                Item::Steps(steps) => {
                    // a row (AI Elements' Tool), and the commands under it when open
                    let key = (c.updated, n);
                    let is_open = open.contains(&key);
                    if steps_row(ui, t, steps, is_open).clicked() {
                        if is_open {
                            open.remove(&key);
                        } else {
                            open.insert(key);
                        }
                    }
                    if is_open {
                        let list = Frame::new().inner_margin(egui::Margin { left: 18, right: 4, top: 0, bottom: 2 }).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            for step in steps.iter() {
                                step_line(ui, t, step);
                            }
                        });
                        let rect = list.response.rect;
                        ui.painter().vline(rect.left() + 9.0, rect.y_range(), Stroke::new(1.0, t.line));
                    }
                }
                Item::Problem(text) => {
                    let last = last_message.is_some_and(|m| n > m);
                    let card = problem_card(ui, t, provider, text, retry.is_some() && last);
                    if card.retry {
                        actions.send = retry.as_ref().map(|m| (m.text.clone(), m.pictures.clone()));
                    }
                    actions.terminal |= card.terminal;
                    if card.copy {
                        ui.ctx().copy_text(text.clone());
                        actions.copied = true;
                    }
                }
                Item::Note(text) => {
                    ui.vertical_centered(|ui| ui.add(Label::new(small(text.as_str()).italics().color(t.fg_muted)).wrap()));
                }
                Item::End { seconds, steps } => {
                    ui.horizontal(|ui| {
                        let done = match *steps {
                            0 => tr!("{} s", seconds),
                            1 => tr!("{} s · 1 step", seconds),
                            n => tr!("{} s · {} steps", seconds, n),
                        };
                        ui.label(RichText::new(done).font(Theme::mono(Theme::TEXT_XS)).color(t.fg_muted));
                        kit::right(ui, |ui| {
                            if said && kit::icon_button(ui, t, icons::COPY, tr!("Copy the reply"), false).clicked() {
                                actions.copy = Some((current, n));
                            }
                            // a reply whose command failed: the same message once more
                            if let Some(m) = retry.as_ref().filter(|_| Some(n) == last_end && !retry_on_card)
                                && kit::action(ui, t, kit::Variant::Outline, Some(icons::ARROW_CLOCKWISE), tr!("Retry")).on_hover_text(tr!("Send the same message again")).clicked()
                            {
                                actions.send = Some((m.text.clone(), m.pictures.clone()));
                            }
                            if Some(n) == last_end && changed && kit::action(ui, t, kit::Variant::Ghost, Some(icons::ARROW_U_UP_LEFT), tr!("Undo")).on_hover_text(tr!("Take back everything this reply changed")).clicked() {
                                actions.undo = Some(current);
                            }
                        });
                    });
                }
            }
        }
        // a reply on its way: the orb, what it's doing, for how long (RareUI's AI Status)
        if c.run.is_some() {
            let doing = match c.running_step() {
                Some(s) if s.label.starts_with("render") => tr!("Listening").to_string(),
                Some(s) if s.label.starts_with("browse") => tr!("Looking for sounds").to_string(),
                Some(s) if !edits(&s.label) => tr!("Reading the project").to_string(),
                Some(s) => tr!("Editing · {}", s.label.split_whitespace().next().unwrap_or("")),
                None => tr!("Thinking").to_string(),
            };
            let elapsed = c.started.map_or(0, |s| s.elapsed().as_secs());
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (rect, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                orb::paint(ui.painter(), rect.center(), 24.0, state, seconds(), t.accent);
                shimmer(ui, t, &format!("{doing}…"));
                ui.label(RichText::new(format!("{elapsed}s")).font(Theme::mono(Theme::TEXT_XS)).color(t.fg_muted));
            });
        }
        if typing {
            ui.ctx().request_repaint();
        }
    }

    /// How to connect each AI, with what Ravune finds on this computer: plain text, as a reply
    /// reads, with the commands to copy and the buttons that do the steps.
    fn guide_ui(&mut self, ui: &mut Ui, t: &Theme, actions: &mut Actions) {
        ui.add_space(t.pad);
        ui.horizontal(|ui| {
            kit::caps(ui, t, tr!("CONNECT AN AI"));
            if self.chat.guide {
                kit::right(ui, |ui| {
                    if kit::icon_button(ui, t, icons::X, tr!("Close"), false).clicked() {
                        actions.close_guide = true;
                    }
                });
            }
        });
        prose(ui, tr!("Ravune answers through an AI that's already on this computer, or one you point it at. Here's what it found."), t.fg_muted, true);
        ui.add_space(t.gap);
        for provider in Provider::ALL {
            let found = self.chat.found.as_ref().and_then(|f| f.first()).map(|(i, m)| (ai::PRESETS[*i].0, m.len()));
            let (status, ready) = match (provider, self.chat.check(provider)) {
                (_, Some(Ok(said))) => (said.clone(), true),
                (Provider::Local, _) if let Some((name, n)) = found => (tr!("{} is running · {} models", name, n), true),
                (_, Some(Err(e))) => (e.clone(), false),
                (Provider::Local, None) => (self.settings.ai.endpoint.clone(), false),
                (p, None) => match p.program().and_then(ai::found) {
                    Some(path) => (path.display().to_string(), true),
                    None => (tr!("not installed").to_string(), false),
                },
            };
            let steps = match provider {
                Provider::ClaudeCode if ready => tr!("Ready. If it says it's signed out: run claude auth login in a terminal. The claude command signs in on its own, apart from the Claude app."),
                Provider::Codex if ready => tr!("Ready. If it asks you to sign in: run codex login in a terminal."),
                Provider::Local if ready => tr!("Answering: pick it and one of its models in the menu under the box."),
                Provider::ClaudeCode => tr!(
                    "1. Install it from claude.com/claude-code.\n2. In the Terminal: claude auth login, and sign in with your Claude account in the browser.\n3. Check with claude auth status: it should say \"loggedIn\": true.\n4. Come back here and send your message again.\nThe claude command keeps its own sign-in: being signed in to the Claude app isn't enough. Ravune runs it with nothing but Ravune's tools."
                ),
                Provider::Codex => tr!("1. In the Terminal: npm install -g @openai/codex\n2. Then: codex login, and sign in with ChatGPT.\nRavune runs it in a read-only sandbox with Ravune's tools."),
                Provider::Local => tr!(
                    "On this computer: install Ollama (ollama.com) or LM Studio and get a model that can use tools (ollama pull qwen2.5:14b; small 7B models often call them wrong). Ravune finds them while they run. Free, and nothing leaves this computer.\n\nIn the cloud: pick OpenAI, Anthropic, Gemini, OpenRouter or Groq in the menu under the box, paste its key (kept in the system keychain), then Test the connection."
                ),
            };
            let mut copied = None;
            guide_card(ui, t, provider.name(), &status, Some(ready), |ui| {
                prose(ui, steps, t.fg_muted, true);
                if provider != Provider::Local {
                    let command = if provider == Provider::ClaudeCode { "claude auth login" } else { "codex login" };
                    let mut buttons = Vec::new();
                    if TERMINAL {
                        buttons.push((kit::Variant::Outline, None, tr!("Open the Terminal")));
                    }
                    buttons.push((kit::Variant::Outline, None, tr!("Copy the command")));
                    let shown = kit::actions(ui, t, &buttons);
                    if TERMINAL && shown[0].clicked() {
                        actions.terminal = true;
                    }
                    if shown.last().is_some_and(egui::Response::clicked) {
                        copied = Some(command.to_string());
                    }
                }
            });
            if let Some(text) = copied {
                ui.ctx().copy_text(text);
                self.say_as(Tone::Done, tr!("Copied"));
            }
        }
        // other apps: the same tools, through the bridge
        let (command, json) = (crate::agent::claude_command(), crate::agent::mcp_json());
        let (mut copied, mut let_in) = (None, false);
        let agents = self.settings.agent.enabled;
        guide_card(ui, t, tr!("Ravune in Claude, Cursor"), if agents { tr!("agents let in") } else { tr!("agents kept out") }, Some(agents), |ui| {
            prose(ui, tr!("The other way round: those apps drive Ravune from outside, while Ravune lets agents in (Preferences → Agents)."), t.fg_muted, false);
            if !agents {
                let_in = kit::actions(ui, t, &[(kit::Variant::Secondary, None, tr!("Let agents in"))])[0].clicked();
            }
            // each one's code, and a button under it that copies it
            for (about, text, copy) in [
                (tr!("Claude Code: run this once in a terminal, then start a new session. --scope user adds Ravune in every folder; without it, only in the folder you ran it in."), &command, tr!("Copy the command")),
                (tr!("Claude Desktop (Settings → Developer → Edit Config), Cursor and other MCP apps: add this to their MCP servers, then restart them."), &json, tr!("Copy the JSON")),
            ] {
                prose(ui, about, t.fg_muted, false);
                Frame::new().fill(t.bg_field).corner_radius(t.radius_sm).inner_margin(t.pad).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(Label::new(RichText::new(text).font(Theme::mono(Theme::TEXT_XS)).color(t.fg_text)).wrap());
                });
                if kit::actions(ui, t, &[(kit::Variant::Outline, None, copy)])[0].clicked() {
                    copied = Some(text);
                }
            }
        });
        if let Some(text) = copied {
            ui.ctx().copy_text(text.clone());
            self.say_as(Tone::Done, tr!("Copied"));
        }
        if let_in {
            self.settings.agent.enabled = true;
        }
    }

    /// The list of conversations, newest first: a row each (its bin deletes it, with an Undo
    /// on the toast), and a button to hold for deleting them all.
    fn chats_menu(&mut self, ui: &mut Ui, t: &Theme) {
        if ui.button(format!("{}  {}", icons::PLUS, tr!("New chat"))).clicked() {
            self.new_chat();
            ui.close();
        }
        kit::menu_line(ui, t);
        let rows: Vec<(usize, String, String, bool)> =
            self.chat.chats.iter().enumerate().filter(|(_, c)| !c.items.is_empty()).map(|(i, c)| (i, c.title.clone(), format!("{} · {}", c.provider.name(), ago(c.updated)), c.run.is_some())).collect();
        if rows.is_empty() {
            ui.label(RichText::new(tr!("No chats yet")).small().color(t.fg_muted));
        }
        let (mut pick, mut remove) = (None, None);
        let any = !rows.is_empty();
        for (i, title, about, running) in rows {
            let row = kit::row(ui, t, egui::Id::unique(("chat-row", i, &title)), i == self.chat.current);
            let r = row.rect;
            // the bin on the right, over the row
            let bin = Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), vec2(22.0, 20.0));
            let b = (!running).then(|| ui.interact(bin, row.id.with("bin"), Sense::click()).on_hover_text(tr!("Delete")));
            let painter = ui.painter();
            let right = if b.is_some() { bin.left() - 6.0 } else { r.right() - 8.0 };
            let (when, _) = kit::fit_text(painter, pos2(right, r.center().y), Align2::RIGHT_CENTER, ago(self.chat.chats[i].updated), Theme::mono(Theme::TEXT_XS), t.fg_muted, 90.0);
            kit::fit_text(painter, pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, title, Theme::body(), t.fg_text, when.left() - r.left() - 18.0);
            if let Some(b) = &b {
                let ink = if b.hovered() {
                    t.meter_hot
                } else if row.hovered() {
                    t.fg_muted
                } else {
                    t.fg_muted.gamma_multiply(0.5)
                };
                kit::icon_at(painter, bin.center(), icons::TRASH, 14.0, ink);
            }
            if b.as_ref().is_some_and(|b| b.clicked()) {
                remove = Some(i);
            } else if row.on_hover_text(&about).clicked() {
                pick = Some(i);
            }
        }
        let mut all = false;
        if any {
            kit::menu_line(ui, t);
            Frame::new().inner_margin(egui::Margin::symmetric(t.pad as i8, (t.gap * 1.5) as i8)).show(ui, |ui| {
                all = kit::hold_button(ui, t, Some(icons::TRASH), tr!("Hold to delete all"), tr!("Deleted"), 1.2);
            });
            if all {
                // the one answering stays
                self.chat.chats.retain(|c| c.run.is_some());
                if self.chat.chats.is_empty() {
                    self.chat.chats.push(Conversation::default());
                }
                self.chat.current = self.chat.chats.len() - 1;
                self.chat.deleted = None;
                self.save_chats();
            }
        }
        if let Some(i) = pick {
            self.chat.current = i;
            self.chat.guide = false;
            self.chat.jump = true;
            ui.close();
        }
        if let Some(i) = remove {
            let gone = self.chat.chats.remove(i);
            if self.chat.chats.is_empty() {
                self.chat.chats.push(Conversation::default());
            }
            if self.chat.current > i || self.chat.current >= self.chat.chats.len() {
                self.chat.current = self.chat.current.saturating_sub(1);
            }
            self.chat.deleted = Some((i, gone));
            self.save_chats();
            self.say_undo(tr!("Chat deleted"), "chat");
        }
    }

    /// The toast's Undo on a deleted chat: it comes back where it was.
    fn chat_undo_delete(&mut self) {
        if self.toast_acted.take_if(|a| *a == "chat").is_none() {
            return;
        }
        if let Some((i, chat)) = self.chat.deleted.take() {
            let i = i.min(self.chat.chats.len());
            self.chat.chats.insert(i, chat);
            self.chat.current = i;
            self.save_chats();
        }
    }

    fn apply(&mut self, ctx: &egui::Context, actions: Actions) {
        if let Some((text, pictures)) = actions.send {
            self.chat_send(ctx, text, pictures);
        }
        if let Some(i) = actions.undo {
            self.undo_reply(i);
        }
        if let Some((i, end)) = actions.copy {
            let c = &self.chat.chats[i];
            let from = c.items[..end].iter().rposition(|i| matches!(i, Item::User(_))).map_or(0, |i| i + 1);
            let text: Vec<&str> = c.items[from..end].iter().filter_map(|i| if let Item::Text { text, .. } = i { Some(text.as_str()) } else { None }).collect();
            ctx.copy_text(text.join("\n\n"));
            self.say_as(Tone::Done, tr!("Copied"));
        } else if actions.copied {
            self.say_as(Tone::Done, tr!("Copied"));
        }
        if actions.close_guide {
            self.chat.guide = false;
        }
        if actions.terminal {
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open").args(["-a", "Terminal"]).spawn();
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                // in a console of its own (CREATE_NEW_CONSOLE)
                let _ = std::process::Command::new("powershell").creation_flags(0x10).spawn();
            }
        }
    }

    /// The prompt bar (AI Elements' Prompt Input), square: a field that lights up while you
    /// type and grows with the text, the pictures going with it above, then a button to attach
    /// more, which AI answers, how long it thinks, and one button that sends or stops. Enter
    /// sends, Shift+Enter starts a new line; a picture pasted into it is attached.
    fn prompt_bar(&mut self, ui: &mut Ui, t: &Theme) {
        let running = self.chat.busy();
        let id = egui::Id::unique("chat-prompt");
        let focused = ui.ctx().memory(|m| m.has_focus(id));
        let glow = ui.ctx().animate_bool_with_time(id.with("glow"), focused, t.motion);
        let stroke = Stroke::new(1.0, Theme::mix(t.line, t.accent, glow));
        let pasted: Vec<std::sync::Arc<egui::ColorImage>> = if focused { ui.input(|i| i.events.iter().filter_map(|e| if let egui::Event::PasteImage(p) = e { Some(p.clone()) } else { None }).collect()) } else { Vec::new() };
        let ctx = ui.ctx().clone();
        if self.chat.codex_models.is_empty() {
            self.chat.codex_models = ai::codex_models(); // their names, for the button under the box
        }
        let thumbs: Vec<Option<TextureHandle>> = self.chat.pictures.clone().iter().map(|p| self.thumb(&ctx, p)).collect();
        let sees = self.settings.ai.provider.sees_images(&self.settings.ai);
        let (mut clicked, mut enter, mut attach, mut remove) = (false, false, false, None);
        let radius = t.radius_md + 1;
        let frame = Frame::new().fill(t.bg_device).stroke(stroke).corner_radius(CornerRadius::same(radius)).inner_margin(egui::Margin { left: 12, right: 8, top: 10, bottom: 8 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !thumbs.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for (i, tex) in thumbs.iter().enumerate() {
                        let r = picture(ui, t, tex.as_ref(), 52.0);
                        // its × in the corner
                        let x = Rect::from_center_size(r.rect.right_top() + vec2(-9.0, 9.0), vec2(16.0, 16.0));
                        let xr = ui.interact(x, r.id.with("remove"), Sense::click()).on_hover_text(tr!("Remove"));
                        let k = (kit::hover(ui, t, &xr) * 0.5 + if r.hovered() || xr.hovered() { 0.5 } else { 0.25 }).min(1.0);
                        ui.painter().rect_filled(x, t.radius_sm, t.bg_base.gamma_multiply(0.6 + 0.4 * k));
                        kit::icon_at(ui.painter(), x.center(), icons::X, 10.0, t.fg_text.gamma_multiply(0.5 + 0.5 * k));
                        if xr.clicked() {
                            remove = Some(i);
                        }
                    }
                });
                if !sees {
                    ui.add(Label::new(small(tr!("{} doesn't look at pictures: they won't be sent. Pick a model that does in the menu below.", who(&self.settings.ai, &[]))).color(t.meter_warn)).wrap());
                }
                ui.add_space(4.0);
            }
            // as tall as its text, up to a point, then it scrolls
            let edit = egui::TextEdit::multiline(&mut self.chat.draft).id(id).frame(Frame::NONE).hint_text(tr!("Ask for a change…")).desired_rows(2).desired_width(f32::INFINITY).return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter));
            let r = egui::ScrollArea::vertical().id_salt("chat-draft").max_height(200.0).auto_shrink([false, true]).show(ui, |ui| ui.add(edit)).inner;
            if std::mem::take(&mut self.chat.focus) {
                r.request_focus();
            }
            enter = r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
            // under the words: pictures, who answers, how long it thinks; send at the far end
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                attach = kit::icon_button(ui, t, icons::PLUS, tr!("Attach pictures (or drop or paste them)"), false).clicked();
                let effort = (self.settings.ai.provider != Provider::Local).then(|| effort_name(&self.settings.ai.effort));
                let effort_w = effort.map_or(0.0, |e| bar_width(ui, t, e, true, false) + 4.0);
                self.provider_menu(ui, t, effort_w);
                if let Some(effort) = effort {
                    self.effort_menu(ui, t, effort);
                }
                kit::right(ui, |ui| {
                    // one square button: an arrow to send, a square to stop
                    let (rect, b) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                    let ready = running || !self.chat.draft.trim().is_empty();
                    let fill = if ready { t.accent } else { t.bg_control };
                    let ink = if ready { t.bg_base } else { t.fg_muted };
                    let k = kit::hover(ui, t, &b);
                    let shape = if b.is_pointer_button_down_on() { rect.shrink(1.0) } else { rect };
                    ui.painter().rect_filled(shape, t.radius_md, fill.gamma_multiply(1.0 - 0.15 * k));
                    if running {
                        ui.painter().rect_filled(Rect::from_center_size(shape.center(), vec2(9.0, 9.0)), 1, ink);
                    } else {
                        arrow(ui.painter(), shape.center(), ink, false);
                    }
                    kit::focus_ring(ui, t, &b, t.radius_md);
                    clicked = b.on_hover_text(if running { tr!("Stop") } else { tr!("Send (Enter)") }).clicked();
                });
            });
        });
        // the beam rides the border, and follows the pointer over the box; it goes while typing
        let box_rect = frame.response.rect;
        self.chat.composer = Some(box_rect);
        let pointer = ui.ctx().pointer_hover_pos().filter(|p| box_rect.contains(*p));
        let lit = ui.ctx().animate_bool_with_time(id.with("beam"), pointer.is_some(), 0.6);
        let off = ui.ctx().animate_bool_with_time(id.with("beam-off"), focused, 0.3);
        let strength = (0.7 + 0.3 * lit) * (1.0 - off);
        if strength > 0.0 && t.motion > 0.0 {
            let (dt, key) = (ui.input(|i| i.stable_dt).min(0.1), id.with("beam-turn"));
            let was: f32 = ui.data(|d| d.get_temp(key)).unwrap_or(0.0);
            let turn = match pointer {
                // its light (0.68 of a turn past where the gradient starts) eases to the pointer
                Some(p) => {
                    let to = p - box_rect.center();
                    let aim = to.x.atan2(-to.y) - 0.68 * std::f32::consts::TAU;
                    was + ((aim - was + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI) * (dt * 10.0).min(1.0)
                }
                None => was + dt * std::f32::consts::TAU / 1.96,
            };
            ui.data_mut(|d| d.insert_temp(key, turn));
            beam(ui.painter(), t, box_rect, radius as f32, strength, turn);
            ui.ctx().request_repaint();
        }
        if let Some(i) = remove {
            self.chat.pictures.remove(i);
        }
        for p in pasted {
            let rgba: Vec<u8> = p.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
            let picture = image::RgbaImage::from_raw(p.size[0] as u32, p.size[1] as u32, rgba).map(image::DynamicImage::ImageRgba8).ok_or_else(|| tr!("not a picture").to_string());
            self.chat_attach(picture, tr!("the pasted picture"));
        }
        if attach && let Some(files) = rfd::FileDialog::new().add_filter(tr!("Pictures"), &PICTURES).pick_files() {
            self.chat_attach_files(&files);
        }
        if running && clicked {
            if let Some(c) = self.chat.chats.iter_mut().find(|c| c.run.is_some()) {
                c.run = None;
                c.items.push(Item::Note(tr!("Stopped").into()));
                c.items.push(Item::End { seconds: c.started.take().map_or(0, |s| s.elapsed().as_secs() as u32), steps: 0 });
            }
            self.end_agent_group();
            self.save_chats();
        } else if !running && (clicked || enter) && !self.chat.draft.trim().is_empty() {
            let (text, pictures) = (std::mem::take(&mut self.chat.draft), std::mem::take(&mut self.chat.pictures));
            self.chat_send(ui.ctx(), text, pictures);
            ui.ctx().memory_mut(|m| m.request_focus(id));
        }
    }

    /// Which AI answers and with which model: a button under the box, as wide as the row leaves
    /// it (`beside`: the effort's button after it). Its menu tries each provider, lists its
    /// models, connects a service in a click and tests it.
    fn provider_menu(&mut self, ui: &mut Ui, t: &Theme, beside: f32) {
        let ready = match self.chat.check(self.settings.ai.provider) {
            Some(Ok(_)) => t.meter_ok,
            Some(Err(_)) => t.meter_hot,
            None => t.fg_muted,
        };
        let room = ui.available_width() - beside - Theme::CONTROL_H - t.gap;
        // the model when one is picked, else who answers with its own
        let provider = self.settings.ai.provider;
        let shown = match provider.model(&mut self.settings.ai).trim() {
            "" => who(&self.settings.ai, &[]),
            model => ai::model_name(model, &self.chat.codex_models).to_string(),
        };
        let r = bar_button(ui, t, None, Some(ready), &shown, true, room).on_hover_text(who(&self.settings.ai, &self.chat.codex_models));
        if r.clicked() {
            // fresh each time it opens: the tests, the models, the local servers
            self.chat.has_key = Some(ai::key(&self.settings.ai.endpoint).is_some());
            self.chat.codex_models = ai::codex_models();
            self.chat.filter.clear();
            for p in Provider::ALL {
                self.chat_check(ui.ctx(), p);
            }
            self.chat_work(ui.ctx(), || News::Found(ai::detect()));
        }
        egui::Popup::menu(&r).width(320.0).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            Frame::new().inner_margin(egui::Margin::symmetric(4, 2)).show(ui, |ui| ui.with_layout(Layout::top_down(Align::Min), |ui| self.provider_menu_ui(ui, t)));
        });
    }

    /// How long the model thinks: a button under the box, and over it a slider from faster to
    /// smarter with a stop per effort, as Claude's apps have it.
    fn effort_menu(&mut self, ui: &mut Ui, t: &Theme, name: &str) {
        let r = bar_button(ui, t, Some(icons::SPARKLE), None, name, false, f32::INFINITY).on_hover_text(tr!("Effort: how long the model thinks before it acts"));
        let at = ai::EFFORTS.iter().position(|e| *e == self.settings.ai.effort).unwrap_or(2);
        let mut picked = None;
        // over the box, from its left edge, as it sits at the bottom of the chat
        egui::Popup::menu(&r).anchor(self.chat.composer.unwrap_or(r.rect)).align(egui::RectAlign::TOP_START).gap(8.0).width(250.0).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            Frame::new().inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr!("Effort")).color(t.fg_muted));
                    ui.label(RichText::new(name).color(t.fg_text).strong());
                    kit::right(ui, |ui| {
                        let (rect, i) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                        kit::icon_at(ui.painter(), rect.center(), icons::INFO, 14.0, t.fg_muted);
                        i.on_hover_text(tr!("Faster replies, or smarter ones that take longer. Claude Code and Codex both take it; a local model doesn't."));
                    });
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(small(tr!("Faster")).color(t.fg_muted));
                    kit::right(ui, |ui| ui.label(small(tr!("Smarter")).color(t.fg_muted)));
                });
                picked = stops(ui, t, ai::EFFORTS.len(), at);
            });
        });
        if let Some(i) = picked {
            self.settings.ai.effort = ai::EFFORTS[i].into();
        }
    }

    fn provider_menu_ui(&mut self, ui: &mut Ui, t: &Theme) {
        let ctx = ui.ctx().clone();
        ui.spacing_mut().item_spacing.y = 2.0;
        kit::caps(ui, t, tr!("WHO ANSWERS"));
        for provider in Provider::ALL {
            let (note, light) = match self.chat.check(provider) {
                Some(Ok(_)) => (tr!("ready"), Some(t.meter_ok)),
                Some(Err(_)) => (tr!("not set up"), Some(t.meter_hot)),
                None => (tr!("trying…"), None),
            };
            let chosen = self.settings.ai.provider == provider;
            if menu_choice(ui, t, chosen, provider.name(), note, light).clicked() && !chosen {
                self.settings.ai.provider = provider;
                self.chat.filter.clear();
                self.chat_check(&ctx, provider);
            }
        }
        let provider = self.settings.ai.provider;
        ui.add_space(t.gap);
        kit::menu_line(ui, t);
        ui.add_space(t.gap);
        kit::caps(ui, t, tr!("MODEL"));
        // the models to pick from (id, name): Claude's aliases, Codex's list, what the endpoint offers
        let (models, default): (Vec<(String, String)>, Option<&str>) = match provider {
            Provider::ClaudeCode => (ai::CLAUDE_MODELS.iter().map(|(id, name)| (id.to_string(), name.to_string())).collect(), Some(tr!("Claude Code's default"))),
            Provider::Codex => (self.chat.codex_models.clone(), Some(tr!("Codex's default"))),
            Provider::Local => (self.chat.models.as_ref().and_then(|m| m.as_ref().ok()).into_iter().flatten().map(|m| (m.clone(), m.clone())).collect(), None),
        };
        match (provider, &self.chat.models) {
            (Provider::Local, None) => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(RichText::new(tr!("Asking for its models…")).small().color(t.fg_muted));
                });
            }
            (Provider::Local, Some(Err(e))) => {
                ui.add(Label::new(small(e).color(t.meter_hot)).wrap());
            }
            _ => {}
        }
        // the models in a menu of their own beside this one, a search on top when the list is long
        let current = provider.model(&mut self.settings.ai).trim().to_string();
        let shown = if current.is_empty() { default.unwrap_or(tr!("None")).to_string() } else { ai::model_name(&current, &self.chat.codex_models).to_string() };
        let row = menu_more(ui, t, &shown);
        let config = egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        let mut picked = None;
        egui::containers::menu::SubMenu::new().config(config).show(ui, &row, |ui| {
            ui.set_width(260.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            if models.len() > 12 {
                ui.add(kit::field(&mut self.chat.filter).hint_text(format!("{}  {}", icons::MAGNIFYING_GLASS, tr!("Search…"))).desired_width(ui.available_width()));
                ui.add_space(t.gap);
            }
            let words = self.chat.filter.to_lowercase();
            egui::ScrollArea::vertical().id_salt("ai-models").max_height(320.0).show(ui, |ui| {
                if let Some(default) = default
                    && menu_choice(ui, t, current.is_empty(), default, "", None).clicked()
                {
                    picked = Some(String::new());
                }
                for (id, name) in models.iter().filter(|(id, name)| id.to_lowercase().contains(&words) || name.to_lowercase().contains(&words)) {
                    if menu_choice(ui, t, *id == current, name, "", None).clicked() {
                        picked = Some(id.clone());
                    }
                }
            });
            if picked.is_some() {
                ui.close();
            }
        });
        if let Some(m) = picked {
            if provider == Provider::Local {
                self.settings.ai.sees_images = ai::sees(&m);
            }
            *provider.model(&mut self.settings.ai) = m;
        }
        if provider == Provider::Local {
            ui.checkbox(&mut self.settings.ai.sees_images, tr!("The model can look at pictures"));
            self.connect_ui(ui, t, &ctx);
        }
        ui.add_space(t.gap);
        kit::menu_line(ui, t);
        ui.add_space(t.gap);
        // the test, and what it found
        ui.horizontal(|ui| {
            let trying = self.chat.checking.contains(&provider);
            if ui.add_enabled(!trying, egui::Button::new(tr!("Test the connection"))).clicked() {
                self.chat_check(&ctx, provider);
            }
            if trying {
                ui.add(egui::Spinner::new().size(12.0));
            }
        });
        match self.chat.check(provider) {
            Some(Ok(said)) => ui.add(Label::new(small(format!("{}  {said}", icons::CHECK)).color(t.meter_ok)).wrap()),
            Some(Err(e)) => ui.add(Label::new(small(format!("{}  {e}", icons::WARNING)).color(t.meter_hot)).wrap().selectable(true)),
            None => ui.label(""),
        };
        if ui.button(format!("{}  {}", icons::INFO, tr!("How to connect an AI…"))).clicked() {
            self.chat.guide = true;
            ui.close();
        }
    }

    /// For a model behind an API: the services to connect in a click (the local ones found
    /// running marked), the endpoint, its key.
    fn connect_ui(&mut self, ui: &mut Ui, t: &Theme, ctx: &egui::Context) {
        ui.add_space(t.gap);
        kit::menu_line(ui, t);
        ui.add_space(t.gap);
        kit::caps(ui, t, tr!("CONNECT"));
        let current = ai::preset(&self.settings.ai.endpoint);
        let mut pick = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for (i, (name, endpoint, _)) in ai::PRESETS.iter().enumerate() {
                let running = self.chat.found.as_ref().is_some_and(|f| f.iter().any(|(j, _)| *j == i));
                let label = if running { format!("■ {name}") } else { name.to_string() };
                let r = ui.add(egui::Button::selectable(current == Some(i), RichText::new(label).small().color(if running { t.meter_ok } else { t.fg_text })));
                if r.on_hover_text(if running { tr!("Running on this computer") } else { endpoint }).clicked() {
                    pick = Some(i);
                }
            }
        });
        let mut changed = false;
        if let Some(i) = pick.filter(|i| Some(*i) != current) {
            self.settings.ai.endpoint = ai::PRESETS[i].1.into();
            self.settings.ai.model.clear();
            changed = true;
        }
        ui.label(RichText::new(tr!("Endpoint (OpenAI-compatible)")).small().color(t.fg_muted));
        let r = ui.add(kit::field(&mut self.settings.ai.endpoint).desired_width(f32::INFINITY).font(Theme::mono(Theme::TEXT_SM)));
        changed |= r.lost_focus();
        if changed {
            self.chat.has_key = Some(ai::key(&self.settings.ai.endpoint).is_some());
            self.chat.checks.retain(|c| c.0 != Provider::Local);
            self.chat_check(ctx, Provider::Local);
        }
        // a cloud service's key, kept by the system keychain
        let preset = ai::preset(&self.settings.ai.endpoint);
        if preset.is_some_and(|i| ai::PRESETS[i].2.is_none()) {
            ui.label(RichText::new(tr!("No key needed: it runs on this computer.")).small().color(t.fg_muted));
            return;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr!("API key")).small().color(t.fg_muted));
            if let Some(url) = preset.and_then(|i| ai::PRESETS[i].2) {
                kit::right(ui, |ui| ui.hyperlink_to(RichText::new(format!("{}  {}", tr!("Get a key"), icons::ARROW_SQUARE_OUT)).small(), url));
            }
        });
        ui.horizontal(|ui| {
            let saved = self.chat.has_key == Some(true);
            let hint = if saved { tr!("Saved in the keychain") } else { tr!("No key") };
            let room = ui.available_width() - if saved { 150.0 } else { 70.0 };
            ui.add_sized(vec2(room, Theme::CONTROL_H), kit::field(&mut self.chat.key).password(true).hint_text(hint));
            if kit::button(ui, t, None, tr!("Save"), false).clicked() {
                let key = std::mem::take(&mut self.chat.key);
                match ai::set_key(&self.settings.ai.endpoint, &key) {
                    Ok(()) => {
                        self.chat.has_key = Some(!key.trim().is_empty());
                        self.chat_check(ctx, Provider::Local);
                    }
                    Err(e) => self.say_as(Tone::Error, tr!("The keychain said no: {}", e)),
                }
            }
            if saved && kit::button(ui, t, None, tr!("Remove"), false).clicked() && ai::set_key(&self.settings.ai.endpoint, "").is_ok() {
                self.chat.has_key = Some(false);
            }
        });
    }
}

/// What the conversation asked for while drawing (applied after it).
#[derive(Default)]
struct Actions {
    send: Option<(String, Vec<PathBuf>)>,
    undo: Option<usize>,
    copy: Option<(usize, usize)>,
    /// Something was copied: the toast says so.
    copied: bool,
    close_guide: bool,
    terminal: bool,
}

/// Where "Open the Terminal" opens one: Terminal on macOS, PowerShell on Windows.
const TERMINAL: bool = cfg!(any(target_os = "macos", windows));

/// Readable plain text (AI Elements' Response): 14 px on a 22-px line, paragraphs apart.
fn prose(ui: &mut Ui, text: &str, color: Color32, selectable: bool) {
    let size = Theme::TEXT_MD + 1.0;
    for (i, paragraph) in text.split("\n\n").enumerate() {
        if i > 0 {
            ui.add_space(4.0);
        }
        let format = TextFormat { font_id: FontId::proportional(size), color, line_height: Some((size * 1.6).round()), ..Default::default() };
        let mut job = LayoutJob::single_section(paragraph.trim_end().to_string(), format);
        job.wrap.max_width = ui.available_width();
        ui.add(Label::new(job).wrap().selectable(selectable));
    }
}

/// A suggestion (AI Elements' Suggestion): a row as wide as the chat, a spark before its words.
fn suggestion(ui: &mut Ui, t: &Theme, text: &str) -> egui::Response {
    let width = ui.available_width();
    let format = TextFormat { font_id: Theme::body(), color: t.fg_text, line_height: Some((Theme::TEXT_MD * 1.45).round()), ..Default::default() };
    let mut job = LayoutJob::single_section(text.to_string(), format);
    job.wrap.max_width = width - 52.0;
    let galley = ui.painter().layout_job(job);
    let (rect, r) = ui.allocate_exact_size(vec2(width, galley.size().y + 16.0), Sense::click());
    let k = kit::hover(ui, t, &r);
    ui.painter().rect_filled(rect, t.radius_md, Theme::mix(t.bg_panel, t.fg_text, 0.025 + 0.045 * k));
    ui.painter().rect_stroke(rect, t.radius_md, Stroke::new(1.0, Theme::mix(t.line, t.accent, 0.5 * k)), StrokeKind::Inside);
    kit::icon_at(ui.painter(), pos2(rect.left() + 17.0, rect.center().y), icons::SPARKLE, 14.0, Theme::mix(t.fg_muted, t.accent, k));
    ui.painter().galley(pos2(rect.left() + 34.0, rect.center().y - galley.size().y / 2.0), galley, t.fg_text);
    kit::icon_at(ui.painter(), pos2(rect.right() - 16.0, rect.center().y), icons::ARROW_RIGHT, 13.0, t.fg_muted.gamma_multiply(k));
    kit::focus_ring(ui, t, &r, t.radius_md);
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A picture as a square tile, cropped to fill it; bigger while hovered.
fn picture(ui: &mut Ui, t: &Theme, tex: Option<&TextureHandle>, size: f32) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let round = CornerRadius::same(t.radius_md);
    match tex {
        Some(tex) => {
            let [w, h] = tex.size().map(|v| v as f32);
            let (u, v) = if w > h { ((1.0 - h / w) / 2.0, 0.0) } else { (0.0, (1.0 - w / h) / 2.0) };
            egui::Image::new((tex.id(), rect.size())).uv(Rect::from_min_max(pos2(u, v), pos2(1.0 - u, 1.0 - v))).corner_radius(round).paint_at(ui, rect);
        }
        None => {
            ui.painter().rect_filled(rect, round, t.bg_control);
            kit::icon_at(ui.painter(), rect.center(), icons::FILE, 18.0, t.fg_muted);
        }
    }
    ui.painter().rect_stroke(rect, round, Stroke::new(1.0, t.line), StrokeKind::Inside);
    match tex {
        Some(tex) => r.on_hover_ui(|ui| {
            let [w, h] = tex.size().map(|v| v as f32);
            let k = 260.0 / w.max(h);
            ui.image((tex.id(), vec2(w * k, h * k)));
        }),
        None => r.on_hover_text(tr!("This picture is gone")),
    }
}

/// Border Beam (libraries.dev/beam), redone for egui and in the theme's own colors: colors that
/// sit along the border (the meter's green on the left, the accent on top, selection blue below,
/// the clip red on the right), a light that rides it and shows them, a soft glow inside. Beam's
/// own numbers, its blurs as bands that fade. `strength` 0..1; `turn`: where its conic gradients
/// start, in radians clockwise from the top (Beam turns it once every 1.96 s).
fn beam(painter: &egui::Painter, t: &Theme, rect: Rect, radius: f32, strength: f32, turn: f32) {
    use egui::Vec2;
    use std::f32::consts::{FRAC_PI_2, PI, TAU};
    let rgb = |c: Color32| [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0];
    // where each color sits (a fraction of the box), how far it reaches (px), its color, and its
    // alpha in the glow; the first one on top
    let spots: [(f32, f32, f32, f32, [f32; 3], f32); 8] = [
        (0.02, 0.68, 9.0, 18.0, rgb(t.meter_ok), 0.5),
        (0.02, 0.68, 4.0, 8.0, rgb(Theme::mix(t.meter_ok, t.select, 0.5)), 0.45),
        (0.72, -0.03, 59.0, 9.0, rgb(t.accent), 0.35),
        (0.74, 1.0, 42.0, 7.0, rgb(t.select), 0.35),
        (1.0, 0.27, 10.0, 17.0, rgb(t.meter_hot), 0.3),
        (1.0, 0.27, 10.0, 18.0, rgb(Theme::mix(t.meter_hot, t.select, 0.5)), 0.4),
        (1.0, 0.27, 5.0, 10.0, rgb(t.select), 0.3),
        (1.0, 0.27, 11.0, 12.0, rgb(Theme::mix(t.meter_hot, t.accent, 0.5)), 0.3),
    ];
    // the conic gradients that turn: the light on the line, the glow's window, the bloom
    const LINE: [(f32, f32); 11] = [(0.0, 0.0), (0.54, 0.0), (0.57, 0.1), (0.6, 0.3), (0.63, 0.6), (0.66, 0.75), (0.69, 0.6), (0.72, 0.3), (0.75, 0.1), (0.78, 0.0), (1.0, 0.0)];
    const GLOW: [(f32, f32); 10] = [(0.0, 0.0), (0.22, 0.0), (0.28, 0.12), (0.36, 0.4), (0.46, 1.0), (0.82, 1.0), (0.88, 0.4), (0.94, 0.12), (0.97, 0.0), (1.0, 0.0)];
    const BLOOM: [(f32, f32); 14] = [(0.0, 0.0), (0.58, 0.0), (0.62, 0.03), (0.65, 0.08), (0.67, 0.2), (0.69, 0.45), (0.7, 0.85), (0.705, 0.85), (0.715, 0.45), (0.73, 0.2), (0.75, 0.08), (0.78, 0.03), (0.82, 0.0), (1.0, 0.0)];
    if strength <= 0.0 || rect.width() < 1.0 {
        return;
    }
    let ramp = |stops: &[(f32, f32)], f: f32| stops.windows(2).find(|w| f <= w[1].0).map_or(0.0, |w| w[0].1 + (w[1].1 - w[0].1) * (f - w[0].0) / (w[1].0 - w[0].0).max(1e-6));
    // premultiplied, 0..1; `glow`: the spots as the glow has them, else at full alpha
    let spots = |p: Pos2, glow: bool| {
        spots.iter().rev().fold([0.0f32; 4], |under, (x, y, rx, ry, rgb, alpha)| {
            let d = vec2((p.x - rect.left() - x * rect.width()) / rx, (p.y - rect.top() - y * rect.height()) / ry).length();
            let a = (1.0 - d).max(0.0) * if glow { *alpha } else { 1.0 };
            [rgb[0] * a + under[0] * (1.0 - a), rgb[1] * a + under[1] * (1.0 - a), rgb[2] * a + under[2] * (1.0 - a), a + under[3] * (1.0 - a)]
        })
    };
    // light reads on a dark theme as it is; on a light one it's ink, softer
    let gain = if t.dark { 1.25 } else { 0.9 };
    let color = |c: [f32; 4], k: f32| {
        let k = (k * strength).clamp(0.0, 1.0);
        Theme::premul([c[0] * gain * k, c[1] * gain * k, c[2] * gain * k, c[3] * k])
    };
    // the border, a point every few pixels, each with the way in, clockwise from the top right
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let corners = [(rect.right_top() + vec2(-r, r), -FRAC_PI_2), (rect.right_bottom() + vec2(-r, -r), 0.0), (rect.left_bottom() + vec2(r, -r), FRAC_PI_2), (rect.left_top() + vec2(r, r), PI)];
    let mut ring: Vec<(Pos2, Vec2)> = Vec::new();
    for (i, (c, start)) in corners.iter().enumerate() {
        for k in 0..=8 {
            let d = Vec2::angled(start + FRAC_PI_2 * k as f32 / 8.0);
            ring.push((*c + d * r, -d));
        }
        let (out, (next, next_start)) = (Vec2::angled(start + FRAC_PI_2), corners[(i + 1) % 4]);
        let (from, to) = (*c + out * r, next + Vec2::angled(next_start) * r);
        let n = ((to - from).length() / 4.0) as usize;
        ring.extend((1..n).map(|k| (from + (to - from) * k as f32 / n as f32, -out)));
    }
    let mut mesh = egui::Mesh::default();
    // a band along the border, from `near` to `far` px in, its color at each end
    let mut band = |near: f32, far: f32, paint: &dyn Fn(Pos2, f32) -> (Color32, Color32)| {
        let base = mesh.vertices.len() as u32;
        for (p, inward) in &ring {
            let to = *p - rect.center();
            let f = ((to.x.atan2(-to.y) - turn) / TAU).rem_euclid(1.0); // clockwise from the top, as CSS turns
            let (a, b) = paint(*p, f);
            mesh.colored_vertex(*p + *inward * near, a);
            mesh.colored_vertex(*p + *inward * far, b);
        }
        let n = ring.len() as u32;
        for i in 0..n {
            let (a, b) = (base + 2 * i, base + 2 * ((i + 1) % n));
            mesh.add_triangle(a, a + 1, b);
            mesh.add_triangle(b, a + 1, b + 1);
        }
    };
    // the glow inside: the colors and a faint white, through the turning window
    band(0.5, 12.0, &|p, f| {
        let [r, g, b, a] = spots(p, true);
        (color([r + 0.3 * (1.0 - a), g + 0.3 * (1.0 - a), b + 0.3 * (1.0 - a), a + 0.3 * (1.0 - a)], 0.24 * ramp(&GLOW, f)), Theme::CLEAR)
    });
    // the line: the light over the colors
    band(0.0, 1.2, &|p, f| {
        let w = ramp(&LINE, f);
        let [r, g, b, a] = spots(p, false);
        let c = color([w + r * (1.0 - w), w + g * (1.0 - w), w + b * (1.0 - w), w + a * (1.0 - w)], 0.46);
        (c, c)
    });
    // the bloom: the light, blurred
    band(0.5, 9.0, &|_, f| (color([1.0; 4], 0.38 * ramp(&BLOOM, f)), Theme::CLEAR));
    painter.add(mesh);
}

/// An effort as the box names it. Its words have lines of their own in the catalog ("effort:
/// High"): the EQ's High is another word in Italian.
fn effort_name(effort: &str) -> &'static str {
    let (line, english) = match effort {
        "low" => ("effort: Low", "Low"),
        "medium" => ("effort: Medium", "Medium"),
        "xhigh" => ("effort: Extra high", "Extra high"),
        "max" => ("effort: Max", "Max"),
        _ => ("effort: High", "High"),
    };
    crate::i18n::lookup(line).unwrap_or(english)
}

/// How wide a [`bar_button`] with this text is: an icon or a light before it, a caret after.
fn bar_width(ui: &Ui, t: &Theme, text: &str, icon: bool, caret: bool) -> f32 {
    ui.painter().layout_no_wrap(text.to_string(), Theme::small(), t.fg_text).size().x + if icon { 28.0 } else { 20.0 } + if caret { 24.0 } else { 10.0 }
}

/// A button of the row under the box (who answers, the effort): an icon, or a light when Ravune
/// knows whether that AI is ready, its text, a caret when it opens a list; lit while hovered or
/// open, sunk while pressed, as the app's buttons are. At most `room` wide.
fn bar_button(ui: &mut Ui, t: &Theme, icon: Option<&str>, light: Option<Color32>, text: &str, caret: bool, room: f32) -> egui::Response {
    let (lead, tail) = (if icon.is_some() { 28.0 } else { 20.0 }, if caret { 24.0 } else { 10.0 });
    let (rect, r) = ui.allocate_exact_size(vec2(bar_width(ui, t, text, icon.is_some(), caret).min(room.max(60.0)), Theme::CONTROL_H), Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&r));
    let k = kit::hover(ui, t, &r).max(if open { 1.0 } else { 0.0 });
    let shape = if r.is_pointer_button_down_on() { rect.shrink(1.0) } else { rect };
    let painter = ui.painter();
    if k > 0.0 {
        painter.rect_filled(shape, t.radius_md, t.bg_control.gamma_multiply(k));
    }
    let (y, ink) = (shape.center().y, Theme::mix(t.fg_muted, t.fg_text, k));
    if let Some(icon) = icon {
        kit::icon_at(painter, pos2(shape.left() + 15.0, y), icon, 15.0, ink);
    } else if let Some(light) = light {
        led(painter, pos2(shape.left() + 11.0, y), light);
    }
    kit::fit_text(painter, pos2(shape.left() + lead, y), Align2::LEFT_CENTER, text, Theme::small(), ink, shape.width() - lead - tail);
    if caret {
        kit::icon_at(painter, pos2(shape.right() - 11.0, y), icons::CARET_DOWN, 11.0, ink);
    }
    kit::focus_ring(ui, t, &r, t.radius_md);
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A slider that stops at `n` places: a track with a mark at each, a handle on the one at `at`.
/// Clicked or dragged, the nearest place, when it's another one.
fn stops(ui: &mut Ui, t: &Theme, n: usize, at: usize) -> Option<usize> {
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click_and_drag());
    let inner = rect.shrink2(vec2(12.0, 0.0)); // the end places sit inside the track's ends
    let x = |i: usize| inner.left() + inner.width() * i as f32 / (n - 1) as f32;
    let (painter, y) = (ui.painter(), rect.center().y);
    painter.rect_filled(rect, t.radius_md, t.bg_field);
    // lit up to the handle
    painter.rect_filled(Rect::from_min_max(rect.min, pos2(x(at) + 5.0, rect.max.y)), t.radius_md, Theme::mix(t.bg_field, t.accent, 0.25));
    for i in (0..n).filter(|i| *i != at) {
        painter.rect_filled(Rect::from_center_size(pos2(x(i), y), vec2(2.0, 8.0)), 0, t.fg_muted);
    }
    let k = kit::hover(ui, t, &r);
    painter.rect_filled(Rect::from_center_size(pos2(x(at), y), vec2(10.0, 20.0)), t.radius_sm, Theme::mix(t.accent.gamma_multiply(0.85), t.accent, k));
    let to = r.interact_pointer_pos().filter(|_| r.clicked() || r.dragged()).map(|p| ((p.x - inner.left()) / inner.width() * (n - 1) as f32).round().clamp(0.0, (n - 1) as f32) as usize);
    r.on_hover_cursor(egui::CursorIcon::PointingHand);
    to.filter(|i| *i != at)
}

/// A line of the AI's menu that opens a menu of its own beside it: what's chosen, an arrow.
fn menu_more(ui: &mut Ui, t: &Theme, text: &str) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    let k = kit::hover(ui, t, &r);
    let painter = ui.painter();
    painter.rect_filled(rect, t.radius_md, Theme::mix(t.bg_field, t.fg_text, 0.03 + 0.04 * k));
    let y = rect.center().y;
    kit::icon_at(painter, pos2(rect.left() + 14.0, y), icons::ROBOT, 14.0, t.fg_muted);
    kit::fit_text(painter, pos2(rect.left() + 28.0, y), Align2::LEFT_CENTER, text, Theme::body(), t.fg_text, rect.width() - 54.0);
    kit::icon_at(painter, pos2(rect.right() - 12.0, y), icons::CARET_RIGHT, 12.0, Theme::mix(t.fg_muted, t.fg_text, k));
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A line of the AI's menu: a tick when chosen, its name, a note on the right with a light
/// before it when it has a color.
fn menu_choice(ui: &mut Ui, t: &Theme, chosen: bool, name: &str, note: &str, light: Option<Color32>) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    let k = kit::hover(ui, t, &r);
    let painter = ui.painter();
    if chosen || k > 0.0 {
        let fill = if chosen { Theme::mix(t.bg_device, t.select, 0.16) } else { Theme::mix(t.bg_device, t.fg_text, 0.06 * k) };
        painter.rect_filled(rect, t.radius_sm, fill);
    }
    let y = rect.center().y;
    if chosen {
        kit::icon_at(painter, pos2(rect.left() + 12.0, y), icons::CHECK, 13.0, t.accent);
    }
    let (said, _) = kit::fit_text(painter, pos2(rect.right() - 8.0, y), Align2::RIGHT_CENTER, note, Theme::mono(Theme::TEXT_XS), t.fg_muted, rect.width() * 0.45);
    let mut right = said.left();
    if let Some(light) = light {
        led(painter, pos2(said.left() - 8.0, y), light);
        right -= 14.0;
    }
    kit::fit_text(painter, pos2(rect.left() + 26.0, y), Align2::LEFT_CENTER, name, Theme::body(), t.fg_text, right - rect.left() - 34.0);
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A reply's commands as one row (AI Elements' Tool, closed): their state, how many, the
/// latest, and a caret that opens the list.
fn steps_row(ui: &mut Ui, t: &Theme, steps: &[Step], open: bool) -> egui::Response {
    let failed = steps.iter().filter(|s| s.state == StepState::Failed).count();
    let state = if failed > 0 {
        StepState::Failed
    } else if steps.iter().any(|s| s.state == StepState::Running) {
        StepState::Running
    } else {
        StepState::Done
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    let k = kit::hover(ui, t, &resp);
    ui.painter().rect_filled(rect, t.radius_md, Theme::mix(t.bg_panel, t.fg_text, 0.03 + 0.03 * k));
    // a hairline down the left in the state's color, like a lit channel strip
    let edge = match state {
        StepState::Failed => t.meter_hot,
        StepState::Running => t.accent,
        StepState::Done => t.line,
    };
    ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(2.0, rect.height())), 0, edge);
    let y = rect.center().y;
    paint_state(ui, t, pos2(rect.left() + 15.0, y), state);
    let painter = ui.painter();
    let count = if steps.len() == 1 { tr!("1 step").to_string() } else { tr!("{} steps", steps.len()) };
    let count = if failed > 0 { tr!("{} · {} failed", count, failed) } else { count };
    let (said, _) = kit::fit_text(painter, pos2(rect.left() + 30.0, y), Align2::LEFT_CENTER, count, Theme::small(), if failed > 0 { t.meter_hot } else { t.fg_text }, rect.width() - 60.0);
    // the latest command, as far as it fits
    if let Some(last) = steps.last() {
        kit::fit_text(painter, pos2(said.right() + 8.0, y), Align2::LEFT_CENTER, &last.label, Theme::mono(Theme::TEXT_XS), t.fg_muted, rect.right() - 26.0 - said.right() - 8.0);
    }
    kit::icon_turned(painter, pos2(rect.right() - 14.0, y), icons::CARET_DOWN, 12.0, Theme::mix(t.fg_muted, t.fg_text, k), if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One command in an open row: its state, its operator, what it was given, what went wrong.
fn step_line(ui: &mut Ui, t: &Theme, step: &Step) {
    let (op, args) = step.label.split_once(' ').unwrap_or((&step.label, ""));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _) = ui.allocate_exact_size(vec2(14.0, 16.0), Sense::hover());
        paint_state(ui, t, rect.center(), step.state);
        let mut job = LayoutJob::default();
        job.append(op, 0.0, TextFormat::simple(Theme::mono(Theme::TEXT_XS), if step.state == StepState::Failed { t.meter_hot } else { t.fg_text }));
        job.append(args, 6.0, TextFormat::simple(Theme::mono(Theme::TEXT_XS), t.fg_muted));
        let r = ui.add(Label::new(job).truncate());
        if !args.is_empty() {
            r.on_hover_text(&step.label);
        }
    });
    if let Some(error) = &step.error {
        Frame::new().inner_margin(egui::Margin { left: 20, right: 0, top: 0, bottom: 2 }).show(ui, |ui| {
            ui.add(Label::new(mono(error).color(t.meter_hot.gamma_multiply(0.85))).wrap().selectable(true));
        });
    }
}

/// A command's state at `c`: a square turning while it runs, a check, a cross.
fn paint_state(ui: &Ui, t: &Theme, c: Pos2, state: StepState) {
    match state {
        StepState::Running => {
            let a = (seconds() * 4.0) as f32;
            let corners: Vec<Pos2> = (0..4).map(|i| c + egui::Vec2::angled(a + i as f32 * std::f32::consts::FRAC_PI_2) * 4.5).collect();
            ui.painter().add(egui::Shape::closed_line(corners, Stroke::new(1.5, t.accent)));
            ui.ctx().request_repaint();
        }
        StepState::Done => kit::icon_at(ui.painter(), c, icons::CHECK, 12.0, t.meter_ok),
        StepState::Failed => kit::icon_at(ui.painter(), c, icons::X, 12.0, t.meter_hot),
    }
}

/// What a problem card's buttons asked for.
struct Card {
    retry: bool,
    terminal: bool,
    copy: bool,
}

/// A problem as a card: what the provider said, what to do about it when Ravune can tell, and
/// buttons to send the message again, open a terminal, copy what it said.
fn problem_card(ui: &mut Ui, t: &Theme, provider: Provider, text: &str, retry: bool) -> Card {
    let help = help(provider, text);
    let mut card = Card { retry: false, terminal: false, copy: false };
    let frame = Frame::new()
        .fill(Theme::mix(t.bg_panel, t.meter_hot, 0.07))
        .stroke(Stroke::new(1.0, t.meter_hot.gamma_multiply(0.35)))
        .corner_radius(t.radius_md)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(RichText::new(icons::WARNING).size(15.0).color(t.meter_hot));
                ui.label(RichText::new(tr!("{} couldn't answer", provider.name())).color(t.fg_text));
            });
            ui.add(Label::new(mono(text).color(t.fg_muted)).wrap().selectable(true));
            if let Some(help) = &help {
                ui.add(Label::new(small(help).color(t.fg_text)).wrap());
            }
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = t.gap;
                if retry {
                    card.retry = kit::action(ui, t, kit::Variant::Secondary, Some(icons::ARROW_CLOCKWISE), tr!("Retry")).on_hover_text(tr!("Send the same message again")).clicked();
                }
                if TERMINAL && help.is_some() && provider != Provider::Local {
                    card.terminal = kit::action(ui, t, kit::Variant::Outline, Some(icons::CODE), tr!("Open Terminal")).clicked();
                }
                kit::right(ui, |ui| card.copy = kit::icon_button(ui, t, icons::COPY, tr!("Copy what it said"), false).clicked());
            });
        });
    // the card's edge lit red, as a clipping meter
    let rect = frame.response.rect;
    ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(2.0, rect.height())), 0, t.meter_hot);
    card
}

/// An arrow, up (send) or down (to the newest message).
fn arrow(painter: &egui::Painter, c: Pos2, ink: Color32, down: bool) {
    let d = if down { -1.0 } else { 1.0 };
    let s = Stroke::new(2.0, ink);
    painter.line_segment([c + vec2(0.0, 5.5 * d), c + vec2(0.0, -5.5 * d)], s);
    painter.line_segment([c + vec2(-4.5, -d), c + vec2(0.0, -5.5 * d)], s);
    painter.line_segment([c + vec2(4.5, -d), c + vec2(0.0, -5.5 * d)], s);
}

/// A section of the AI guide: a card whose header (a light when it's ready, its name, what
/// Ravune found, a caret) opens and closes it; closed at first, as it was left after that.
fn guide_card(ui: &mut Ui, t: &Theme, name: &str, status: &str, ready: Option<bool>, add: impl FnOnce(&mut Ui)) {
    let id = egui::Id::unique(("guide card", name));
    let mut open = ui.data_mut(|d| d.get_persisted::<bool>(id)).unwrap_or(false);
    Frame::new().fill(Theme::mix(t.bg_panel, t.fg_text, 0.03)).stroke(Stroke::new(1.0, t.line)).corner_radius(t.radius_md).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), Theme::CONTROL_H + 2.0 * t.gap), Sense::click());
        let h = kit::hover(ui, t, &r);
        if h > 0.0 {
            ui.painter().rect_filled(rect, t.radius_md, Theme::mix(t.bg_panel, t.fg_text, 0.03 * h));
        }
        if r.clicked() {
            open = !open;
        }
        let mut x = rect.left() + t.pad;
        if let Some(ready) = ready {
            led(ui.painter(), pos2(x + 4.0, rect.center().y), if ready { t.meter_ok } else { t.fg_muted });
            x += 14.0;
        }
        let caret = pos2(rect.right() - t.pad - 6.0, rect.center().y);
        kit::icon_turned(ui.painter(), caret, icons::CARET_DOWN, 13.0, t.fg_muted, if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 });
        let (named, _) = kit::fit_text(ui.painter(), pos2(x, rect.center().y), Align2::LEFT_CENTER, name, Theme::body(), t.fg_text, caret.x - 12.0 - x);
        kit::fit_text(ui.painter(), pos2(named.right() + t.gap * 2.0, rect.center().y), Align2::LEFT_CENTER, status, Theme::mono(Theme::TEXT_XS), t.fg_muted, caret.x - 12.0 - named.right() - t.gap * 2.0);
        if open {
            Frame::new().inner_margin(egui::Margin { left: t.pad as i8, right: t.pad as i8, top: 0, bottom: t.pad as i8 }).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(t.gap, t.gap);
                add(ui);
            });
        }
    });
    ui.add_space(t.gap);
    ui.data_mut(|d| d.insert_persisted(id, open));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(text: &str) -> Item {
        Item::User(Message { text: text.into(), pictures: Vec::new() })
    }

    #[test]
    fn a_reply_becomes_messages_steps_and_problems() {
        let (tx, events) = channel();
        let mut c = Conversation { run: Some(Run::for_test(events)), started: Some(Instant::now()), ..Default::default() };
        for e in [
            Event::Session("s1".into()),
            Event::Tool("a".into(), "query project".into()),
            Event::Tool("b".into(), "pattern.add".into()),
            Event::ToolDone("a".into(), None),
            Event::Text("Writing it.".into()),
            Event::Tool("c".into(), "render and analyze".into()),
            Event::ToolDone("c".into(), Some("no audio".into())),
            Event::Text("Failed to authenticate".into()),
            Event::Problem("Failed to authenticate".into()),
        ] {
            tx.send(e).unwrap();
        }
        assert!(!c.take(), "still on its way");
        assert_eq!(c.session.as_deref(), Some("s1"));
        drop(tx);
        assert!(c.take(), "done when the provider hangs up");
        let kinds: Vec<String> = c
            .items
            .iter()
            .map(|i| match i {
                Item::Steps(s) => format!("steps {}", s.iter().map(|s| format!("{:?}{}", s.state, s.error.as_deref().map(|e| format!(" ({e})")).unwrap_or_default())).collect::<Vec<_>>().join(",")),
                Item::Text { text, .. } => format!("text {text}"),
                Item::Problem(p) => format!("problem {p}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(kinds, ["steps Done,Running", "text Writing it.", "steps Failed (no audio)", "problem Failed to authenticate"]);
        assert!(help(Provider::ClaudeCode, "Failed to authenticate: OAuth session expired").is_some_and(|h| h.contains("claude auth login")));
        assert!(help(Provider::Local, "The local model didn't answer").is_some());
        assert!(help(Provider::ClaudeCode, "the pattern has no notes").is_none());
        assert!(edits("mixer.set {\"track\":1}") && !edits("query project"));
    }

    #[test]
    fn a_reply_that_went_wrong_can_be_sent_again() {
        let step = |state| Item::Steps(vec![Step { id: "1".into(), label: "pattern.add".into(), state, error: None }]);
        let end = Item::End { seconds: 1, steps: 1 };
        let chat = |reply: Vec<Item>| Conversation { items: [vec![said("first"), Item::Text { text: "Done.".into(), shown: 5 }, end.clone(), said("groove")], reply].concat(), ..Default::default() };
        let again = |c: &Conversation| c.retry().map(|m| m.text.clone());
        assert_eq!(again(&chat(vec![Item::Problem("Failed to authenticate".into()), end.clone()])).as_deref(), Some("groove"));
        assert_eq!(again(&chat(vec![step(StepState::Failed), Item::Text { text: "It broke.".into(), shown: 9 }, end.clone()])).as_deref(), Some("groove"), "a command that failed");
        assert_eq!(again(&chat(vec![step(StepState::Done), Item::Text { text: "Done.".into(), shown: 5 }, end.clone()])), None, "one that went well");
        // a problem in an earlier reply doesn't count
        let earlier = Conversation { items: vec![said("a"), Item::Problem("no".into()), end.clone(), said("b"), Item::Text { text: "Fine.".into(), shown: 5 }, end], ..Default::default() };
        assert_eq!(again(&earlier), None);
    }

    #[test]
    fn conversations_are_kept_without_their_run() {
        let groove = Item::User(Message { text: "a groove like this".into(), pictures: vec![PathBuf::from("a.jpg")] });
        let c = Conversation { title: "Groove".into(), items: vec![groove, Item::Text { text: "Done.".into(), shown: 2 }, Item::End { seconds: 3, steps: 1 }], ..Default::default() };
        let back: Vec<Conversation> = serde_json::from_str(&serde_json::to_string(&vec![c]).unwrap()).unwrap();
        assert_eq!(back[0].title, "Groove");
        assert!(matches!(&back[0].items[0], Item::User(m) if m.pictures == [PathBuf::from("a.jpg")]));
        assert!(matches!(back[0].items[1], Item::Text { shown: usize::MAX, .. }), "a reopened chat shows its text whole");
        assert_eq!(serde_json::to_string(&said("groove")).unwrap(), r#"{"User":{"text":"groove"}}"#);
    }
}
