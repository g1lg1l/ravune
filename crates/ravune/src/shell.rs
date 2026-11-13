//! The window around the editors: top bar, status bar, on-screen keys, the editor tabs and
//! floating editor windows, Preferences, the shortcut sheet, the command palette and the
//! rename dialog.

use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, ComboBox, CornerRadius, FontId, Id, Key, Modifiers, RichText, Sense, Stroke, Ui, pos2, vec2};

use crate::keymap::{self, COMMANDS, Keymap, Preset};
use ravune_core::{OPERATORS, PPQ};
use ravune_engine::Engine;
use serde_json::json;

use crate::theme::{THEME_EXTENSION, THEME_EXTENSIONS, Theme};
use crate::widgets::WaveStyle;
use crate::i18n::{self, Lang};
use crate::{App, BUFFER_SIZES, PIANO_KEYS, Tab, files, icons, kit, usage, widgets};

/// What a toast is about: its badge, its title and their color (Velo's tones, after Sileo).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tone {
    /// Something to read, and go on.
    Info,
    Done,
    /// Ravune waits on the user: a knob to move, an update to install.
    Action,
    Error,
}

/// The one toast on screen; a new one replaces it.
#[derive(Debug)]
pub struct Toast {
    tone: Tone,
    pub text: String,
    born: Instant,
    /// A button on it ("Undo") and what it's for.
    action: Option<(String, &'static str)>,
}

impl Toast {
    pub fn new(tone: Tone, text: String) -> Self {
        Self { tone, text, born: Instant::now(), action: None }
    }

    /// With a button: when it's clicked, the app's `toast_acted` says `what`.
    pub fn with_action(mut self, label: impl Into<String>, what: &'static str) -> Self {
        self.action = Some((label.into(), what));
        self
    }
}

/// Seconds on screen, seconds in and out, and how wide it gets.
const TOAST_LIFE: f32 = 6.0;
const TOAST_ENTER: f32 = 0.4;
const TOAST_WIDTH: f32 = 340.0;

/// The editors: tab, icon, name, keymap command.
pub const EDITORS: [(Tab, &str, &str, &str); 4] = [
    (Tab::Rack, icons::SQUARES_FOUR, "Channel Rack", "editor.rack"),
    (Tab::PianoRoll, icons::PIANO_KEYS, "Piano Roll", "editor.piano"),
    (Tab::Mixer, icons::FADERS, "Mixer", "editor.mixer"),
    (Tab::Modulation, icons::WAVE_SINE, "Modulation", "editor.modulation"),
];

/// What the rename dialog renames.
#[derive(Clone, Copy)]
pub enum Rename {
    Pattern(usize),
    Channel(usize),
    Track(usize),
    Macro(usize),
    Workspace(usize),
    Lane(u32),
    /// An effect, by id.
    Device(u32),
    /// A marker, by index.
    Marker(usize),
}

pub struct Renaming {
    pub what: Rename,
    pub text: String,
    focused: bool,
}

impl Renaming {
    pub fn new(what: Rename, text: impl Into<String>) -> Self {
        Self { what, text: text.into(), focused: false }
    }
}

#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub enum PrefsTab {
    #[default]
    Audio,
    Midi,
    Look,
    Keys,
    Agents,
    Extensions,
}

impl PrefsTab {
    pub fn named(name: &str) -> Option<PrefsTab> {
        let all = [PrefsTab::Audio, PrefsTab::Midi, PrefsTab::Look, PrefsTab::Keys, PrefsTab::Agents, PrefsTab::Extensions];
        all.into_iter().find(|t| format!("{t:?}").eq_ignore_ascii_case(name))
    }
}

/// What a command palette line runs.
#[derive(Clone)]
enum Choice {
    Op(&'static str),
    Script(Box<crate::script::Script>),
}

/// A theme in the Look list: its name and a strip of its colors.
fn theme_row(ui: &mut Ui, t: &Theme, theme: &Theme, current: bool) -> egui::Response {
    let r = kit::row(ui, t, Id::unique(("theme-row", &theme.name)), current);
    let mut painter = ui.painter_at(r.rect);
    painter.multiply_opacity(kit::row_alpha(ui, &r));
    painter.text(r.rect.left_center() + vec2(t.pad, 0.0), Align2::LEFT_CENTER, &theme.name, FontId::proportional(13.0), t.fg_text);
    let size = r.rect.height() - 10.0;
    for (i, color) in theme.swatches().into_iter().rev().enumerate() {
        let min = pos2(r.rect.right() - t.pad - (i + 1) as f32 * (size + 2.0), r.rect.top() + 5.0);
        let swatch = egui::Rect::from_min_size(min, vec2(size, size));
        painter.rect_filled(swatch, CornerRadius::same(t.radius_sm), color);
        painter.rect_stroke(swatch, CornerRadius::same(t.radius_sm), Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
    }
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, current, &theme.name));
    r
}

/// A group on the F1 sheet: its name, then each line's shortcuts and what they do.
type SheetGroup<'a> = (&'a str, Vec<(Vec<String>, &'a str)>);

fn kbd(ui: &mut Ui, t: &Theme, keys: &str) {
    for key in keys.split(' ') {
        let galley = ui.painter().layout_no_wrap(key.to_string(), Theme::mono(11.0), t.fg_text);
        let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(10.0, 6.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(t.radius_sm), t.bg_control);
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, t.bg_field));
        ui.painter().galley(rect.min + vec2(5.0, 3.0), galley, t.fg_text);
    }
}

impl App {
    // ---- top bar ------------------------------------------------------------

    /// The menu bar on top: the logo and the menus on the left; search, help and
    /// preferences on the right.
    pub fn menu_bar(&mut self, ui: &mut Ui) {
        let t = self.theme.clone();
        let top = ui.max_rect();
        kit::bar(ui, &t, |ui| {
            widgets::logo(ui, &t, self.beat.map(|b| (b / 4.0).fract() as f32));
            kit::vsep(ui, &t);
            let r = kit::button(ui, &t, Some(icons::FILE), tr!("File"), false);
            kit::menu(&t, &r).show(|ui| self.file_menu(ui));
            let r = kit::button(ui, &t, None, tr!("Edit"), false);
            kit::menu(&t, &r).show(|ui| self.edit_menu(ui));
            let r = kit::button(ui, &t, None, tr!("Create"), false);
            kit::menu(&t, &r).show(|ui| self.create_menu(ui));
            let r = kit::button(ui, &t, None, tr!("View"), false);
            kit::menu(&t, &r).show(|ui| self.view_menu(ui));
            let r = kit::button(ui, &t, None, tr!("Help"), false);
            kit::menu(&t, &r).show(|ui| self.help_menu(ui));
            kit::right(ui, |ui| {
                let r = kit::icon_button(ui, &t, icons::GEAR_SIX, &self.tip(tr!("Preferences"), "app.preferences"), self.show_settings);
                if r.clicked() {
                    self.toggle_preferences();
                }
                let r = kit::icon_button(ui, &t, icons::QUESTION, tr!("Help: manual, shortcuts (F1), about"), self.show_help || self.show_manual);
                kit::menu(&t, &r).show(|ui| self.help_menu(ui));
                self.chat_toggle(ui, &t);
                let keys = self.settings.keymap.keys("app.palette").first().map(keymap::text).unwrap_or_default();
                let r = kit::button(ui, &t, Some(icons::MAGNIFYING_GLASS), &tr!("Search commands   {}", keys), false);
                if r.clicked() {
                    self.open_palette();
                }
            });
        });
        // a divider between the window's title and the menu bar (after the bar, which fills
        // its background over anything drawn before)
        ui.painter().hline(top.x_range(), top.top() + 0.5, egui::Stroke::new(1.0, t.line));
    }

