//! Clip properties: double-click an audio clip (or "Properties…" on any clip) for its gain,
//! pitch, fades, mute, how it follows the tempo, and envelopes (volume, pan and a DJ
//! filter). A window by default, or docked on the right of the playlist.
//!
//! Its sound sits on top the way a sampler channel's does ([`App::sound_view`], shared with
//! the rack): the same grid and waveform, and a handle at each end to cut the sound.

use eframe::egui::{self, Align, Align2, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, pos2, vec2};
use ravune_core::{ClipFx, ClipKind, Curve, DeviceKind, EnvTarget, PPQ, ParamInfo, STEP, Stretch, Target, bend_spots, curve_at, curve_samples, insert_point, remove_point, sample_name};
use serde_json::json;

use crate::playlist::sound_ticks;
use crate::theme::Theme;
use crate::{App, icons, kit, widgets};
use crate::shell::Tone;

const GAIN: ParamInfo = ParamInfo {
    name: "Gain",
    min: -60.0,
    max: 12.0,
    default: 0.0,
    curve: Curve::Linear,
    unit: "dB",
    help: "The clip's level.",
};
const PITCH: ParamInfo = ParamInfo {
    name: "Pitch",
    min: -24.0,
    max: 24.0,
    default: 0.0,
    curve: Curve::Linear,
    unit: "semi",
    help: "Transpose in semitones and cents: faster or slower, unless Keep length is on (then only the pitch moves). Ctrl: whole semitones, Shift: finer.",
};

/// A sound's length, rate and file: the line in the corner of its waveform.
pub fn sound_info(sound: Option<&ravune_engine::Sample>, path: &str) -> String {
    // its folder and its name: a whole path from the disk would run off the corner
    let parts: Vec<String> = std::path::Path::new(path.trim_start_matches("builtin:")).iter().rev().take(2).map(|p| p.to_string_lossy().into_owned()).collect();
    let file = parts.into_iter().rev().collect::<Vec<_>>().join("/");
    match sound {
        Some(s) => format!("{:.2} s · {:.1} kHz · {file}", s.seconds(), s.rate / 1000.0),
        None => tr!("missing: {}", file),
    }
}

/// Which end of a sound a handle cuts.
#[derive(Clone, Copy, PartialEq)]
pub enum Cut {
    Start,
    End,
}

/// How tall the sound is drawn on top of the clip and sampler panels.
pub const SOUND_H: f32 = 130.0;

/// Whose sound a panel shows: the selected sampler channel, or audio clip `i`.
#[derive(Clone, Copy, PartialEq)]
pub enum Owner {
    Channel,
    Clip(usize),
}

/// Where a sound's panel puts its own EQ and effects: its track, and whether that is shared
/// (or the master), so touching them first gives the sound a track of its own.
#[derive(Clone, Copy)]
struct SoundTrack<'a> {
    owner: Owner,
    track: usize,
    name: &'a str,
    shared: bool,
}

/// A mixer track's name ("Master" past the end).
fn current_name(names: &[String], track: usize) -> &str {
    names.get(track).map_or("Master", String::as_str)
}

/// A sound's start or end, in seconds of it; `max`: how far it reaches.
pub(crate) fn cut_info(name: &'static str, max: f32, default: f32) -> ParamInfo {
    let help = if name == "Start" { "Where the sound starts playing: the left handle on it." } else { "Where it stops: the right handle on it." };
    ParamInfo { name, min: 0.0, max: max.max(0.01), default, curve: Curve::Linear, unit: "s", help }
}

/// Classic shapes over a clip's content (`from`..`to` ticks).
pub fn shapes(target: EnvTarget, from: u32, to: u32) -> Vec<(&'static str, Vec<(u32, f32)>)> {
    let line = |a: f32, b: f32| vec![(from, a), (to, b)];
    // each beat (or bar…): low at the start, back up three quarters in, low again
    let pulse = |every: u32, low: f32, high: f32| -> Vec<(u32, f32)> {
        let mut points: Vec<(u32, f32)> = (from..to).step_by(every as usize).flat_map(|t| [(t, low), ((t + every * 3 / 4).min(to), high)]).collect();
        points.dedup_by_key(|(t, _)| *t);
        points
    };
    match target {
        EnvTarget::Volume => vec![("Rise", line(0.0, 1.0)), ("Fall", line(1.0, 0.0)), ("Pump 1/4", pulse(PPQ, 0.1, 1.0)), ("Gate 1/8", pulse(PPQ / 2, 0.0, 1.0))],
        EnvTarget::Pan => vec![("Left to right", line(0.0, 1.0)), ("Right to left", line(1.0, 0.0)), ("Auto-pan 1/2", pulse(PPQ * 2, 0.15, 0.85))],
        EnvTarget::Filter => vec![("Open up", line(0.08, 0.5)), ("Close down", line(0.5, 0.08)), ("High-pass rise", line(0.5, 0.9)), ("Wobble 1/8", pulse(PPQ / 2, 0.12, 0.45))],
    }
}

/// What a line editor edits: one of a clip's envelopes, or an automation clip's line.
#[derive(Clone, Copy)]
enum Line {
    Env(EnvTarget),
    Auto(Target),
}

/// The color an envelope is drawn in, on the clip and in the editor.
pub fn envelope_color(t: &Theme, target: EnvTarget) -> egui::Color32 {
    match target {
        EnvTarget::Volume => t.fg_text,
        EnvTarget::Pan => t.select,
        EnvTarget::Filter => t.accent,
    }
}