    /// The transport bar under it: tempo, play, position, undo, workspaces, the readouts.
    pub fn top_bar(&mut self, ui: &mut Ui) {
        let t = self.theme.clone();
        kit::bar(ui, &t, |ui| {
            kit::caps(ui, &t, "BPM");
            let mut bpm = self.session.project.tempo;
            let r = ui.add(egui::DragValue::new(&mut bpm).range(20.0..=300.0).speed(0.2).fixed_decimals(2));
            if r.changed() {
                self.run_merge("transport.set_tempo", json!({ "bpm": bpm }), "tempo");
            }
            if r.drag_stopped() || r.lost_focus() {
                self.session.close_merge();
            }
            self.hint(&r, tr!("Tempo in beats per minute: drag up / down, or double-click to type."));
            let r = kit::button(ui, &t, None, "TAP", false);
            if r.clicked() {
                self.tap_tempo();
            }
            let tip = self.tip(tr!("Tap tempo: click on the beat a few times and the tempo follows"), "app.tap_tempo");
            self.hint(&r, tip);
            let r = kit::icon_button(ui, &t, icons::METRONOME, &self.tip(tr!("Metronome"), "transport.metronome_toggle"), self.session.project.metronome);
            if r.clicked() {
                self.run("transport.metronome_toggle", json!({}));
            }
            kit::vsep(ui, &t);

            if let Some(i) = kit::segmented(ui, &t, self.session.song_mode as usize, &["PAT", "SONG"])
                && (i == 1) != self.session.song_mode
            {
                self.run("transport.mode_toggle", json!({}));
            }
            let playing = self.session.playing;
            let r = kit::icon_button(ui, &t, if playing { icons::STOP } else { icons::PLAY }, tr!("Play / stop (Space)"), playing);
            if r.clicked() {
                self.run("transport.play_toggle", json!({}));
            }
            let recording = self.recording.is_some();
            let tip = self.tip(tr!("Record what plays to a new clip on the playlist"), "app.record");
            // recording: the button breathes
            let blink = if recording && !self.settings.reduce_motion { 0.55 + 0.45 * (ui.input(|i| i.time) * std::f64::consts::TAU * 0.8).cos().abs() as f32 } else { 1.0 };
            let r = kit::tinted_icon_button(ui, &t, icons::RECORD, &tip, recording, t.meter_hot.gamma_multiply(blink));
            if r.clicked() {
                self.toggle_record();
            }
            let tip = tr!("Record automation: while the song plays, knobs you turn write their moves into automation clips");
            if kit::tinted_icon_button(ui, &t, icons::CHART_LINE, tip, self.auto_rec, t.meter_hot).clicked() {
                self.auto_rec = !self.auto_rec;
            }
            let r = widgets::position(ui, &t, self.beat.unwrap_or(self.session.position as f64 / PPQ as f64));
            self.hint(&r, tr!("Position: bar . beat . sixteenth. Home goes back to the start."));
            let r = widgets::beat_counter(ui, &t, self.beat, self.settings.reduce_motion);
            self.hint(&r, tr!("The beat: four lights, the first one for the bar's downbeat."));
            kit::vsep(ui, &t);

            for (id, icon, label) in [
                ("edit.undo", icons::ARROW_U_UP_LEFT, self.session.undo_label()),
                ("edit.redo", icons::ARROW_U_UP_RIGHT, self.session.redo_label()),
            ] {
                let what = if id == "edit.undo" { tr!("Undo") } else { tr!("Redo") };
                let tip = self.tip(&format!("{what} {}", label.map_or(tr!("(nothing yet)"), i18n::t)), id);
                let r = ui.add_enabled_ui(label.is_some(), |ui| kit::icon_button(ui, &t, icon, &tip, false)).inner;
                if r.clicked() {
                    self.run(id, json!({}));
                }
            }
            kit::vsep(ui, &t);
            self.workspace_tabs(ui, &t);

            kit::right(ui, |ui| {
                // one row, everything centred on it, evenly spaced
                ui.spacing_mut().item_spacing.x = t.gap * 2.0;
                // a narrow window keeps the essentials: the audio readout turns into just the CPU
                let roomy = ui.available_width() > 420.0;
                let (text, color) = match &self.engine {
                    Some(e) if roomy => {
                        let s = e.stats();
                        let ms = s.buffer_frames() as f32 / e.sample_rate as f32 * 1000.0;
                        (format!("{:.1} kHz  {ms:.1} ms  CPU {:>2.0}%", e.sample_rate as f32 / 1000.0, s.load() * 100.0), t.fg_muted)
                    }
                    Some(e) => (format!("CPU {:>2.0}%", e.stats().load() * 100.0), t.fg_muted),
                    None => (tr!("NO AUDIO DEVICE").to_string(), t.meter_hot),
                };
                let r = ui.label(RichText::new(text).font(Theme::mono(11.0)).color(color));
                self.hint(&r, tr!("Sample rate · buffer latency · audio CPU. Preferences (Ctrl ,) to change."));
                // clicks and gaps the audio had: a bigger buffer (Preferences) or fewer plugins help
                if let Some(n) = self.engine.as_ref().map(|e| e.stats().xruns()).filter(|n| *n > 0) {
                    let r = ui.label(RichText::new(tr!("{} dropouts", n)).font(Theme::mono(11.0)).color(t.meter_hot));
                    self.hint(&r, tr!("The audio missed its deadline this many times (clicks or gaps): a bigger buffer in Preferences → Audio, or fewer plugins, helps"));
                }
                let r = widgets::meter(ui, &t, self.level);
                self.hint(&r, tr!("Master output level. Red means clipping."));
                self.volume_slider(ui, &t, 64.0);
                if let Some(p) = self.export_progress() {
                    ui.label(RichText::new(tr!("Exporting {}%", format!("{:.0}", p * 100.0))).font(Theme::mono(11.0)).color(t.accent));
                }
            });
        });
    }

    /// A menu line for a keymap command, with its current shortcut.
    fn menu_command(&mut self, ui: &mut Ui, icon: &str, label: &str, command: &str) {
        let keys = self.settings.keymap.keys(command).first().map(keymap::text).unwrap_or_default();
        if ui.add(egui::Button::new(format!("{icon}  {label}")).shortcut_text(keys)).clicked() {
            self.command(command);
            ui.close();
        }
    }

    fn edit_menu(&mut self, ui: &mut Ui) {
        self.menu_command(ui, icons::ARROW_U_UP_LEFT, tr!("Undo"), "edit.undo");
        self.menu_command(ui, icons::ARROW_U_UP_RIGHT, tr!("Redo"), "edit.redo");
        ui.separator();
        let any = !self.playlist_selection().is_empty();
        if ui.add(egui::Button::new(format!("{}  {}", icons::CROSSHAIR, tr!("Select every clip"))).shortcut_text("Ctrl+A")).clicked() {
            self.select_all_clips();
            ui.close();
        }
        for (icon, label, keys, what) in [(icons::COPY, tr!("Duplicate the clips"), "Ctrl+D", 0), (icons::SPEAKER_SLASH, tr!("Mute / unmute the clips"), "0", 1), (icons::TRASH, tr!("Delete the clips"), tr!("Del"), 2)] {
            if ui.add_enabled(any, egui::Button::new(format!("{icon}  {label}")).shortcut_text(keys)).clicked() {
                match what {
                    0 => self.duplicate_clips(),
                    1 => self.mute_clips(),
                    _ => self.delete_clips(),
                }
                ui.close();
            }
        }
        ui.add(egui::Label::new(RichText::new(tr!("Ctrl+C / X / V: copy, cut, paste")).small().color(self.theme.fg_muted)).truncate());
        ui.separator();
        if ui.add(egui::Button::new(format!("{}  {}", icons::KEYBOARD, tr!("Keyboard shortcuts"))).shortcut_text("F1")).clicked() {
            self.show_help = true;
            ui.close();
        }
        self.menu_command(ui, icons::GEAR_SIX, tr!("Preferences"), "app.preferences");
    }

    fn create_menu(&mut self, ui: &mut Ui) {
        self.menu_command(ui, icons::SQUARES_FOUR, tr!("New pattern"), "pattern.add");
        if ui.button(format!("{}  {}", icons::WAVE_SAWTOOTH, tr!("New VA-1 channel"))).clicked() {
            self.run("channel.add", json!({}));
            ui.close();
        }
        if ui.button(format!("{}  {}", icons::WAVEFORM, tr!("New channel from a sound…"))).clicked() {
            if let Some(file) = rfd::FileDialog::new().add_filter(tr!("Audio or instrument"), &crate::browser::AUDIO_EXTENSIONS).add_filter("SFZ / Decent Sampler", &ravune_core::INSTRUMENT_EXTENSIONS).pick_file() {
                self.run("channel.add", json!({ "sample": file.to_string_lossy() }));
            }
            ui.close();
        }
        ui.menu_button(format!("{}  {}", icons::MUSIC_NOTE, tr!("VA-1 from a preset")), |ui| {
            for (name, _) in ravune_core::SYNTH_PRESETS {
                if ui.button(name).clicked() {
                    self.run("channel.add", json!({ "preset": name }));
                    ui.close();
                }
            }
        });
        ui.separator();
        let r = ui.button(format!("{}  {}", icons::TREE_STRUCTURE, tr!("Song from the patterns")));
        self.hint(&r, tr!("Lays your patterns out as a song: intro, build, drop, break, drop 2, outro, with markers"));
        if r.clicked() {
            self.run("song.structure", json!({}));
            ui.close();
        }
        if ui.button(format!("{}  {}", icons::FADERS, tr!("New mixer track"))).clicked() {
            self.run("mixer.add_track", json!({}));
            ui.close();
        }
        if ui.button(format!("{}  {}", icons::PLUGS, tr!("Plugins…"))).clicked() {
            self.browser.tab = crate::browser::Tab::Plugins;
            self.settings.show_browser = true;
            ui.close();
        }
    }

    fn view_menu(&mut self, ui: &mut Ui) {
        for (_, icon, label, command) in EDITORS {
            self.menu_command(ui, icon, i18n::t(label), command);
        }
        ui.separator();
        for (label, command, on) in [(tr!("Browser"), "app.browser", self.settings.show_browser), (tr!("Editor area"), "app.editors", self.settings.show_editor), (tr!("On-screen keys"), "app.keys", self.settings.show_keys), (tr!("Visualizer"), "app.visualizer", self.settings.viz.show), (tr!("AI chat"), "app.chat", self.settings.show_chat)] {
            let mut v = on;
            if ui.checkbox(&mut v, label).changed() {
                self.command(command);
            }
        }
        ui.separator();
        ui.checkbox(&mut self.playlist.follow, tr!("Playlist follows the playhead"));
        ui.checkbox(&mut self.playlist.snap, tr!("Playlist snaps to the grid"));
        ui.separator();
        self.menu_command(ui, icons::MAGNIFYING_GLASS, tr!("Find a command"), "app.palette");
    }

    fn file_menu(&mut self, ui: &mut Ui) {
        let items: [(&str, &str, &str); 7] = [
            (icons::FILE_PLUS, tr!("New"), "app.new"),
            (icons::FOLDER_OPEN, tr!("Open…"), "app.open"),
            (icons::FLOPPY_DISK, tr!("Save"), "app.save"),
            (icons::FLOPPY_DISK, tr!("Save As…"), "app.save_as"),
            (icons::CLOCK_COUNTER_CLOCKWISE, tr!("Versions…"), "app.versions"),
            (icons::EXPORT, tr!("Export"), "app.export"),
            (icons::GLOBE, tr!("Export credits…"), "app.credits"),
        ];
        for (i, (icon, label, command)) in items.into_iter().enumerate() {
            if i == 2 {
                let recent = self.settings.recent.clone();
                ui.menu_button(format!("{}  {}", icons::CLOCK_COUNTER_CLOCKWISE, tr!("Open Recent")), |ui| {
                    if recent.is_empty() {
                        ui.label(RichText::new(tr!("Nothing yet: projects you open or save show up here")).small());
                    }
                    for path in &recent {
                        let p = std::path::Path::new(path);
                        let name = p.file_stem().map_or(path.clone(), |s| s.to_string_lossy().into_owned());
                        let folder = p.parent().and_then(|d| d.file_name()).map_or(String::new(), |d| d.to_string_lossy().into_owned());
                        let r = ui.add(egui::Button::new(format!("{}  {name}", icons::FILE)).shortcut_text(folder));
                        if r.clicked() {
                            self.request(files::Pending::Open(p.to_path_buf()));
                            ui.close();
                        }
                        r.on_hover_text(path);
                    }
                    if !recent.is_empty() {
                        ui.separator();
                        if ui.button(format!("{}  {}", icons::TRASH, tr!("Clear the list"))).clicked() {
                            self.settings.recent.clear();
                            ui.close();
                        }
                    }
                });
                ui.menu_button(format!("{}  {}", icons::STACK, tr!("New from Template")), |ui| {
                    for (name, text) in files::TEMPLATES {
                        if ui.button(name).clicked() {
                            self.request(files::Pending::Bundled(text));
                            ui.close();
                        }
                    }
                    // and the ones extensions bring
                    let more = crate::extensions::files("templates");
                    if !more.is_empty() {
                        ui.separator();
                    }
                    for path in more {
                        let name = path.file_stem().map_or(String::new(), |s| s.to_string_lossy().into_owned());
                        if ui.button(format!("{}  {name}", icons::PUZZLE_PIECE)).clicked() {
                            self.request(files::Pending::Template(path));
                            ui.close();
                        }
                    }
                });
                if ui.button(format!("{}  {}", icons::PUZZLE_PIECE, tr!("Install Extension…"))).clicked() {
                    ui.close();
                    self.install_extension_file(ui.ctx());
                }
            }
            if i == 2 || i == 5 {
                ui.separator();
            }
            let keys = self.settings.keymap.keys(command).first().map(keymap::text).unwrap_or_default();
            if command == "app.export" {
                // one entry per format; the last one used carries the export key
                ui.menu_button(format!("{icon}  {label}"), |ui| {
                    for format in crate::export::Format::ALL {
                        let key = if format == self.settings.export_format { keys.clone() } else { String::new() };
                        let r = ui.add(egui::Button::new(format!("{}…", format.label())).shortcut_text(key));
                        self.hint(&r, format.hint());
                        if r.clicked() {
                            ui.close();
                            self.export_as(format);
                        }
                    }
                    ui.separator();
                    let r = ui.add(egui::Button::new(format!("{}  {}", icons::FILE_ARROW_UP, tr!("DAWproject…"))));
                    self.hint(&r, tr!("The song for Bitwig, Studio One, Cubase and other DAWs: notes, audio, the mixer"));
                    if r.clicked() {
                        ui.close();
                        self.export_dawproject();
                    }
                    let r = ui.add_enabled(self.session.loop_region.is_some(), egui::Button::new(format!("{}  {}", icons::REPEAT, tr!("Loop region…"))));
                    self.hint(&r, tr!("Only the loop region, in the format picked last, with its tail"));
                    if r.clicked() {
                        ui.close();
                        self.export_loop();
                    }
                    let r = ui.add(egui::Button::new(format!("{}  {}", icons::STACK, tr!("Stems…"))));
                    self.hint(&r, tr!("A file per mixer track, in the format picked last, with its reverb and delay tails"));
                    if r.clicked() {
                        ui.close();
                        self.export_stems();
                    }
                });
                continue;
            }
            let r = ui.add(egui::Button::new(format!("{icon}  {label}")).shortcut_text(keys));
            if command == "app.versions" {
                self.hint(&r, tr!("Every save is kept: go back to how the song was before"));
            }
            if command == "app.credits" {
                self.hint(&r, tr!("A text file crediting every online sound this project uses, with its license"));
            }
            if r.clicked() {
                self.command(command);
            }
        }
    }

    // ---- status bar and keys ------------------------------------------------

    /// Ravune's loads for the status bar, as far as this system tells them, each with what its
    /// hover says: the CPU, memory, GPU and video memory the app takes (the audio engine's own
    /// load is in the top bar).
    fn loads(&self) -> Vec<(usage::Load, String)> {
        let u = &self.usage;
        let size = |mb: f32| if mb >= 1000.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{mb:.0} MB") };
        let mb = |bytes: u64| (bytes >> 20) as f32;
        let trail = |t: &std::collections::VecDeque<f32>| t.iter().copied().collect::<Vec<f32>>();
        // memory against its own highest with room above: a share of all of it would lie flat
        let relative = |t: &std::collections::VecDeque<f32>| {
            let top = t.iter().copied().fold(0.0, f32::max) * 1.25;
            t.iter().map(|v| if top > 0.0 { v / top } else { 0.0 }).collect::<Vec<f32>>()
        };
        let mut loads = Vec::new();
        if let Some(cpu) = u.cpu {
            let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
            let tip = tr!("Ravune's share of the whole CPU ({} threads), audio and interface together", cores);
            loads.push((usage::Load { name: "CPU", value: cpu, format: |v| format!("{v:.0}%"), room: "100%", strain: cpu / 100.0, warn: 0.6, high: 0.85, trail: trail(&u.trails.cpu) }, tip));
        }
        if let Some(memory) = u.memory {
            let strain = u.trails.memory.back().copied().unwrap_or(0.0);
            let tip = match u.memory_total {
                Some(total) => tr!("Ravune's memory: {} of the {} this computer has", size(mb(memory)), size(mb(total))),
                None => tr!("Ravune's memory: {}", size(mb(memory))),
            };
            loads.push((usage::Load { name: "RAM", value: mb(memory), format: size, room: "999 MB", strain, warn: 0.5, high: 0.75, trail: relative(&u.trails.memory) }, tip));
        }
        if let Some(gpu) = u.gpu {
            let tip = tr!("How busy Ravune keeps the GPU, as Task Manager counts it").to_string();
            loads.push((usage::Load { name: "GPU", value: gpu, format: |v| format!("{v:.0}%"), room: "100%", strain: gpu / 100.0, warn: 0.7, high: 0.9, trail: trail(&u.trails.gpu) }, tip));
        }
        if let Some(vram) = u.vram {
            let strain = u.trails.vram.back().copied().unwrap_or(0.0);
            let tip = match u.vram_budget.filter(|b| *b > 0) {
                Some(budget) => tr!("Video memory Ravune holds: {} of the {} the system lets it use", size(mb(vram)), size(mb(budget))),
                None => tr!("Video memory Ravune holds: {}", size(mb(vram))),
            };
            loads.push((usage::Load { name: "VRAM", value: mb(vram), format: size, room: "999 MB", strain, warn: 0.7, high: 0.9, trail: relative(&u.trails.vram) }, tip));
        }
        loads
    }