impl App {
    /// The properties of clip `i`: a floating window (the rest stays usable) or docked on the
    /// right of the playlist. Open, it follows the clip you select.
    pub fn clip_props(&mut self, ui: &mut Ui, i: usize) {
        let t = self.theme.clone();
        if let [one] = self.playlist_selection()[..]
            && one != i
        {
            self.playlist.props = Some(one);
            return;
        }
        let typing = ui.ctx().text_edit_focused();
        if !typing && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.playlist.props = None;
            return;
        }
        // the clip's name, then its switches as icons: all its clips, mute, snap, replace
        let header = |app: &mut Self, ui: &mut Ui, docked: bool| {
            let Some(clip) = app.session.project.clips.get(i).cloned() else { return };
            let (name, icon) = app.clip_title(&clip);
            let audio = matches!(clip.kind, ClipKind::Audio { .. });
            let same = app.session.project.clips.iter().filter(|c| c.kind == clip.kind).count();
            kit::bar(ui, &t, |ui| {
                kit::right(ui, |ui| {
                    if kit::icon_button(ui, &t, icons::X, tr!("Close (Esc)"), false).clicked() {
                        app.playlist.props = None;
                    }
                    let (place, help) = if docked { (icons::ARROW_SQUARE_OUT, tr!("Float it in a window")) } else { (icons::SIDEBAR_SIMPLE, tr!("Dock it on the right")) };
                    if kit::icon_button(ui, &t, place, help, false).clicked() {
                        app.playlist.props_docked = !docked;
                    }
                    kit::vsep(ui, &t);
                    if audio {
                        let r = kit::icon_button(ui, &t, icons::FOLDER_OPEN, tr!("Replace the sound…"), false);
                        if r.clicked()
                            && let Some(file) = rfd::FileDialog::new().add_filter("Audio", &crate::browser::AUDIO_EXTENSIONS).pick_file()
                        {
                            app.replace_clip_sound(i, &file.to_string_lossy());
                        }
                        let r = kit::icon_button(ui, &t, icons::MAGNET, tr!("Snap the cuts to 16ths"), app.settings.snap_cuts);
                        if r.clicked() {
                            app.settings.snap_cuts = !app.settings.snap_cuts;
                        }
                        app.hint(&r, tr!("On: the handles and envelope points land on 16ths (Alt: the other way round). Off: anywhere."));
                    }
                    let mute = clip.fx.mute;
                    let r = kit::icon_button(ui, &t, if mute { icons::SPEAKER_SLASH } else { icons::SPEAKER_HIGH }, if mute { tr!("Muted") } else { tr!("Mute") }, mute);
                    if r.clicked() {
                        app.session.begin_group("Mute");
                        for k in app.edited_clips(i) {
                            app.run("clip.set_fx", json!({ "index": k, "mute": !mute }));
                        }
                        app.session.end_group();
                    }
                    app.hint(&r, tr!("A muted clip stays on the playlist but doesn't play (0 on the playlist)."));
                    if audio && same > 1 {
                        let all = app.playlist.props_all;
                        let r = kit::icon_button(ui, &t, icons::LINK, &tr!("Changes go to all {} clips of this sound", same), all);
                        if r.clicked() {
                            app.playlist.props_all = !all;
                        }
                        app.hint(&r, tr!("On: what you change here goes to every clip playing this sound (one undo for all). Off: this clip only."));
                    }
                    // the name takes what's left, "…" when it doesn't fit
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(format!("{icon}  {name}")).strong()).truncate());
                        let place = tr!("bar {} · lane {}", format!("{:.2}", clip.start as f32 / (4 * PPQ) as f32 + 1.0), clip.lane + 1);
                        ui.add(egui::Label::new(RichText::new(place).small().color(t.fg_muted)).truncate());
                    });
                });
            });
        };
        if self.playlist.props_docked {
            let mut open = true;
            egui::Panel::right("clip-props").frame(egui::Frame::new().fill(t.bg_panel)).resizable(true).default_size(420.0).size_range(340.0..=760.0).show_collapsible(ui, &mut open, |ui| {
                kit::region(ui);
                header(self, ui, true);
                kit::scroll(ui, &t, "clip-props", true, t.bg_panel, |ui| self.clip_props_body(ui, &t, i));
            });
        } else {
            let ctx = ui.ctx().clone();
            egui::Window::new("Clip properties")
                .id(egui::Id::unique("clip-props-window"))
                .title_bar(false)
                .frame(kit::window_frame(&ctx, &t))
                .resizable(true)
                .default_size([520.0, 480.0])
                .min_size([380.0, 280.0])
                .default_pos(ctx.content_rect().center() - vec2(260.0, 240.0))
                .show(&ctx, |ui| {
                    kit::region(ui);
                    header(self, ui, false);
                    kit::scroll(ui, &t, "clip-props-window", true, t.bg_base, |ui| self.clip_props_body(ui, &t, i));
                });
        }
    }

    /// A new mixer track, named after the first clip's sound, and `clips` playing into it:
    /// one undo step.
    pub fn clips_to_new_track(&mut self, clips: &[usize]) {
        let name = clips.first().and_then(|i| match &self.session.project.clips.get(*i)?.kind {
            ClipKind::Audio { sample } => Some(sample_name(sample).to_string()),
            _ => None,
        });
        let before = self.session.project.mixer.len();
        self.session.begin_group("New mixer track");
        self.run("mixer.add_track", json!({ "name": name }));
        if self.session.project.mixer.len() > before {
            for &i in clips {
                self.run("clip.set_fx", json!({ "index": i, "track": before }));
            }
        }
        self.session.end_group();
    }

    /// The tempo clip `i`'s sound was made at, by its name, length and beat.
    pub fn sound_bpm(&mut self, i: usize) -> Option<f32> {
        let ClipKind::Audio { sample } = &self.session.project.clips.get(i)?.kind else { return None };
        let s = self.library.get(sample).ok()?;
        ravune_engine::detect_bpm(&s, sample_name(sample))
    }

    /// How clip `i` follows the tempo (detecting the sound's own first if it has none); the
    /// clip keeps the same part of the sound, so it grows or shrinks with it.
    pub fn follow_tempo(&mut self, i: usize, stretch: Stretch, bpm: Option<f32>, merge: &str) {
        let Some(clip) = self.session.project.clips.get(i).cloned() else { return };
        let bpm = match bpm {
            Some(bpm) => bpm,
            None if clip.fx.bpm <= 0.0 && stretch != Stretch::Off => self.sound_bpm(i).unwrap_or_else(|| {
                self.say(tr!("No steady beat in it: it keeps its own speed (type its tempo, or Fit to grid)"));
                0.0
            }),
            None => clip.fx.bpm,
        };
        let tempo = self.session.project.tempo;
        let q = clip.fx.speed(tempo) / ClipFx { stretch, bpm, ..clip.fx.clone() }.speed(tempo);
        self.run_merge("clip.set_fx", json!({ "index": i, "stretch": stretch, "bpm": bpm }), merge);
        if (q - 1.0).abs() > 1e-9 {
            let (offset, length) = ((clip.offset as f64 * q).round() as u32, ((clip.length as f64 * q).round() as u32).max(1));
            self.run_merge("clip.set", json!({ "index": i, "offset": offset, "length": length }), merge);
        }
    }

    /// Stretches clip `i`'s sound to a whole number of bars at the song's tempo (or beats,
    /// when it's shorter than about a bar), whatever tempo it was made at.
    pub fn fit_to_grid(&mut self, i: usize) {
        let Some(clip) = self.session.project.clips.get(i) else { return };
        let ClipKind::Audio { sample } = &clip.kind else { return };
        let Ok(s) = self.library.get(sample) else { return };
        let (seconds, mode) = (s.seconds() as f64, if clip.fx.stretch == Stretch::Off { Stretch::Stretch } else { clip.fx.stretch });
        let beats = seconds * self.session.project.tempo as f64 / 60.0;
        let fit = if beats >= 3.0 { (beats / 4.0).round().max(1.0) * 4.0 } else { beats.round().max(1.0) };
        self.follow_tempo(i, mode, Some((fit * 60.0 / seconds) as f32), "clip-fit");
        self.session.close_merge();
        self.say(if fit == 4.0 {
            tr!("Fits 1 bar").to_string()
        } else if fit > 4.0 {
            tr!("Fits {} bars", fit / 4.0)
        } else if fit == 1.0 {
            tr!("Fits 1 beat").to_string()
        } else {
            tr!("Fits {} beats", fit)
        });
    }

    /// The sound on top of the clip and sampler panels, drawn the same in both: the beat
    /// grid at the song's tempo, the waveform, what doesn't play shaded and a handle at each
    /// end of what does (drag one to cut the sound's start or end). `view`: the ticks across
    /// `rect`; `wave`: the sound and how many ticks it lasts; `plays`: the part that plays;
    /// `dim`: how far the waveform fades back (under a line drawn over it); `info`: what the
    /// sound is, in its top left corner. Returns the handle being dragged and the tick under
    /// the mouse.
    #[allow(clippy::too_many_arguments)]
    pub fn sound_view(&mut self, ui: &mut Ui, t: &Theme, rect: Rect, view: (f64, f64), wave: Option<(&ravune_engine::Sample, f64)>, plays: (f64, f64), dim: f32, info: &str) -> Option<(Cut, f64)> {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(t.radius_sm), t.bg_field);
        let x_of = |tick: f64| rect.left() + ((tick - view.0) / (view.1 - view.0).max(1.0)) as f32 * rect.width();
        let tick_at = |x: f32| view.0 + ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * (view.1 - view.0);
        // beats, bars brighter
        for b in ((view.0.max(0.0) as u32).div_ceil(PPQ)..=(view.1.max(0.0) as u32) / PPQ).map(|b| b * PPQ) {
            let x = x_of(b as f64);
            let color = if b % (4 * PPQ) == 0 { t.line } else { Theme::mix(t.bg_field, t.line, 0.4) };
            painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, color));
        }
        let out = wave.and_then(|wave| self.sound_handles(ui, t, rect, &painter, wave, plays, dim, (x_of, tick_at)));
        // what it is, in the corner, on a backing it stays readable over
        if !info.is_empty() && !ui.rect_contains_pointer(rect) {
            let text = painter.layout(info.to_string(), FontId::proportional(11.0), t.fg_muted, rect.width() - 16.0);
            let at = rect.left_top() + vec2(6.0, 4.0);
            painter.rect_filled(Rect::from_min_size(at, text.size()).expand2(vec2(4.0, 2.0)), CornerRadius::same(t.radius_sm), t.bg_field.gamma_multiply(0.85));
            painter.galley(at, text, t.fg_muted);
        }
        out
    }

    /// The waveform, what doesn't play shaded, and the two handles: [`Self::sound_view`]'s.
    #[allow(clippy::too_many_arguments)]
    fn sound_handles(&mut self, ui: &mut Ui, t: &Theme, rect: Rect, painter: &egui::Painter, (sound, ticks): (&ravune_engine::Sample, f64), plays: (f64, f64), dim: f32, (x_of, tick_at): (impl Fn(f64) -> f32, impl Fn(f32) -> f64)) -> Option<(Cut, f64)> {
        widgets::waveform(painter, t, self.settings.wave, &sound.overview, egui::Rangef::new(x_of(0.0), x_of(ticks)), rect.shrink2(vec2(0.0, 6.0)));
        if dim > 0.0 {
            painter.rect_filled(rect, CornerRadius::same(t.radius_sm), t.bg_field.gamma_multiply(dim));
        }
        let (a, b) = (x_of(plays.0).clamp(rect.left(), rect.right()), x_of(plays.1).clamp(rect.left(), rect.right()));
        let shade = t.bg_field.gamma_multiply(0.75);
        painter.rect_filled(Rect::from_x_y_ranges(rect.left()..=a, rect.y_range()), CornerRadius::ZERO, shade);
        painter.rect_filled(Rect::from_x_y_ranges(b..=rect.right(), rect.y_range()), CornerRadius::ZERO, shade);
        let mut out = None;
        for (cut, x, help) in [(Cut::Start, a, tr!("Drag to cut the start of the sound")), (Cut::End, b, tr!("Drag to cut the end of the sound"))] {
            let r = ui.interact(Rect::from_x_y_ranges(x - 5.0..=x + 5.0, rect.y_range()), ui.scope_id().with(("sound-cut", cut == Cut::Start)), Sense::drag());
            let hot = r.hovered() || r.dragged();
            painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(if hot { 2.5 } else { 1.5 }, t.select));
            // a flag at the top, pointing into what plays (clockwise either way)
            let (top, low) = (pos2(x, rect.top()), pos2(x, rect.top() + 8.0));
            let flag = if cut == Cut::Start { vec![top, top + vec2(8.0, 0.0), low] } else { vec![top, low, top - vec2(8.0, 0.0)] };
            painter.add(Shape::convex_polygon(flag, t.select, Stroke::NONE));
            if hot {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if r.dragged()
                && let Some(p) = r.interact_pointer_pos()
            {
                out = Some((cut, tick_at(p.x)));
            }
            if r.drag_stopped() {
                self.session.close_merge();
            }
            self.hint(&r, help);
        }
        out
    }

    /// Where cuts and a clip's envelope points land: on 16ths with the magnet on (Alt: the
    /// other way round), else anywhere.
    pub(crate) fn cut_grid(&self, ui: &Ui) -> u32 {
        if self.settings.snap_cuts != ui.input(|i| i.modifiers.alt) { STEP } else { 1 }
    }

    /// A handle on clip `i`'s sound dragged to `tick` (of its content): the clip trims or
    /// grows as from its edges on the playlist (on 16ths with the magnet on), and the clips of
    /// the same sound with it while the changes go to all of them.
    fn cut_clip(&mut self, ui: &Ui, i: usize, cut: Cut, tick: f64) {
        let Some(clip) = self.session.project.clips.get(i).cloned() else { return };
        let grid = self.cut_grid(ui) as f64;
        let tick = ((tick / grid).round() * grid).max(0.0) as i64;
        let to = (clip.start as i64 + tick - clip.offset as i64).max(0) as u32;
        let all = self.playlist.props_all;
        let from: Vec<(usize, u32, u32, u32)> =
            self.session.project.clips.iter().enumerate().filter(|(k, c)| *k == i || all && c.kind == clip.kind).map(|(k, c)| (k, c.start, c.length, c.offset)).collect();
        let shortest = STEP / 4;
        match cut {
            Cut::Start => {
                for (k, start, length, offset) in crate::playlist::trimmed(&from, i, to, shortest) {
                    if self.session.project.clips.get(k).is_some_and(|c| (c.start, c.length, c.offset) != (start, length, offset)) {
                        self.run_merge("clip.set", json!({ "index": k, "start": start, "length": length, "offset": offset }), "clip-cut");
                    }
                }
            }
            Cut::End => {
                for (k, length) in crate::playlist::resized(&from, i, to, shortest) {
                    if self.session.project.clips.get(k).is_some_and(|c| c.length != length) {
                        self.run_merge("clip.set", json!({ "index": k, "length": length }), "clip-cut");
                    }
                }
            }
        }
    }

    /// A clip's name and icon: its sound, its pattern or the knob it moves.
    fn clip_title(&self, clip: &ravune_core::Clip) -> (String, &'static str) {
        match &clip.kind {
            ClipKind::Audio { sample } => (sample_name(sample).to_string(), icons::WAVEFORM),
            ClipKind::Pattern { pattern } => (self.session.project.patterns.get(*pattern).map_or("?".into(), |p| p.name.clone()), icons::SQUARES_FOUR),
            ClipKind::Automation { target, .. } => (crate::mixer::target_name(&self.session.project, *target), icons::CHART_LINE),
        }
    }

    /// The clips clip `i`'s panel changes: itself, and the others of its sound while the
    /// changes go to all of them.
    fn edited_clips(&self, i: usize) -> Vec<usize> {
        let clips = &self.session.project.clips;
        let Some(kind) = clips.get(i).map(|c| &c.kind) else { return Vec::new() };
        let all = self.playlist.props_all && matches!(kind, ClipKind::Audio { .. });
        clips.iter().enumerate().filter(|(k, c)| *k == i || all && c.kind == *kind).map(|(k, _)| k).collect()
    }

    /// Another sound for clip `i` (and the others of its sound, with the changes going to all).
    fn replace_clip_sound(&mut self, i: usize, path: &str) {
        let clips = self.edited_clips(i);
        self.session.begin_group("Replace sound");
        for k in clips {
            self.run("clip.set", json!({ "index": k, "sample": path }));
        }
        self.session.end_group();
    }

    /// The other sounds playing into `track`, besides the one `owner` shows (a clip's
    /// siblings count as itself): their names, each once.
    fn sharing(&self, owner: Owner, track: usize) -> Vec<String> {
        let p = &self.session.project;
        let own = |kind: &ClipKind| matches!(owner, Owner::Clip(i) if p.clips.get(i).is_some_and(|c| c.kind == *kind));
        let mut names: Vec<String> = p.channels.iter().enumerate().filter(|(k, c)| c.track == track && !(owner == Owner::Channel && *k == self.session.selected)).map(|(_, c)| c.name.clone()).collect();
        for c in p.clips.iter().filter(|c| c.fx.track == track && !own(&c.kind)) {
            if let ClipKind::Audio { sample } = &c.kind {
                names.push(sample_name(sample).to_string());
            }
        }
        names.dedup();
        names.sort();
        names.dedup();
        names
    }

    /// Gives the sound a mixer track of its own, named after it and feeding the track it
    /// played into (so that one's effects still apply), in one undo step. It's a ghost: out
    /// of the mixer unless its ghosts are shown.
    fn own_track(&mut self, owner: Owner, track: usize, name: &str) -> Option<usize> {
        let new = self.session.project.mixer.len();
        self.session.begin_group("Own mixer track");
        self.run("mixer.add_track", json!({ "name": name, "hidden": true }));
        if self.session.project.mixer.len() > new {
            if track != 0 {
                self.run("mixer.set", json!({ "track": new, "route": track }));
            }
            match owner {
                Owner::Channel => self.run("channel.set", json!({ "track": new })),
                Owner::Clip(i) => {
                    let kind = self.session.project.clips.get(i).map(|c| c.kind.clone());
                    let all = self.playlist.props_all;
                    let clips: Vec<usize> = self.session.project.clips.iter().enumerate().filter(|(k, c)| *k == i || all && Some(&c.kind) == kind.as_ref()).map(|(k, _)| k).collect();
                    for k in clips {
                        self.run("clip.set_fx", json!({ "index": k, "track": new }));
                    }
                }
            }
        }
        self.session.end_group();
        (self.session.project.mixer.len() > new).then_some(new)
    }

    /// The track a sound's own EQ and effects go on: its track when it plays there alone,
    /// else (shared, or the master) a new one of its own, made now, feeding the old one.
    fn sound_track(&mut self, s: SoundTrack) -> usize {
        if s.shared { self.own_track(s.owner, s.track, s.name).unwrap_or(s.track) } else { s.track }
    }

    /// A basic effect for the sound: a switch and its main knobs. It is the first effect of
    /// that kind on the sound's own track (the same one the mixer shows), added the first time
    /// it's switched on (with the track, if the sound has none of its own yet) and bypassed
    /// when switched off.
    fn quick_effect(&mut self, ui: &mut Ui, t: &Theme, s: SoundTrack, kind: DeviceKind, knobs: &[usize]) {
        let dev = if s.shared { None } else { self.session.project.mixer.get(s.track).and_then(|m| m.devices.iter().find(|d| d.kind == kind)).cloned() };
        let on = dev.as_ref().is_some_and(|d| !d.bypass);
        // no name of its own: the row says which effect it is
        let info = ParamInfo { name: "", min: 0.0, max: 1.0, default: 0.0, curve: Curve::Steps, unit: "onoff", help: "" };
        let (r, v) = widgets::switch(ui, t, &info, if on { 1.0 } else { 0.0 });
        if v.is_some() {
            match &dev {
                None => {
                    let at = self.sound_track(s);
                    self.run("device.add", json!({ "track": at, "kind": kind }));
                }
                Some(d) => self.run("device.bypass", json!({ "id": d.id })),
            }
        }
        self.hint(&r, tr!("{} on this sound alone: on its own mixer track, where the mixer shows it too", crate::i18n::t(kind.name())));
        ui.add_enabled_ui(dev.is_some(), |ui| {
            if !on {
                ui.multiply_opacity(0.45);
            }
            for &p in knobs {
                let info = &kind.params()[p];
                let value = dev.as_ref().and_then(|d| d.params.get(p).copied()).unwrap_or(info.default);
                let (r, v) = widgets::param(ui, t, info, value);
                if let Some(d) = &dev {
                    self.apply_knob(&r, v, "device.set", |v| json!({ "id": d.id, "param": p, "value": v }), info);
                }
            }
        });
    }

    /// The MIX section of a sound's panel: the track it plays into, and EQ, reverb, delay and
    /// any effect for this sound alone. A sound sharing its track (or on the master) gets one
    /// of its own the first time any of them is touched, so they never change the others.
    pub(crate) fn sound_mix(&mut self, ui: &mut Ui, t: &Theme, owner: Owner, track: usize, name: &str) {
        Self::props_section(ui, t, tr!("MIX"), |ui| {
            let names: Vec<String> = self.session.project.mixer.iter().map(|m| m.name.clone()).collect();
            let others = self.sharing(owner, track);
            let s = SoundTrack { owner, track, name, shared: track == 0 || !others.is_empty() };
            ui.horizontal_wrapped(|ui| {
                kit::caps(ui, t, tr!("PLAYS INTO"));
                let salt = match owner {
                    Owner::Channel => 0,
                    Owner::Clip(i) => i + 1,
                };
                let r = egui::ComboBox::from_id_salt(("sound-track", salt)).selected_text(format!("{}  {}", icons::FADERS, current_name(&names, track))).width(150.0).show_ui(ui, |ui| {
                    for (k, n) in names.iter().enumerate() {
                        if ui.selectable_label(k == track, n).clicked() {
                            match owner {
                                Owner::Channel => self.run("channel.set", json!({ "track": k })),
                                Owner::Clip(i) => self.run("clip.set_fx", json!({ "index": i, "track": k })),
                            }
                        }
                    }
                });
                self.hint(&r.response, tr!("The mixer track this sound plays into."));
                let r = kit::button(ui, t, Some(icons::PLUS), tr!("Effect"), false);
                kit::menu(t, &r).show(|ui| {
                    for kind in DeviceKind::ALL {
                        if ui.button(crate::i18n::t(kind.name())).clicked() {
                            let at = self.sound_track(s);
                            self.run("device.add", json!({ "track": at, "kind": kind }));
                            ui.close();
                        }
                    }
                });
                self.hint(&r, tr!("An effect on this sound alone (on its own mixer track)."));
            });
            if s.shared {
                let note = if track == 0 {
                    tr!("It plays into the master: the EQ and effects here first give it a track of its own, so they touch only this sound.").to_string()
                } else {
                    let list = if others.len() > 3 { tr!("{} and {} more", others[..3].join(", "), others.len() - 3) } else { others.join(", ") };
                    tr!("{} play into {} too: the EQ and effects here first give this sound a track of its own, feeding it.", list, current_name(&names, track))
                };
                ui.label(RichText::new(note).small().color(t.fg_muted));
            }
            let eq = if s.shared { [0.0; 3] } else { self.session.project.mixer.get(track).map_or([0.0; 3], |m| m.eq) };
            let params = DeviceKind::Eq.params();
            egui::Grid::new(("sound-mix", track)).num_columns(2).spacing(vec2(t.pad, t.gap)).show(ui, |ui| {
                kit::caps(ui, t, "EQ");
                ui.horizontal(|ui| {
                    for (band, (info, key)) in [(&params[0], "low"), (&params[2], "mid"), (&params[3], "high")].into_iter().enumerate() {
                        let (r, v) = widgets::knob(ui, t, info, eq[band]);
                        let at = if v.is_some() { self.sound_track(s) } else { track };
                        self.apply_knob(&r, v, "mixer.set", |v| json!({ "track": at, key: v }), info);
                    }
                });
                ui.end_row();
                kit::caps(ui, t, tr!("REVERB"));
                ui.horizontal(|ui| self.quick_effect(ui, t, s, DeviceKind::Reverb, &[0, 3]));
                ui.end_row();
                kit::caps(ui, t, tr!("DELAY"));
                ui.horizontal(|ui| self.quick_effect(ui, t, s, DeviceKind::Delay, &[0, 1, 2, 3]));
                ui.end_row();
            });
        });
    }

    /// Whether `track` is the sound's alone (not the master, nothing else playing into it):
    /// then its panel shows the track's whole effect chain.
    pub(crate) fn owns_track(&self, owner: Owner, track: usize) -> bool {
        track != 0 && self.sharing(owner, track).is_empty()
    }

    /// A titled group of controls under a divider, edge to edge (no card: it stays compact).
    pub(crate) fn props_section(ui: &mut Ui, t: &Theme, title: &str, add: impl FnOnce(&mut Ui)) {
        kit::section(ui, t, title, false, add);
    }

    fn clip_props_body(&mut self, ui: &mut Ui, t: &Theme, i: usize) {
        let Some(clip) = self.session.project.clips.get(i).cloned() else {
            self.playlist.props = None;
            return;
        };
        let beats = |ticks: u32| ticks as f32 / PPQ as f32;
        if let ClipKind::Automation { target, points, curves } = &clip.kind {
            let (target, points, curves) = (*target, points.clone(), curves.clone());
            Self::props_section(ui, t, tr!("AUTOMATION"), |ui| {
                self.envelope_editor(ui, t, i, &clip, &points, &curves, Line::Auto(target));
                ui.horizontal_wrapped(|ui| {
                    for (label, points) in shapes(EnvTarget::Volume, clip.offset, clip.offset + clip.length) {
                        if kit::button(ui, t, None, crate::i18n::t(label), false).clicked() {
                            self.run("clip.automation", json!({ "index": i, "points": points, "curves": [] }));
                        }
                    }
                    if kit::button(ui, t, None, tr!("Flip"), false).clicked() {
                        // upside down: a stretch that bowed up bows down
                        let flipped: Vec<(u32, f32)> = points.iter().map(|(t, v)| (*t, 1.0 - v)).collect();
                        let bent: Vec<f32> = curves.iter().map(|c| -c).collect();
                        self.run("clip.automation", json!({ "index": i, "points": flipped, "curves": bent }));
                    }
                });
                ui.label(RichText::new(tr!("Moves the knob while the song plays over it; when stopped, the knob is yours. Edit the line here or right on the playlist.")).small().color(t.fg_muted));
            });
            return;
        }
        let ClipKind::Audio { sample } = &clip.kind else {
            let ClipKind::Pattern { pattern } = clip.kind else { return };
            ui.horizontal_wrapped(|ui| {
                for (icon, label, tab) in [(icons::SQUARES_FOUR, tr!("Edit in the rack"), crate::Tab::Rack), (icons::PIANO_KEYS, tr!("Edit in the piano roll"), crate::Tab::PianoRoll)] {
                    if kit::button(ui, t, Some(icon), label, false).clicked() {
                        self.run("pattern.select", json!({ "pattern": pattern }));
                        self.tab = tab;
                    }
                }
                if kit::button(ui, t, Some(icons::SPARKLE), tr!("Make unique"), false).clicked() {
                    self.run("clip.make_unique", json!({ "index": i }));
                }
            });
            return;
        };
        let sound = self.library.get(sample).ok();
        let info = sound_info(sound.as_deref(), sample);

        // the sound on top, as on a sampler's panel, with the envelope picked under it drawn over it
        let target = self.playlist.env_target;
        let env = clip.fx.envelope(target).cloned();
        match &env {
            Some(env) => self.envelope_editor(ui, t, i, &clip, &env.points, &env.curves, Line::Env(target)),
            None => {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().max(200.0), SOUND_H), Sense::hover());
                let ticks = sound.as_ref().map_or(0.0, |s| sound_ticks(s.seconds(), &clip.fx, self.session.project.tempo));
                let plays = (clip.offset as f64, (clip.offset + clip.length) as f64);
                if let Some((cut, tick)) = self.sound_view(ui, t, rect, (0.0, ticks.max(plays.1)), sound.as_deref().map(|s| (s, ticks)), plays, 0.0, &info) {
                    self.cut_clip(ui, i, cut, tick);
                }
            }
        }
        // which envelope is drawn over the sound, and shapes for it
        let labels: Vec<String> = EnvTarget::ALL.iter().map(|e| if clip.fx.envelope(*e).is_some() { format!("{} •", e.name()) } else { e.name().to_string() }).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let current = EnvTarget::ALL.iter().position(|e| *e == target).unwrap_or(0);
        ui.horizontal_wrapped(|ui| {
            kit::caps(ui, t, tr!("ENVELOPE"));
            if let Some(k) = kit::segmented(ui, t, current, &labels) {
                self.playlist.env_target = EnvTarget::ALL[k];
            }
            if env.is_none() && kit::button(ui, t, Some(icons::PENCIL_SIMPLE), tr!("Draw"), true).clicked() {
                self.run("clip.envelope", json!({ "index": i, "target": target }));
            }
            for (label, points) in shapes(target, clip.offset, clip.offset + clip.length) {
                if kit::button(ui, t, None, crate::i18n::t(label), false).clicked() {
                    self.run("clip.envelope", json!({ "index": i, "target": target, "points": points }));
                }
            }
        });
        match &env {
            Some(env) => {
                ui.horizontal(|ui| {
                    let mut shown = env.shown;
                    if ui.checkbox(&mut shown, tr!("Show on the playlist")).changed() {
                        self.run("clip.envelope", json!({ "index": i, "target": target, "shown": shown }));
                    }
                    kit::right(ui, |ui| {
                        if kit::button(ui, t, Some(icons::TRASH), tr!("Remove"), false).clicked() {
                            self.run("clip.envelope", json!({ "index": i, "target": target, "remove": true }));
                        }
                    });
                });
            }
            None => {
                ui.label(RichText::new(match target {
                    EnvTarget::Volume => tr!("Volume over the clip: swells, gates, a pumping feel. Pick a shape or draw one."),
                    EnvTarget::Pan => tr!("Pan over the clip: move the sound from side to side."),
                    EnvTarget::Filter => tr!("A DJ filter: low-pass below the middle line, high-pass above. Sweeps and wobbles."),
                }).small().color(t.fg_muted));
            }
        }

        // a sampler's order: where the sound starts and stops, pitch, level, then the clip's own
        Self::props_section(ui, t, tr!("SAMPLE"), |ui| {
            let tempo = self.session.project.tempo;
            let secs = |ticks: u32| (ticks as f64 * crate::playlist::tick_seconds(tempo) * clip.fx.speed(tempo)) as f32;
            let whole = sound.as_ref().map_or(0.0, |s| s.seconds());
            ui.horizontal_wrapped(|ui| {
                let end = clip.offset + clip.length;
                for (cut, info, at) in [(Cut::Start, cut_info("Start", whole, 0.0), clip.offset), (Cut::End, cut_info("End", whole.max(secs(end)), whole), end)] {
                    let (r, v) = widgets::knob(ui, t, &info, secs(at));
                    if let Some(s) = v {
                        self.cut_clip(ui, i, cut, sound_ticks(s, &clip.fx, tempo));
                    }
                    if r.drag_stopped() {
                        self.session.close_merge();
                    }
                    self.hint(&r, crate::i18n::t(info.help));
                }
                let (r, v) = widgets::knob(ui, t, &PITCH, clip.fx.pitch);
                self.apply_knob(&r, v, "clip.set_fx", |v| json!({ "index": i, "pitch": v }), &PITCH);
                if let Some(pitch) = v {
                    self.pitch_length(&r, i, &clip, pitch);
                }
                let (r, v) = widgets::knob(ui, t, &GAIN, clip.fx.gain);
                self.apply_knob(&r, v, "clip.set_fx", |v| json!({ "index": i, "gain": v }), &GAIN);
                let length = beats(clip.length);
                for (key, name, value) in [("fade_in", "Fade in", clip.fx.fade_in), ("fade_out", "Fade out", clip.fx.fade_out)] {
                    let info = ParamInfo { name, min: 0.0, max: length, default: 0.0, curve: Curve::Linear, unit: "beats", help: "How long the clip takes to come in / go out." };
                    let (r, v) = widgets::knob(ui, t, &info, beats(value));
                    self.apply_knob(&r, v, "clip.set_fx", |v| json!({ "index": i, key: (v * PPQ as f32).round() as u32 }), &info);
                }
            });
            ui.horizontal_wrapped(|ui| {
                kit::caps(ui, t, tr!("PLAYS"));
                if let Some(k) = kit::segmented(ui, t, clip.fx.looped as usize, &[tr!("One-shot"), tr!("Loop")]) {
                    self.run("clip.set_fx", json!({ "index": i, "looped": k == 1 }));
                }
                let what = if clip.fx.looped { tr!("The sound repeats under the clip: stretch the clip for more rounds.") } else { tr!("Once through: past the sound's end the clip is silent.") };
                ui.label(RichText::new(what).small().color(t.fg_muted));
            });
        });

        Self::props_section(ui, t, "TEMPO", |ui| {
            let keep = clip.fx.stretch == Stretch::Stretch;
            let r = kit::button(ui, t, Some(icons::TIMER), tr!("Keep length"), keep);
            if r.clicked() {
                // stretch mode, following no tempo unless it already had one
                self.follow_tempo(i, if keep { Stretch::Off } else { Stretch::Stretch }, Some(clip.fx.bpm), "clip-keep");
                self.session.close_merge();
            }
            self.hint(&r, tr!("Pitch the clip without making it faster or slower (stretch mode)."));
            let modes: Vec<&str> = Stretch::ALL.iter().map(|m| m.name()).collect();
            let current = Stretch::ALL.iter().position(|m| *m == clip.fx.stretch).unwrap_or(0);
            if let Some(k) = kit::segmented(ui, t, current, &modes) {
                self.follow_tempo(i, Stretch::ALL[k], None, "clip-stretch");
                self.session.close_merge();
            }
            ui.horizontal_wrapped(|ui| {
                kit::caps(ui, t, tr!("MADE AT"));
                let mut bpm = clip.fx.bpm as f64;
                let r = ui.add(
                    egui::DragValue::new(&mut bpm)
                        .range(0.0..=400.0)
                        .speed(0.1)
                        .custom_formatter(|v, _| if v <= 0.0 { "? BPM".into() } else { format!("{v:.1} BPM") })
                        .custom_parser(|s| s.trim().trim_end_matches(|c: char| c.is_alphabetic() || c == ' ').parse().ok()),
                );
                if r.changed() && bpm >= 20.0 {
                    self.follow_tempo(i, clip.fx.stretch, Some(bpm as f32), "clip-bpm");
                }
                if r.drag_stopped() || r.lost_focus() {
                    self.session.close_merge();
                }
                self.hint(&r, tr!("The tempo the sound was made at: drag or type it."));
                let r = kit::button(ui, t, Some(icons::METRONOME), tr!("Detect"), false);
                if r.clicked() {
                    match self.sound_bpm(i) {
                        Some(bpm) => {
                            self.follow_tempo(i, clip.fx.stretch, Some(bpm), "clip-bpm-detect");
                            self.say_as(Tone::Done, tr!("Made at {} BPM, by the sound of it", bpm));
                        }
                        None => self.say(tr!("No steady beat in it: type its tempo")),
                    }
                    self.session.close_merge();
                }
                self.hint(&r, tr!("Guess the tempo from the file name, the loop's length and its beat."));
                let r = kit::button(ui, t, Some(icons::GRID_FOUR), tr!("Fit to grid"), false);
                if r.clicked() {
                    self.fit_to_grid(i);
                }
                self.hint(&r, tr!("Stretch the sound to a whole number of bars (or beats) at the song's tempo."));
                if clip.fx.bpm > 0.0 {
                    for (label, k, help) in [("×2", 2.0, tr!("Twice as fast: detection heard half the beat")), ("÷2", 0.5, tr!("Half as fast: detection heard double the beat"))] {
                        let r = kit::button(ui, t, None, label, false);
                        if r.clicked() {
                            self.follow_tempo(i, clip.fx.stretch, Some(clip.fx.bpm * k), "clip-bpm-x");
                            self.session.close_merge();
                        }
                        self.hint(&r, help);
                    }
                }
            });
            ui.label(RichText::new(match clip.fx.stretch {
                Stretch::Off => tr!("Plays at its own speed: at another tempo than its own it drifts off the grid."),
                Stretch::Resample => tr!("Follows the song's tempo like a turntable: faster is higher."),
                Stretch::Stretch => tr!("Follows the song's tempo and keeps its pitch."),
            }).small().color(t.fg_muted));
        });

        self.sound_mix(ui, t, Owner::Clip(i), clip.fx.track, sample_name(sample));
        let track = self.session.project.clips.get(i).map_or(0, |c| c.fx.track);
        if self.owns_track(Owner::Clip(i), track) {
            ui.add_space(t.gap);
            self.chain(ui, t, track);
        }
        if self.playlist.props_all {
            self.copy_to_siblings(i, &clip);
        }
    }

    /// A new pitch changes clip `i`'s speed: it keeps the same part of its sound, so it grows
    /// or shrinks with it on the playlist (with Keep length the speed doesn't move, nor does it).
    fn pitch_length(&mut self, resp: &egui::Response, i: usize, before: &ravune_core::Clip, pitch: f32) {
        let tempo = self.session.project.tempo;
        let now = (before.offset, before.length, before.fx.speed(tempo));
        // a drag scales from where it began, so rounding to ticks doesn't creep frame by frame
        let id = resp.id.with("pitch-from");
        let (offset, length, speed) = if resp.dragged() {
            resp.ctx.data_mut(|d| {
                if resp.drag_started() {
                    d.remove::<(u32, u32, f64)>(id);
                }
                *d.get_temp_mut_or_insert_with(id, || now)
            })
        } else {
            now
        };
        let q = speed / ClipFx { pitch, ..before.fx.clone() }.speed(tempo);
        let (offset, length) = ((offset as f64 * q).round() as u32, ((length as f64 * q).round() as u32).max(1));
        if (offset, length) != (before.offset, before.length) {
            self.run_joined("clip.set", json!({ "index": i, "offset": offset, "length": length }));
        }
    }

    /// What changed on clip `i` this frame (it was `before`) goes to every other clip of
    /// the same sound too, in the same undo step: tempo and pitch first (which grows or shrinks each
    /// to keep its part of the sound), then the rest of its sound, then its envelopes.
    fn copy_to_siblings(&mut self, i: usize, before: &ravune_core::Clip) {
        let Some(now) = self.session.project.clips.get(i).cloned() else { return };
        if now.fx == before.fx {
            return;
        }
        let tempo = self.session.project.tempo;
        let siblings: Vec<usize> = self.session.project.clips.iter().enumerate().filter(|(k, c)| *k != i && c.kind == now.kind).map(|(k, _)| k).collect();
        let fx = &now.fx;
        for k in siblings {
            let c = self.session.project.clips[k].clone();
            if (c.fx.stretch, c.fx.bpm, c.fx.pitch) != (fx.stretch, fx.bpm, fx.pitch) {
                let q = c.fx.speed(tempo) / ClipFx { stretch: fx.stretch, bpm: fx.bpm, pitch: fx.pitch, ..c.fx.clone() }.speed(tempo);
                let (offset, length) = ((c.offset as f64 * q).round() as u32, ((c.length as f64 * q).round() as u32).max(1));
                self.run_joined("clip.set", json!({ "index": k, "offset": offset, "length": length }));
            }
            self.run_joined(
                "clip.set_fx",
                json!({ "index": k, "gain": fx.gain, "pitch": fx.pitch, "fade_in": fx.fade_in, "fade_out": fx.fade_out, "mute": fx.mute, "track": fx.track, "stretch": fx.stretch, "bpm": fx.bpm, "looped": fx.looped }),
            );
            for target in EnvTarget::ALL {
                match (fx.envelope(target), c.fx.envelope(target)) {
                    (Some(e), _) => self.run_joined("clip.envelope", json!({ "index": k, "target": target, "points": e.points, "shown": e.shown })),
                    (None, Some(_)) => self.run_joined("clip.envelope", json!({ "index": k, "target": target, "remove": true })),
                    (None, None) => {}
                }
            }
        }
    }

    /// The envelope over the clip's sound: click to add a point, drag to move it (Alt: off
    /// the grid), right-click to delete it.
    #[allow(clippy::too_many_arguments)]
    fn envelope_editor(&mut self, ui: &mut Ui, t: &Theme, i: usize, clip: &ravune_core::Clip, points: &[(u32, f32)], curves: &[f32], what: Line) {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(200.0), SOUND_H), Sense::click_and_drag());
        // the line lives on what the clip plays (`from`..`to`); an audio clip shows its whole
        // sound around it, with handles to cut it
        let (from, to) = (clip.offset as f64, (clip.offset + clip.length) as f64);
        let sound = match &clip.kind {
            ClipKind::Audio { sample } => self.library.get(sample).ok().map(|s| {
                let ticks = sound_ticks(s.seconds(), &clip.fx, self.session.project.tempo);
                (s, ticks)
            }),
            _ => None,
        };
        let view = sound.as_ref().map_or((from, to), |(_, ticks)| (0.0, ticks.max(to)));
        let info = match &clip.kind {
            ClipKind::Audio { sample } => sound_info(sound.as_ref().map(|(s, _)| &**s), sample),
            _ => String::new(),
        };
        if let Some((cut, tick)) = self.sound_view(ui, t, rect, view, sound.as_ref().map(|(s, ticks)| (&**s, *ticks)), (from, to), 0.55, &info) {
            self.cut_clip(ui, i, cut, tick);
        }
        let painter = ui.painter_at(rect);
        let x_of = |tick: f64| rect.left() + ((tick - view.0) / (view.1 - view.0).max(1.0)) as f32 * rect.width();
        let tick_at = |x: f32| view.0 + ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * (view.1 - view.0);
        let y_of = |v: f32| rect.bottom() - 4.0 - v * (rect.height() - 8.0);
        let v_at = |y: f32| ((rect.bottom() - 4.0 - y) / (rect.height() - 8.0)).clamp(0.0, 1.0);
        // value guides, then the neutral line
        kit::quarter_lines(&painter, egui::Rangef::new(x_of(from), x_of(to)), y_of, t.line);
        let (neutral, color) = match what {
            Line::Env(target) => (target.neutral(), envelope_color(t, target)),
            Line::Auto(_) => (0.5, t.accent),
        };
        if let Line::Env(_) = what {
            let y = y_of(neutral);
            painter.line_segment([pos2(x_of(from), y), pos2(x_of(to), y)], Stroke::new(1.0, t.fg_muted.gamma_multiply(0.4)));
        }
        let at = |tick: f64| curve_at(points, curves, tick).unwrap_or(neutral);
        let line: Vec<Pos2> = curve_samples(points, curves, from, to).into_iter().map(|(tk, v)| pos2(x_of(tk), y_of(v))).collect();
        painter.add(Shape::line(line, Stroke::new(1.5, color)));

        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        let near = |p: Pos2| points.iter().position(|(tk, v)| pos2(x_of(*tk as f64), y_of(*v)).distance(p) < 8.0);
        let spots: Vec<(usize, Pos2)> = bend_spots(points, curves).into_iter().filter(|(_, mid, _)| (from..=to).contains(mid)).map(|(k, mid, v)| (k, pos2(x_of(mid), y_of(v)))).collect();
        let near_bend = |p: Pos2| spots.iter().find(|(_, at)| at.distance(p) < 8.0).map(|(k, _)| *k);
        let hovered = pointer.and_then(near);
        let hovered_bend = pointer.filter(|_| hovered.is_none()).and_then(near_bend);
        for (k, spot) in &spots {
            kit::bend_handle(&painter, *spot, color, hovered_bend == Some(*k) || self.playlist.env_bend.is_some_and(|b| b.0 == *k));
        }
        for (k, (tk, v)) in points.iter().enumerate() {
            let at = pos2(x_of(*tk as f64), y_of(*v));
            let big = hovered == Some(k) || self.playlist.env_drag == Some(k);
            painter.circle_filled(at, if big { 5.0 } else { 3.5 }, color);
        }
        let grid = self.cut_grid(ui);
        let snap = |tick: f64| ((tick / grid as f64).round() as u32 * grid).clamp(from as u32, to as u32);
        let op = if let Line::Env(_) = what { "clip.envelope" } else { "clip.automation" };
        let set = |points: Vec<(u32, f32)>, curves: Vec<f32>| match what {
            Line::Env(target) => json!({ "index": i, "target": target, "points": points, "curves": curves }),
            Line::Auto(_) => json!({ "index": i, "points": points, "curves": curves }),
        };
        // a new point where the mouse is, in its place among the others
        let with_point = |p: Pos2| {
            let (mut pts, mut bends) = (points.to_vec(), curves.to_vec());
            let tick = snap(tick_at(p.x));
            let k = pts.iter().position(|(t, _)| *t >= tick).unwrap_or(pts.len());
            if pts.get(k).is_some_and(|(t, _)| *t == tick) {
                pts[k].1 = v_at(p.y);
            } else {
                insert_point(&mut pts, &mut bends, k, (tick, v_at(p.y)));
            }
            (k, pts, bends)
        };
        if resp.drag_started()
            && let Some(p) = ui.input(|i| i.pointer.press_origin())
        {
            self.session.close_merge();
            if let Some(k) = near(p) {
                self.playlist.env_drag = Some(k);
            } else if let Some(k) = near_bend(p) {
                self.playlist.env_bend = Some((k, curves.get(k).copied().unwrap_or(0.0), p.y));
            } else {
                // a new point where the drag began, then it follows the mouse
                let (k, pts, bends) = with_point(p);
                self.run_merge(op, set(pts, bends), "env-drag");
                self.playlist.env_drag = Some(k);
            }
        } else if resp.dragged()
            && let (Some(k), Some(p)) = (self.playlist.env_drag, pointer)
            && k < points.len()
        {
            // a point stays between its neighbours
            let lo = if k > 0 { points[k - 1].0 + 1 } else { from as u32 };
            let hi = if k + 1 < points.len() { points[k + 1].0.saturating_sub(1) } else { to as u32 };
            let mut pts = points.to_vec();
            pts[k] = (snap(tick_at(p.x)).clamp(lo, hi.max(lo)), v_at(p.y));
            if pts[k] != points[k] {
                self.run_merge(op, set(pts, curves.to_vec()), "env-drag");
            }
        } else if resp.dragged()
            && let (Some((k, bent, y0)), Some(p)) = (self.playlist.env_bend, pointer)
            && k + 1 < points.len()
        {
            // up bows the stretch up, down bows it down
            let mut bends = curves.to_vec();
            bends.resize(points.len() - 1, 0.0);
            bends[k] = (bent + (y0 - p.y) / 60.0).clamp(-1.0, 1.0);
            self.run_merge(op, set(points.to_vec(), bends), "env-bend");
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        } else if resp.double_clicked()
            && let Some(k) = pointer.and_then(near_bend)
            && k < curves.len()
        {
            let mut bends = curves.to_vec();
            bends[k] = 0.0;
            self.run(op, set(points.to_vec(), bends));
        } else if resp.clicked()
            && let Some(p) = pointer
            && near(p).is_none()
            && near_bend(p).is_none()
        {
            let (_, pts, bends) = with_point(p);
            self.run(op, set(pts, bends));
        }
        if resp.secondary_clicked()
            && let Some(k) = pointer.and_then(near)
            && points.len() > 1
        {
            let (mut pts, mut bends) = (points.to_vec(), curves.to_vec());
            remove_point(&mut pts, &mut bends, k);
            self.run(op, set(pts, bends));
        }
        // a hand over a point (it's picked up), a closed one while it moves
        if self.playlist.env_drag.is_some() && resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if hovered.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if hovered_bend.is_some() && self.playlist.env_drag.is_none() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if resp.drag_stopped() {
            (self.playlist.env_drag, self.playlist.env_bend) = (None, None);
            self.session.close_merge();
        }
        if let Some(p) = pointer.filter(|p| rect.contains(*p)) {
            let value = hovered.map_or_else(|| at(tick_at(p.x)), |k| points[k].1);
            let text = match what {
                Line::Env(target) => crate::i18n::t(&target.describe(value)).to_string(),
                Line::Auto(target) => crate::mixer::describe_target(&self.session.project, target, value),
            };
            // top right: the sound's own line is in the left corner
            painter.text(rect.right_top() + vec2(-6.0, 4.0), Align2::RIGHT_TOP, text, FontId::monospace(10.0), t.fg_text);
        }
        self.hint(&resp, tr!("Click to add a point, drag to move it (Alt: off the grid), right-click to delete it · drag a ring up or down to bend the line, double-click it to straighten it."));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sound_on_a_shared_track_gets_its_own() {
        let mut app = App::for_tests();
        let drums = app.session.project.mixer.len();
        app.session.run("mixer.add_track", json!({ "name": "Drums" })).unwrap();
        for ch in [0, 1] {
            app.session.run("channel.set", json!({ "channel": ch, "track": drums })).unwrap();
        }
        app.session.selected = 0;
        let name = app.session.project.channels[0].name.clone();
        let own = app.sound_track(SoundTrack { owner: Owner::Channel, track: drums, name: &name, shared: true });
        assert_eq!(own, drums + 1);
        assert_eq!(app.session.project.channels[0].track, own);
        assert_eq!(app.session.project.channels[1].track, drums, "the other sound stays");
        assert_eq!(app.session.project.mixer[own].route, drums, "it feeds the track it was in");
        assert!(app.session.project.mixer[own].hidden, "a ghost: out of the mixer unless shown");
        assert!(app.owns_track(Owner::Channel, own) && !app.owns_track(Owner::Channel, 0));
        assert_eq!(app.sound_track(SoundTrack { owner: Owner::Channel, track: own, name: &name, shared: false }), own, "its own: no new one");
    }
}