    pub fn status_bar(&mut self, ui: &mut Ui) {
        let t = self.theme.clone();
        kit::strip(ui, &t, 24.0, t.bg_field, |ui| {
            let tip = tr!("Tip: F1 lists every shortcut · {} finds any command", self.settings.keymap.label("app.palette"));
            // what's under the mouse, else how the area under it works, else a tip
            let text = [&self.info, &self.info_zone].into_iter().find(|s| !s.is_empty()).unwrap_or(&tip);
            ui.add(egui::Label::new(RichText::new(text).color(t.fg_muted).small()).truncate());
            kit::right(ui, |ui| {
                let r = kit::icon_button(ui, &t, icons::WAVEFORM, &self.tip(tr!("Visualizer"), "app.visualizer"), self.settings.viz.show);
                if r.clicked() {
                    self.settings.viz.show = !self.settings.viz.show;
                }
                let r = kit::icon_button(ui, &t, icons::KEYBOARD, &self.tip(tr!("On-screen keys"), "app.keys"), self.settings.show_keys);
                if r.clicked() {
                    self.settings.show_keys = !self.settings.show_keys;
                }
                let r = kit::icon_button(ui, &t, icons::SIDEBAR_SIMPLE, &self.tip(tr!("Browser"), "app.browser"), self.settings.show_browser);
                if r.clicked() {
                    self.settings.show_browser = !self.settings.show_browser;
                }
                kit::vsep(ui, &t);
                // the app's own load, a chip each (right to left: the last first)
                for (load, tip) in self.loads().iter().rev() {
                    let r = usage::chip(ui, &t, load);
                    self.hint(&r, tip);
                }
                kit::vsep(ui, &t);
                // agents: shown once turned on, lit while one is connected
                if let Some(n) = self.agents_connected() {
                    let last = self.agent.log.last().map_or(String::new(), |l| format!(" · {}", l.what));
                    let tip = if n > 0 { tr!("{} agent(s) connected{}", n, last) } else { tr!("Agents may connect (MCP){}", last) };
                    // it lights up while an agent is at work
                    let busy = self.agent.log.last().is_some_and(|l| ui.input(|i| i.time) - l.at < 0.6);
                    if kit::icon_button(ui, &t, icons::ROBOT, &tip, n > 0 || busy).clicked() {
                        (self.show_settings, self.prefs_tab) = (true, PrefsTab::Agents);
                    }
                }
                let names: Vec<&str> = self.midi.devices().collect();
                let (text, color) = if names.is_empty() { (tr!("no MIDI").to_string(), t.fg_muted) } else { (names.join(", "), t.select) };
                let r = ui.label(RichText::new(format!("{}  {text}", icons::PIANO_KEYS)).small().color(color));
                self.hint(&r, tr!("MIDI controllers are detected automatically, even while Ravune is running."));
                let target = self.session.selected_channel().map_or(tr!("None").to_string(), |c| c.name.clone());
                let keys = format!("{}  {target} · C{} · vel {:.0}", icons::KEYBOARD, self.settings.octave, self.settings.velocity * 127.0);
                let r = ui.label(RichText::new(keys).small().color(t.fg_text));
                self.hint(&r, tr!("The computer keyboard plays this channel: A–K notes, Z / X octave, C / V velocity."));
            });
        });
    }

    pub fn keyboard_panel(&mut self, ui: &mut Ui) {
        let t = self.theme.clone();
        let base = self.base_note();
        kit::padded(ui, &t, |ui| {
            let engine = &self.engine;
            let held = |n: u8| engine.as_ref().is_some_and(|e| e.stats().is_held(n));
            let label = |n: u8| n.checked_sub(base).and_then(|o| PIANO_KEYS.iter().find(|k| k.1 == o)).map(|k| k.2);
            let input = widgets::piano(ui, &t, base.saturating_sub(12), 3, held, label, &mut self.mouse_note);
            if let Some(n) = input.release {
                self.note_off(n);
            }
            if let Some(n) = input.press {
                self.note_on(n);
            }
        });
    }

    // ---- editors: docked tabs and floating windows --------------------------

    /// The editor tab bar; on its own when the editor area is folded.
    pub fn editor_tabs(&mut self, ui: &mut Ui) {
        let t = self.theme.clone();
        kit::region(ui);
        let docked: Vec<(Tab, &str, &str)> = EDITORS.iter().map(|e| (e.0, e.1, i18n::t(e.2))).collect();
        let split = self.split.is_some() && self.settings.show_editor;
        kit::bar(ui, &t, |ui| {
            if let Some(tab) = kit::tabs(ui, &t, Id::unique("editor-tabs"), self.tab, &docked, split) {
                if self.split == Some(tab) {
                    self.split = Some(self.tab); // the two sides trade places
                }
                self.tab = tab;
                self.show_editor(tab);
                self.settings.show_editor = true;
            }
            kit::right(ui, |ui| {
                let open = self.settings.show_editor;
                let icon = if open { icons::CARET_DOWN } else { icons::CARET_UP };
                let tip = self.tip(if open { tr!("Fold the editors away") } else { tr!("Open the editors") }, "app.editors");
                if kit::icon_button(ui, &t, icon, &tip, false).clicked() {
                    self.settings.show_editor = !open;
                }
                let r = kit::icon_button(ui, &t, icons::SQUARE_SPLIT_HORIZONTAL, tr!("Split: a second editor beside this one"), self.split.is_some());
                if r.clicked() {
                    self.split = match self.split {
                        Some(_) => None,
                        None => EDITORS.iter().map(|e| e.0).find(|e| *e != self.tab && !self.settings.floating.contains(e)),
                    };
                    self.settings.show_editor = true;
                }
                let window = EDITORS.iter().find(|e| e.0 == self.tab).map_or(String::new(), |e| e.3.replacen("editor.", "window.", 1));
                let r = kit::icon_button(ui, &t, icons::ARROW_SQUARE_OUT, &self.tip(tr!("Open this editor in a window"), &window), false);
                if r.clicked() {
                    self.toggle_window(self.tab);
                }
            });
        });
    }

    /// The toast (Velo's, after Sileo, squared off): a card in the bottom-right corner, a
    /// square badge and its tone's title in its color, a lit edge on the left, the message, a
    /// button when it has one. It springs in, holds while hovered, and goes when clicked.
    pub fn toast_ui(&mut self, ctx: &egui::Context) {
        let t = self.theme.clone();
        let Some(toast) = &mut self.toast else { return };
        let now = Instant::now();
        let age = (now - toast.born).as_secs_f32();
        if age > TOAST_LIFE + TOAST_ENTER {
            self.toast = None;
            return;
        }
        let (color, icon, title) = match toast.tone {
            Tone::Info => (t.accent, icons::INFO, None),
            Tone::Done => (t.meter_ok, icons::CHECK, Some(tr!("Done"))),
            Tone::Action => (t.select, icons::LIGHTNING, Some(tr!("Your turn"))),
            Tone::Error => (t.meter_hot, icons::WARNING, Some(tr!("Error"))),
        };
        // in: up from 8 px below, from 96 %, with a little bounce; out: down and faded
        let (k_in, k_out) = (kit::spring(age / TOAST_ENTER), kit::ease((age - TOAST_LIFE) / TOAST_ENTER));
        let (opacity, scale) = (kit::ease(age / TOAST_ENTER) * (1.0 - k_out), 0.96 + 0.04 * k_in);
        let corner = ctx.content_rect().right_bottom() - vec2(t.pad * 2.0, 24.0 + t.pad * 2.0);
        let shift = vec2(0.0, 8.0 * (1.0 - k_in) + 8.0 * k_out);
        let action = toast.action.clone();
        let out = egui::Area::new(Id::unique("toast")).order(egui::Order::Foreground).fixed_pos(corner).pivot(Align2::RIGHT_BOTTOM).show(ctx, |ui| {
            ui.multiply_opacity(opacity);
            let transform = egui::emath::TSTransform::new(corner.to_vec2() * (1.0 - scale) + shift, scale);
            ui.with_visual_transform(transform, |ui| {
                let frame = egui::Frame::new()
                    .fill(t.bg_device)
                    .stroke(Stroke::new(1.0, Theme::mix(t.line, color, 0.35)))
                    .corner_radius(t.radius_md)
                    .inner_margin(egui::Margin { left: 14, right: 12, top: 9, bottom: 9 })
                    .shadow(ctx.global_style().visuals.popup_shadow)
                    .show(ui, |ui| {
                        ui.set_max_width(TOAST_WIDTH);
                        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                        let mut clicked = false;
                        ui.horizontal(|ui| {
                            // the badge: a square of its color, its icon in it
                            let (badge, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                            ui.painter().rect_filled(badge, t.radius_sm, color.gamma_multiply(0.2));
                            kit::icon_at(ui.painter(), badge.center(), icon, 12.0, color);
                            if let Some(title) = title {
                                ui.label(RichText::new(title).color(color));
                            }
                            let text = egui::Label::new(RichText::new(&toast.text).color(t.fg_text));
                            if title.is_none() {
                                ui.add(text.wrap());
                            }
                        });
                        if title.is_some() {
                            ui.add(egui::Label::new(RichText::new(&toast.text).color(t.fg_muted)).wrap());
                        }
                        if let Some((label, _)) = &action {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                clicked = kit::action(ui, &t, kit::Variant::Outline, Some(icons::ARROW_U_UP_LEFT), label).clicked();
                            });
                        }
                        clicked
                    });
                // the lit edge, as a meter's
                let rect = frame.response.rect;
                ui.painter().rect_filled(egui::Rect::from_min_size(rect.min, vec2(2.0, rect.height())), 0, color);
                (frame.inner, rect)
            })
            .inner
        });
        let (clicked, rect) = out.inner;
        let hit = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p));
        let toast = self.toast.as_mut().expect("still there");
        if clicked {
            self.toast_acted = action.map(|a| a.1);
            toast.born = now - Duration::from_secs_f32(TOAST_LIFE.max(age));
        } else if out.response.clicked() {
            // a click on it: it goes
            toast.born = now - Duration::from_secs_f32(TOAST_LIFE.max(age));
        } else if hit && age > TOAST_LIFE - 1.0 && age < TOAST_LIFE {
            // held while hovered: the clock waits a second short of going
            toast.born = now - Duration::from_secs_f32(TOAST_LIFE - 1.0);
        }
        // frames while it moves; in between, wake for when it goes
        let settled = age > TOAST_ENTER && age < TOAST_LIFE;
        ctx.request_repaint_after(if settled && !hit { Duration::from_secs_f32(TOAST_LIFE - age) } else { Duration::from_millis(16) });
    }

    pub fn editor_area(&mut self, ui: &mut Ui) {
        let t = self.theme.clone();
        if let Some(split) = self.split.filter(|s| *s != self.tab) {
            let full = ui.available_width();
            egui::Panel::right("editor-split")
                .frame(egui::Frame::new().fill(t.bg_base))
                .resizable(true)
                .default_size(full * 0.5)
                .size_range(full * 0.25..=full * 0.75)
                .show(ui, |ui| self.split_area(ui, split));
        }
        self.editor_tabs(ui);
        // switching editors crossfades the content in
        let now = ui.input(|i| i.time);
        if self.last_tab != self.tab {
            (self.last_tab, self.tab_since) = (self.tab, now);
        }
        let k = if t.motion > 0.0 { ((now - self.tab_since) as f32 / 0.2).min(1.0) } else { 1.0 };
        if k < 1.0 {
            ui.ctx().request_repaint();
        }
        ui.multiply_opacity(kit::ease(k));
        if self.settings.floating.contains(&self.tab) {
            kit::padded(ui, &t, |ui| {
                ui.label(RichText::new(tr!("This editor is open in its own window.")).color(t.fg_muted));
                if kit::button(ui, &t, Some(icons::ARROW_SQUARE_IN), tr!("Dock it back here"), false).clicked() {
                    self.toggle_window(self.tab);
                }
            });
            return;
        }
        self.editor(ui, self.tab);
    }

    pub fn editor(&mut self, ui: &mut Ui, tab: Tab) {
        let pattern_beat = self.beat.filter(|_| !self.session.song_mode);
        match tab {
            // (Device was a tab of its own: saved layouts that still have it get the rack)
            Tab::Rack | Tab::Device => {
                let now_step = pattern_beat.map(|b| (b * 4.0) as usize % self.session.current_pattern().length);
                self.channel_rack(ui, now_step);
            }
            Tab::PianoRoll => self.piano_roll(ui, pattern_beat.map(|b| b * PPQ as f64)),
            Tab::Mixer => self.mixer(ui),
            Tab::Modulation => self.modulation(ui),
        }
    }

    /// Brings an editor up: focuses its window if it floats, else selects its tab.
    pub fn show_editor(&mut self, tab: Tab) {
        if self.settings.floating.contains(&tab) {
            self.raise = Some(tab);
        } else if self.split != Some(tab) {
            self.tab = tab; // (the split side already shows it otherwise)
        }
    }

    /// Pops an editor out into a floating window, or docks it back.
    pub fn toggle_window(&mut self, tab: Tab) {
        if let Some(i) = self.settings.floating.iter().position(|f| *f == tab) {
            self.settings.floating.remove(i);
            self.tab = tab;
        } else {
            self.settings.floating.push(tab);
            self.raise = Some(tab);
            if let Some(&(next, ..)) = EDITORS.iter().find(|e| !self.settings.floating.contains(&e.0)) {
                self.tab = next; // keep the dock showing something
            }
        }
    }

    /// Floating editor windows (FL-style): drag by the title bar, resize from the edges.
    pub fn windows(&mut self, ctx: &egui::Context) {
        let t = self.theme.clone();
        for (k, tab) in self.settings.floating.clone().into_iter().enumerate() {
            let Some(&(_, icon, name, command)) = EDITORS.iter().find(|e| e.0 == tab) else { continue };
            let keys = self.settings.keymap.label(command);
            let id = Id::unique(("editor-window", name));
            if self.raise == Some(tab) {
                ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, id));
                self.raise = None;
            }
            let mut close = false;
            egui::Window::new(name)
                .id(id)
                .title_bar(false)
                .frame(kit::window_frame(ctx, &t))
                .resizable(true)
                .default_size([760.0, 420.0])
                .default_pos(pos2(160.0 + 28.0 * k as f32, 120.0 + 28.0 * k as f32))
                .min_size([420.0, 220.0])
                .show(ctx, |ui| {
                    kit::region(ui);
                    kit::bar(ui, &t, |ui| {
                        ui.label(RichText::new(format!("{icon}  {}", i18n::t(name))).strong().color(t.fg_text));
                        ui.label(RichText::new(&keys).small().color(t.fg_muted));
                        kit::right(ui, |ui| {
                            close = kit::icon_button(ui, &t, icons::X, tr!("Close"), false).clicked();
                            if kit::icon_button(ui, &t, icons::ARROW_SQUARE_IN, tr!("Dock back into the editor area"), false).clicked() {
                                self.toggle_window(tab);
                            }
                        });
                    });
                    self.editor(ui, tab);
                });
            if close {
                self.settings.floating.retain(|f| *f != tab);
            }
        }
    }

    // ---- preferences (floating, not modal) ----------------------------------

    pub fn toggle_preferences(&mut self) {
        self.show_settings = !self.show_settings;
        if self.show_settings {
            self.devices = Engine::devices();
        }
    }

    pub fn preferences(&mut self, ctx: &egui::Context, now: f64) {
        if !self.show_settings {
            return;
        }
        let t = self.theme.clone();
        let mut close = false;
        egui::Window::new("Preferences")
            .id(Id::unique("preferences"))
            .title_bar(false)
            .frame(kit::window_frame(ctx, &t))
            // any size you drag it to, the same on every tab; a long tab scrolls
            .resizable(true)
            .default_size([480.0, 560.0])
            .min_size([400.0, 260.0])
            .default_pos(pos2(ctx.content_rect().right() - 520.0, 60.0))
            .show(ctx, |ui| {
                kit::region(ui);
                kit::bar(ui, &t, |ui| {
                    ui.label(RichText::new(format!("{}  {}", icons::GEAR_SIX, tr!("Preferences"))).strong());
                    let tip = self.tip(tr!("Close"), "app.preferences");
                    kit::right(ui, |ui| close = kit::icon_button(ui, &t, icons::X, &tip, false).clicked());
                });
                kit::bar(ui, &t, |ui| {
                    let items = [
                        (PrefsTab::Audio, icons::SPEAKER_HIGH, "Audio"),
                        (PrefsTab::Midi, icons::PIANO_KEYS, "MIDI"),
                        (PrefsTab::Look, icons::SPARKLE, tr!("Look")),
                        (PrefsTab::Keys, icons::KEYBOARD, tr!("Keys")),
                        (PrefsTab::Agents, icons::ROBOT, tr!("Agents")),
                        (PrefsTab::Extensions, icons::PUZZLE_PIECE, tr!("Extensions")),
                    ];
                    if let Some(tab) = kit::tabs(ui, &t, Id::unique("prefs-tabs"), self.prefs_tab, &items, false) {
                        self.prefs_tab = tab;
                    }
                });
                kit::scroll(ui, &t, "prefs", true, t.bg_base, |ui| match self.prefs_tab {
                    PrefsTab::Audio => self.prefs_audio(ui, &t, now),
                    PrefsTab::Midi => self.prefs_midi(ui, &t),
                    PrefsTab::Look => self.prefs_look(ui, &t),
                    PrefsTab::Keys => self.prefs_keys(ui, &t),
                    PrefsTab::Agents => self.prefs_agents(ui, &t),
                    PrefsTab::Extensions => self.prefs_extensions(ui, &t),
                });
            });
        if close {
            self.show_settings = false;
            self.binding = None;
        }
    }

    fn prefs_keys(&mut self, ui: &mut Ui, t: &Theme) {
        kit::section(ui, t, tr!("LAYOUT"), true, |ui| {
            let current = Preset::ALL.iter().position(|p| *p == self.settings.keymap.preset).unwrap_or(0);
            if let Some(i) = kit::segmented(ui, t, current, &Preset::ALL.map(Preset::name)) {
                self.settings.keymap = Keymap { preset: Preset::ALL[i], ..Default::default() };
                self.binding = None;
                self.say_as(Tone::Done, tr!("Keys: the {} layout", Preset::ALL[i].name()));
            }
            let note = match self.settings.keymap.changed.len() {
                0 => tr!("Click a shortcut to change it · right-click clears it.").to_string(),
                n => tr!("{} changed · picking a layout resets them.", n),
            };
            ui.label(RichText::new(note).small().color(t.fg_muted));
        });
        // a section per group, in the window's one scroll (no list scrolling inside it)
        let mut from = 0;
        while from < COMMANDS.len() {
            let group = COMMANDS[from].group;
            let to = COMMANDS[from..].iter().position(|c| c.group != group).map_or(COMMANDS.len(), |n| from + n);
            kit::section(ui, t, &i18n::t(group).to_uppercase(), false, |ui| {
                for c in &COMMANDS[from..to] {
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(RichText::new(i18n::t(c.label)).color(t.fg_text)).truncate());
                        kit::right(ui, |ui| {
                            let waiting = self.binding == Some(c.id);
                            let keys = self.settings.keymap.label(c.id);
                            let text = if waiting { tr!("Press keys… (Esc cancels)") } else if keys.is_empty() { tr!("None") } else { &keys };
                            let r = kit::button(ui, t, None, text, waiting);
                            if r.clicked() {
                                self.binding = if waiting { None } else { Some(c.id) };
                            }
                            if r.secondary_clicked() {
                                self.settings.keymap.clear(c.id);
                            }
                        });
                    });
                }
            });
            from = to;
        }
    }

    /// The app's output volume: a little horizontal fader (0..2, 1 = 0 dB, marked) and its
    /// speaker, both centred on the bar, after the mixer.
    fn volume_slider(&mut self, ui: &mut Ui, t: &Theme, width: f32) {
        const MAX: f32 = 2.0;
        let db = |v: f32| if v <= 0.0 { "-inf".to_string() } else { format!("{:+.1}", 20.0 * v.log10()) };
        // laid out right to left in the top bar, left to right in the preferences: the
        // speaker goes on the left either way
        let rtl = ui.layout().main_dir() == egui::Direction::RightToLeft;
        let speaker = |ui: &mut Ui, v: f32| {
            let (rect, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
            kit::icon_at(ui.painter(), rect.center(), if v <= 0.0 { icons::SPEAKER_X } else { icons::SPEAKER_HIGH }, 14.0, t.fg_muted);
        };
        if !rtl {
            speaker(ui, self.settings.volume);
        }
        let (rect, r) = ui.allocate_exact_size(vec2(width, 18.0), Sense::click_and_drag());
        let (left, right) = (rect.left() + 5.0, rect.right() - 5.0);
        let v = &mut self.settings.volume;
        if let Some(p) = r.interact_pointer_pos().filter(|_| r.dragged() || r.clicked()) {
            *v = ((p.x - left) / (right - left)).clamp(0.0, 1.0) * MAX;
        }
        if r.hovered() {
            let wheel = ui.input(|i| i.smooth_scroll_delta.y);
            if wheel != 0.0 {
                *v = (*v + wheel * 0.002).clamp(0.0, MAX);
                ui.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
            }
        }
        if r.double_clicked() {
            *v = 1.0;
        }
        let (y, x) = (rect.center().y, left + (right - left) * *v / MAX);
        let hot = r.hovered() || r.dragged();
        let p = ui.painter();
        p.rect_filled(egui::Rect::from_min_max(pos2(left, y - 2.0), pos2(right, y + 2.0)), CornerRadius::same(2), t.bg_field);
        p.rect_filled(egui::Rect::from_min_max(pos2(left, y - 2.0), pos2(x, y + 2.0)), CornerRadius::same(2), Theme::mix(t.bg_control, t.accent, 0.55));
        let unity = left + (right - left) / MAX;
        p.line_segment([pos2(unity, y - 5.0), pos2(unity, y + 5.0)], Stroke::new(1.0, t.line));
        let cap = egui::Rect::from_center_size(pos2(x, y), vec2(8.0, 14.0));
        p.rect_filled(cap, CornerRadius::same(t.radius_sm), if hot { Theme::mix(t.bg_control, t.fg_text, 0.25) } else { t.bg_control });
        p.rect_stroke(cap, CornerRadius::same(t.radius_sm), Stroke::new(1.0, if hot { t.fg_muted } else { t.line }), egui::StrokeKind::Inside);
        r.widget_info(|| egui::WidgetInfo::slider(true, self.settings.volume as f64, tr!("Output volume")));
        if rtl {
            speaker(ui, self.settings.volume);
        }
        self.hint(&r, tr!("Output volume {} dB: the app's own level, after the mixer (the project doesn't change) · wheel to nudge · double-click: 0 dB", db(self.settings.volume)));
    }

    fn prefs_audio(&mut self, ui: &mut Ui, t: &Theme, now: f64) {
        kit::section(ui, t, tr!("OUTPUT VOLUME"), true, |ui| {
            ui.horizontal(|ui| self.volume_slider(ui, t, 220.0));
            ui.label(RichText::new(tr!("Ravune's own level, after the mixer: how loud everything comes out of your speakers. The project's mix stays as it is.")).small().color(t.fg_muted));
        });
        let mut restart = false;
        kit::section(ui, t, tr!("OUTPUT DEVICE"), false, |ui| {
            let current = self.settings.device.clone().unwrap_or_else(|| tr!("System default").into());
            ComboBox::from_id_salt("device").selected_text(current).width(ui.available_width()).show_ui(ui, |ui| {
                restart |= ui.selectable_value(&mut self.settings.device, None, tr!("System default")).clicked();
                for d in &self.devices {
                    restart |= ui.selectable_value(&mut self.settings.device, Some(d.clone()), d).clicked();
                }
            });
            ui.horizontal(|ui| {
                restart |= kit::button(ui, t, Some(icons::ARROW_COUNTER_CLOCKWISE), tr!("Restart audio"), false).clicked();
                if kit::button(ui, t, Some(icons::MAGNIFYING_GLASS), tr!("Find devices"), false).clicked() {
                    self.devices = Engine::devices();
                }
            });
        });
        kit::section(ui, t, "BUFFER", false, |ui| {
            let name = |b: Option<u32>| b.map_or(tr!("Device default").to_string(), |n| tr!("{} samples", n));
            ComboBox::from_id_salt("buffer").selected_text(name(self.settings.buffer)).width(ui.available_width()).show_ui(ui, |ui| {
                for b in BUFFER_SIZES {
                    restart |= ui.selectable_value(&mut self.settings.buffer, b, name(b)).clicked();
                }
            });
            ui.label(RichText::new(tr!("Smaller buffer = less delay, more risk of crackles.")).small().color(t.fg_muted));
        });
        if restart {
            self.start_engine(now);
        }
        kit::section(ui, t, tr!("NOW"), false, |ui| {
            if let Some(e) = &self.engine {
                let s = e.stats();
                let ms = s.buffer_frames() as f32 / e.sample_rate as f32 * 1000.0;
                egui::Grid::new("audio-stats").num_columns(2).spacing(vec2(t.pad, t.gap)).show(ui, |ui| {
                    for (k, v) in [
                        (tr!("Device"), e.device.clone()),
                        (tr!("Sample rate"), format!("{} Hz", e.sample_rate)),
                        ("Buffer", tr!("{} frames · {} ms", s.buffer_frames(), format!("{ms:.1}"))),
                        (tr!("Audio load"), format!("{:.1}%", s.load() * 100.0)),
                        (tr!("Dropouts"), s.xruns().to_string()),
                    ] {
                        ui.label(RichText::new(k).color(t.fg_muted));
                        ui.label(RichText::new(v).font(Theme::mono(11.0)));
                        ui.end_row();
                    }
                });
            }
            if let Some(err) = &self.engine_error {
                ui.label(RichText::new(err).color(t.meter_hot));
            }
        });
    }

    fn prefs_midi(&mut self, ui: &mut Ui, t: &Theme) {
        kit::section(ui, t, tr!("MIDI INPUTS"), true, |ui| {
            let names: Vec<String> = self.midi.devices().map(String::from).collect();
            if names.is_empty() {
                ui.label(RichText::new(tr!("None. Controllers are picked up as soon as you plug them in.")).color(t.fg_muted));
            }
            for n in names {
                ui.label(RichText::new(format!("{}  {n}", icons::PIANO_KEYS)).color(t.select));
            }
        });
        self.prefs_osc(ui, t);
    }

    fn prefs_look(&mut self, ui: &mut Ui, t: &Theme) {
        // each language under its own name, so it's found whatever the current one is
        kit::section(ui, t, tr!("LANGUAGE"), true, |ui| {
            let current = Lang::ALL.iter().position(|l| *l == self.settings.language).unwrap_or(0);
            if let Some(i) = kit::segmented(ui, t, current, &Lang::ALL.map(Lang::name)) {
                self.settings.language = Lang::ALL[i];
                i18n::set(Lang::ALL[i]);
            }
        });
        kit::section(ui, t, tr!("WAVEFORMS"), false, |ui| {
            let looks = [WaveStyle::Mono, WaveStyle::Rgb, WaveStyle::ThreeBand];
            let current = looks.iter().position(|w| *w == self.settings.wave).unwrap_or(0);
            if let Some(i) = kit::segmented(ui, t, current, &["Mono", "RGB", tr!("3-Band")]) {
                self.settings.wave = looks[i];
            }
            let help = match self.settings.wave {
                WaveStyle::Mono => tr!("One color, brighter where there are more highs."),
                WaveStyle::Rgb => tr!("Red = lows, green = mids, blue = highs, mixed."),
                WaveStyle::ThreeBand => tr!("Lows, mids and highs as layers, like a CDJ."),
            };
            ui.label(RichText::new(help).small().color(t.fg_muted));
        });
        kit::section(ui, t, tr!("INTERFACE SIZE"), false, |ui| {
            let zooms = [0.9, 1.0, 1.1, 1.25, 1.5];
            let current = zooms.iter().position(|z| (*z - self.settings.zoom).abs() < 0.01).unwrap_or(1);
            if let Some(i) = kit::segmented(ui, t, current, &["90%", "100%", "110%", "125%", "150%"]) {
                self.settings.zoom = zooms[i];
                ui.ctx().set_zoom_factor(zooms[i]);
            }
        });
        kit::section(ui, t, tr!("MOTION"), false, |ui| {
            if let Some(i) = kit::segmented(ui, t, self.settings.reduce_motion as usize, &[tr!("Animations on"), tr!("Reduce motion")]) {
                self.settings.reduce_motion = i == 1;
                self.use_theme(ui.ctx(), self.settings.theme.clone());
            }
        });
        kit::section(ui, t, tr!("THEME"), false, |ui| {
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let current = self.settings.theme.name.clone();
                let mut picked = None;
                for theme in Theme::builtins().into_iter().chain(self.settings.themes.iter().cloned()) {
                    if theme_row(ui, t, &theme, theme.name == current).clicked() {
                        picked = Some(theme);
                    }
                }
                if let Some(theme) = picked {
                    self.use_theme(ui.ctx(), theme);
                }
            });
            ui.horizontal(|ui| {
                if kit::button(ui, t, Some(icons::PENCIL_SIMPLE), tr!("Edit theme…"), self.show_theme_editor).clicked() {
                    self.show_theme_editor = !self.show_theme_editor;
                }
                if kit::button(ui, t, Some(icons::FOLDER_OPEN), tr!("Import…"), false).clicked() {
                    self.import_theme(ui.ctx());
                }
            });
        });
    }

    /// Switches to `theme` (also what the editor calls on every change: it's cheap).
    pub fn use_theme(&mut self, ctx: &egui::Context, theme: Theme) {
        self.settings.theme = theme;
        self.theme = self.settings.effective_theme();
        self.theme.apply(ctx);
    }

    fn import_theme(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new().add_filter(tr!("Ravune theme"), &THEME_EXTENSIONS).pick_file() else { return };
        let read = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| ron::from_str::<Theme>(&text).map_err(|e| e.to_string()));
        match read {
            Ok(theme) => {
                self.settings.themes.retain(|t| t.name != theme.name);
                self.settings.themes.push(theme.clone());
                self.say_as(Tone::Done, tr!("Imported the theme \"{}\"", theme.name));
                self.use_theme(ctx, theme);
            }
            Err(e) => self.say_as(Tone::Error, tr!("Can't read {}: {}", path.display(), e)),
        }
    }

    fn export_theme(&mut self) {
        let theme = &self.settings.theme;
        let dialog = rfd::FileDialog::new().add_filter(tr!("Ravune theme"), &[THEME_EXTENSION]).set_file_name(format!("{}.{THEME_EXTENSION}", theme.name));
        let Some(path) = dialog.save_file() else { return };
        let text = ron::ser::to_string_pretty(theme, ron::ser::PrettyConfig::default()).unwrap_or_default();
        match std::fs::write(&path, text) {
            Ok(()) => self.say_as(Tone::Done, tr!("Saved {}", path.display())),
            Err(e) => self.say_as(Tone::Error, tr!("Can't write {}: {}", path.display(), e)),
        }
    }

    // ---- theme editor (floating) ----------------------------------------------

    pub fn theme_editor(&mut self, ctx: &egui::Context) {
        if !self.show_theme_editor {
            return;
        }
        let t = self.theme.clone();
        let mut theme = self.settings.theme.clone();
        let builtin = Theme::builtins().iter().any(|b| b.name == theme.name);
        let saved = self.settings.themes.contains(&theme);
        let mut close = false;
        let mut save = false;
        let mut export = false;
        egui::Window::new("Theme")
            .id(Id::unique("theme-editor"))
            .title_bar(false)
            .frame(kit::window_frame(ctx, &t))
            .resizable([false, true])
            .default_height(560.0)
            .default_pos(pos2(ctx.content_rect().right() - 760.0, 60.0))
            .show(ctx, |ui| {
                ui.set_width(320.0);
                kit::region(ui);
                kit::bar(ui, &t, |ui| {
                    ui.label(RichText::new(format!("{}  {}", icons::PENCIL_SIMPLE, tr!("Theme"))).strong());
                    kit::right(ui, |ui| close = kit::icon_button(ui, &t, icons::X, tr!("Close"), false).clicked());
                });
                kit::bar(ui, &t, |ui| {
                    ui.add(kit::field(&mut theme.name).desired_width(150.0)).on_hover_text(tr!("The theme's name"));
                    if let Some(i) = kit::segmented(ui, &t, theme.dark as usize, &[tr!("Light"), tr!("Dark")]) {
                        theme.dark = i == 1;
                    }
                });
                kit::bar(ui, &t, |ui| {
                    let label = if saved { tr!("Saved") } else if builtin { tr!("Save a copy") } else { tr!("Save") };
                    save = kit::button(ui, &t, Some(icons::FLOPPY_DISK), label, false).clicked();
                    export = kit::button(ui, &t, Some(icons::EXPORT), tr!("Export…"), false).clicked();
                    if kit::icon_button(ui, &t, icons::ARROW_COUNTER_CLOCKWISE, tr!("Back to Ravune Dark"), false).clicked() {
                        theme = Theme::dark();
                    }
                });
                kit::scroll(ui, &t, "theme-tokens", true, t.bg_base, |ui| {
                    kit::caps(ui, &t, tr!("COLORS"));
                    egui::Grid::new("theme-colors").num_columns(2).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
                        for (name, color) in theme.colors_mut() {
                            ui.label(RichText::new(i18n::t(name)).color(t.fg_muted));
                            egui::color_picker::color_edit_button_srgba(ui, color, egui::color_picker::Alpha::Opaque);
                            ui.end_row();
                        }
                    });
                    ui.add_space(t.pad);
                    kit::caps(ui, &t, tr!("SIZES"));
                    egui::Grid::new("theme-sizes").num_columns(2).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
                        let slider = |ui: &mut Ui, name: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, step: f64| {
                            ui.label(RichText::new(name).color(t.fg_muted));
                            ui.add(egui::Slider::new(value, range).step_by(step));
                            ui.end_row();
                        };
                        let (mut sm, mut md) = (theme.radius_sm as f32, theme.radius_md as f32);
                        slider(ui, tr!("Corners, small"), &mut sm, 0.0..=8.0, 1.0);
                        slider(ui, tr!("Corners, cards"), &mut md, 0.0..=12.0, 1.0);
                        (theme.radius_sm, theme.radius_md) = (sm as u8, md as u8);
                        slider(ui, tr!("Padding"), &mut theme.pad, 4.0..=16.0, 1.0);
                        slider(ui, tr!("Gaps"), &mut theme.gap, 0.0..=10.0, 1.0);
                        slider(ui, tr!("Bars"), &mut theme.bar_h, 24.0..=44.0, 1.0);
                        slider(ui, tr!("List rows"), &mut theme.row_h, 18.0..=32.0, 1.0);
                        slider(ui, tr!("Motion (s)"), &mut theme.motion, 0.0..=0.5, 0.01);
                    });
                });
            });
        if builtin && theme != self.settings.theme && theme.name == self.settings.theme.name {
            theme.name = tr!("{} (edited)", theme.name); // never shadow a built-in
        }
        if theme != self.settings.theme {
            self.use_theme(ctx, theme);
        }
        if save {
            let theme = self.settings.theme.clone();
            self.settings.themes.retain(|s| s.name != theme.name);
            self.say_as(Tone::Done, tr!("Saved the theme \"{}\"", theme.name));
            self.settings.themes.push(theme);
        }
        if export {
            self.export_theme();
        }
        if close {
            self.show_theme_editor = false;
        }
    }

    // ---- shortcut sheet (F1) --------------------------------------------------

    pub fn shortcut_sheet(&mut self, ctx: &egui::Context) {
        if !self.show_help {
            return;
        }
        let t = self.theme.clone();
        // the keymap's commands, then the keys that never change
        let map = &self.settings.keymap;
        let mut groups: Vec<SheetGroup> = Vec::new();
        for c in &COMMANDS {
            let keys: Vec<String> = map.keys(c.id).iter().map(keymap::text).collect();
            if keys.is_empty() {
                continue;
            }
            match groups.last_mut() {
                Some((g, list)) if *g == c.group => list.push((keys, c.label)),
                _ => groups.push((c.group, vec![(keys, c.label)])),
            }
        }
        for (group, list) in keymap::FIXED {
            groups.push((group, list.iter().map(|(k, what)| (vec![k.to_string()], *what)).collect()));
        }
        let title = tr!("Keyboard shortcuts · {} layout", map.preset.name());
        let width = (ctx.content_rect().width() - 48.0).min(1000.0);
        let ((), close) = kit::modal(ctx, &t, "shortcuts", &title, width, |ui| {
            ui.columns(3, |cols| {
                // fill the columns in order, each about a third of the lines
                let total: usize = groups.iter().map(|g| g.1.len() + 2).sum();
                let (mut col, mut lines) = (0, 0);
                for (group, list) in &groups {
                    if lines > total * (col + 1) / 3 && col < 2 {
                        col += 1;
                    }
                    lines += list.len() + 2;
                    let ui = &mut cols[col];
                    kit::caps(ui, &t, &i18n::t(group).to_uppercase());
                    for (keys, what) in list {
                        ui.horizontal(|ui| {
                            for (i, k) in keys.iter().enumerate() {
                                if i > 0 {
                                    ui.label(RichText::new("/").small().color(t.fg_muted));
                                }
                                kbd(ui, &t, k);
                            }
                            ui.add(egui::Label::new(RichText::new(i18n::t(what)).color(t.fg_text)).truncate());
                        });
                    }
                    ui.add_space(t.pad);
                }
            });
        });
        if close {
            self.show_help = false;
        }
    }

    // ---- command palette (Ctrl K) -------------------------------------------

    pub fn open_palette(&mut self) {
        self.palette = Some(crate::Palette { scripts: crate::script::list(), ..Default::default() });
    }

    pub fn palette_ui(&mut self, ctx: &egui::Context) {
        let Some(pal) = &mut self.palette else { return };
        let t = self.theme.clone();
        let session = &self.session;
        let query = pal.query.to_lowercase();
        let fits = |text: &str| query.split_whitespace().all(|w| text.to_lowercase().contains(w));
        // operators, then scripts: (label, note, keys, ready, what it runs)
        let mut matches: Vec<(String, String, String, bool, Choice)> = OPERATORS
            .iter()
            .filter(|o| fits(&format!("{} {} {}", o.label, i18n::t(o.label), o.id)))
            .map(|o| {
                let note = if o.example == "{}" { i18n::t(o.description).to_string() } else { tr!("needs input: use the UI or a script").to_string() };
                (i18n::t(o.label).to_string(), note, self.settings.keymap.label(o.id), o.example == "{}" && (o.poll)(session), Choice::Op(o.id))
            })
            .collect();
        for script in pal.scripts.iter().filter(|s| fits(&format!("script {} {}", s.name, s.about))) {
            matches.push((tr!("Script: {}", script.name), script.about.clone(), String::new(), true, Choice::Script(Box::new(script.clone()))));
        }
        let mut chosen: Option<Choice> = None;
        let (_, close) = kit::modal_flush(ctx, &t, "palette", tr!("Commands"), 560.0, |ui| {
            // the search on a padded band, the list edge to edge under a divider: its scroll
            // bar on the modal's right edge
            let r = egui::Frame::new()
                .inner_margin(t.pad)
                .show(ui, |ui| {
                    ui.add(
                        kit::field(&mut pal.query)
                            .hint_text(format!("{}  {}", icons::MAGNIFYING_GLASS, tr!("Search commands…")))
                            .desired_width(f32::INFINITY),
                    )
                })
                .inner;
            r.request_focus();
            let line = ui.available_rect_before_wrap();
            ui.painter().hline(line.x_range(), line.top() - 0.5, egui::Stroke::new(1.0, t.line));
            let (up, down, enter) = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                )
            });
            let last = matches.len().saturating_sub(1);
            pal.selected = if down {
                (pal.selected + 1).min(last)
            } else if up {
                pal.selected.saturating_sub(1)
            } else {
                pal.selected.min(last)
            };
            egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, (label, note, keys, ready, choice)) in matches.iter().enumerate() {
                    let r = kit::row(ui, &t, Id::unique(("palette-row", label)), i == pal.selected);
                    let color = if *ready { t.fg_text } else { t.fg_muted };
                    let mut painter = ui.painter_at(r.rect);
                    painter.multiply_opacity(kit::row_alpha(ui, &r));
                    let name = painter.text(r.rect.left_center() + vec2(t.pad, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(13.0), color);
                    let keys_rect = painter.text(r.rect.right_center() - vec2(t.pad, 0.0), Align2::RIGHT_CENTER, keys, Theme::mono(11.0), t.fg_text);
                    let right = if keys.is_empty() { r.rect.right() - t.pad } else { keys_rect.left() - 2.0 * t.pad };
                    let room = right - name.right() - 2.0 * t.pad;
                    kit::fit_text(&painter, pos2(right, r.rect.center().y), Align2::RIGHT_CENTER, note.as_str(), FontId::proportional(11.0), t.fg_muted, room);
                    if r.clicked() && *ready {
                        chosen = Some(choice.clone());
                    }
                    if i == pal.selected && (up || down) {
                        r.scroll_to_me(None);
                    }
                }
            });
            let foot = ui.available_rect_before_wrap();
            ui.painter().hline(foot.x_range(), foot.top() - 0.5, egui::Stroke::new(1.0, t.line));
            egui::Frame::new().inner_margin(egui::Margin::symmetric(t.pad as i8, (t.gap * 1.5) as i8)).show(ui, |ui| {
                let found = if matches.is_empty() { tr!("Nothing found").to_string() } else { tr!("{} commands", matches.len()) };
                ui.label(RichText::new(tr!("{} · ↑ ↓ choose · Enter runs · Esc closes", found)).small().color(t.fg_muted));
            });
            if enter {
                chosen = matches.get(pal.selected).filter(|m| m.3).map(|m| m.4.clone());
            }
        });
        if close || chosen.is_some() {
            self.palette = None;
        }
        match chosen {
            Some(Choice::Op(id)) => self.run(id, json!({})),
            Some(Choice::Script(script)) => self.run_script(&script),
            None => {}
        }
    }

    // ---- rename dialog --------------------------------------------------------

    pub fn rename_dialog(&mut self, ctx: &egui::Context) {
        let Some(r) = &mut self.rename else { return };
        let t = self.theme.clone();
        let title = match r.what {
            Rename::Pattern(_) => tr!("Rename pattern"),
            Rename::Channel(_) => tr!("Rename channel"),
            Rename::Track(_) => tr!("Rename mixer track"),
            Rename::Device(_) => tr!("Rename effect"),
            Rename::Macro(_) => tr!("Rename macro"),
            Rename::Workspace(_) => tr!("Rename workspace"),
            Rename::Lane(_) => tr!("Rename lane"),
            Rename::Marker(_) => tr!("Rename marker"),
        };
        let (confirm, close) = kit::modal(ctx, &t, "rename", title, 340.0, |ui| {
            let edit = ui.add(kit::field(&mut r.text).desired_width(f32::INFINITY));
            if !r.focused {
                edit.request_focus();
                r.focused = true;
            }
            let mut confirm = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            ui.horizontal(|ui| {
                confirm |= kit::button(ui, &t, Some(icons::CHECK), tr!("Rename"), true).clicked();
                ui.label(RichText::new(tr!("Enter to confirm · Esc to cancel")).small().color(t.fg_muted));
            });
            confirm
        });
        if confirm && let Some(r) = self.rename.take() {
            let name = r.text.trim().to_string();
            if !name.is_empty() {
                match r.what {
                    Rename::Pattern(i) => self.run("pattern.rename", json!({ "pattern": i, "name": name })),
                    Rename::Channel(i) => self.run("channel.set", json!({ "channel": i, "name": name })),
                    Rename::Track(i) => self.run("mixer.set", json!({ "track": i, "name": name })),
                    Rename::Device(id) => self.run("device.style", json!({ "id": id, "name": name })),
                    Rename::Macro(i) => self.run("macro.set", json!({ "index": i, "name": name })),
                    Rename::Lane(l) => self.run("lane.set", json!({ "lane": l, "name": name })),
                    Rename::Marker(k) => self.run("marker.set", json!({ "index": k, "name": name })),
                    Rename::Workspace(i) => {
                        if let Some(w) = self.settings.workspaces.get_mut(i) {
                            w.name = name;
                        }
                    }
                }
            }
        } else if close {
            self.rename = None;
        }
    }
}

