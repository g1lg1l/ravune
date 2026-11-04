//! Mixer (FL-style inserts: pan, EQ, fader, route, sends), effect chains, and the modulation
//! page (macros, LFOs, mappings). Every edit is an operator.

use eframe::egui::{
    self, Align, Align2, Color32, ComboBox, CornerRadius, FontId, Frame, Layout, Pos2, Rect, Response, RichText, ScrollArea, Sense,
    Shape, Stroke, StrokeKind, Ui, UiBuilder, epaint::CubicBezierShape, pos2, vec2,
};
use ravune_core::{BAR, Clip, ClipKind, Curve, Device, DeviceKind, Instrument, MAX_LFOS, MAX_TRACKS, PATTERN_COLORS, PPQ, ParamInfo, Project, Source, Target};
use serde_json::json;

use crate::rack::label;
use crate::shell::{Rename, Renaming};
use crate::theme::Theme;
use crate::{App, icons, kit, widgets};
use crate::shell::Tone;

const STRIP_W: f32 = 108.0;
/// The patch bay under the strips, where the cables hang.
const CABLES_H: f32 = 64.0;

const PAN: ParamInfo = ParamInfo {
    name: "Pan",
    min: -1.0,
    max: 1.0,
    default: 0.0,
    curve: Curve::Linear,
    unit: "pan",
    help: "Position in the stereo field, left to right.",
};
const VOLUME: ParamInfo = ParamInfo {
    name: "Track Volume",
    min: 0.0,
    max: 1.25,
    default: 1.0,
    curve: Curve::Linear,
    unit: "dB",
    help: "Track fader. The meter beside it shows the level after the fader.",
};
const SEND: ParamInfo = ParamInfo {
    name: "Send",
    min: 0.0,
    max: 1.0,
    default: 0.25,
    curve: Curve::Linear,
    unit: "%",
    help: "How much of this track also goes to the other one, after the fader. Right-click the row to remove the send.",
};
const MACRO: ParamInfo = ParamInfo {
    name: "",
    min: 0.0,
    max: 1.0,
    default: 0.0,
    curve: Curve::Linear,
    unit: "%",
    help: "One knob that moves every parameter mapped to it. Alt+drag it onto a knob to map it there; right-click to rename.",
};
const RATE: ParamInfo = ParamInfo {
    name: "Rate",
    min: 0.05,
    max: 20.0,
    default: 1.0,
    curve: Curve::Log,
    unit: "Hz",
    help: "LFO speed in cycles per second.",
};
const LENGTH: ParamInfo = ParamInfo {
    name: "Length",
    min: 0.25,
    max: 16.0,
    default: 1.0,
    curve: Curve::Log,
    unit: "beats",
    help: "One LFO cycle every this many beats, locked to the tempo and the bar.",
};

/// Icons a track or an effect can wear, by the name the project keeps.
const LOOKS: [(&str, &str); 28] = [
    ("waveform", icons::WAVEFORM),
    ("synth", icons::WAVE_SAWTOOTH),
    ("sine", icons::WAVE_SINE),
    ("keys", icons::PIANO_KEYS),
    ("guitar", icons::GUITAR),
    ("mic", icons::MICROPHONE),
    ("vocal", icons::MICROPHONE_STAGE),
    ("vinyl", icons::VINYL_RECORD),
    ("headphones", icons::HEADPHONES),
    ("speaker", icons::SPEAKER_HIFI),
    ("notes", icons::MUSIC_NOTES),
    ("bolt", icons::LIGHTNING),
    ("sparkle", icons::SPARKLE),
    ("fire", icons::FIRE),
    ("snow", icons::SNOWFLAKE),
    ("drop", icons::DROP),
    ("planet", icons::PLANET),
    ("star", icons::STAR),
    ("heart", icons::HEART),
    ("ghost", icons::GHOST),
    ("robot", icons::ROBOT),
    ("wind", icons::WIND),
    ("bell", icons::BELL),
    ("spiral", icons::SPIRAL),
    ("mountains", icons::MOUNTAINS),
    ("bus", icons::ARROWS_MERGE),
    ("filter", icons::FUNNEL),
    ("faders", icons::FADERS),
];

fn look(name: Option<&str>) -> Option<&'static str> {
    LOOKS.iter().find(|(n, _)| Some(*n) == name).map(|(_, icon)| *icon)
}

/// A track's color: its own, else the palette's for its place (the master: the accent).
fn track_color(t: &Theme, i: usize, color: Option<[u8; 3]>) -> Color32 {
    match color {
        Some(c) => Theme::pattern(0, Some(c)),
        None if i == 0 => t.accent,
        None => Theme::pattern(i - 1, None),
    }
}

/// An effect's own icon, else its kind's.
fn effect_icon(d: &Device) -> &'static str {
    look(d.icon.as_deref()).unwrap_or(match d.kind {
        DeviceKind::Eq => icons::FADERS_HORIZONTAL,
        DeviceKind::Filter => icons::FUNNEL,
        DeviceKind::Compressor | DeviceKind::Limiter => icons::ARROWS_IN_LINE_VERTICAL,
        DeviceKind::Distortion => icons::FIRE,
        DeviceKind::Chorus => icons::WAVE_SINE,
        DeviceKind::Delay => icons::REPEAT,
        DeviceKind::Reverb => icons::PLANET,
        DeviceKind::Plugin => icons::PLUG,
    })
}

/// Colors for a menu: the palette, a custom one, automatic. `Some(None)` = automatic.
fn color_choice(ui: &mut Ui, t: &Theme, current: Option<[u8; 3]>, fallback: [u8; 3]) -> Option<Option<[u8; 3]>> {
    let mut picked = None;
    for row in PATTERN_COLORS.chunks(6) {
        ui.horizontal(|ui| {
            for c in row {
                if kit::swatch(ui, t, Theme::pattern(0, Some(*c)), 20.0, tr!("Use this color")).clicked() {
                    picked = Some(Some(*c));
                    ui.close();
                }
            }
        });
    }
    ui.horizontal(|ui| {
        let mut rgb = current.unwrap_or(fallback);
        if ui.color_edit_button_srgb(&mut rgb).changed() {
            picked = Some(Some(rgb));
        }
        ui.label(tr!("Custom"));
    });
    if ui.button(tr!("Automatic")).clicked() {
        picked = Some(None);
        ui.close();
    }
    picked
}

/// Icons for a menu, in a grid; `Some(None)` = automatic.
fn icon_choice(ui: &mut Ui, t: &Theme, current: Option<&str>) -> Option<Option<&'static str>> {
    let mut picked = None;
    for row in LOOKS.chunks(7) {
        ui.horizontal(|ui| {
            for (name, icon) in row {
                if kit::icon_button(ui, t, icon, name, current == Some(*name)).clicked() {
                    picked = Some(Some(*name));
                    ui.close();
                }
            }
        });
    }
    if ui.button(tr!("Automatic")).clicked() {
        picked = Some(None);
        ui.close();
    }
    picked
}

/// A line across a strip or a card, edge to edge (its padding included).
fn divider(ui: &mut Ui, t: &Theme, width: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
    ui.painter().hline((r.left() - t.pad)..=(r.right() + t.pad), r.center().y, Stroke::new(1.0, t.line));
}

/// A modulation source's panel: edge to edge with the next, a line between them.
fn card(t: &Theme) -> Frame {
    Frame::new().fill(t.bg_device).inner_margin(t.pad)
}

/// The line after a panel shown with [`card`].
fn card_edge(ui: &Ui, t: &Theme, rect: Rect) {
    ui.painter().vline(rect.right() - 0.5, rect.y_range(), Stroke::new(1.0, t.line));
}

/// "Drums · Delay · Mix": what a mapping moves, for lists and hints.
pub fn target_name(p: &Project, target: Target) -> String {
    let track = |i: usize| p.mixer.get(i).map_or("?", |t| t.name.as_str());
    match target {
        Target::Synth { slot, param } => {
            let channel = p.channels.iter().find(|c| c.slot == slot).map_or("?", |c| c.name.as_str());
            format!("{channel} · {}", param.info().name)
        }
        Target::Device { id, param } => match p.device(id) {
            Some((t, _, d)) => format!("{} · {} · {}", track(t), d.kind.name(), d.kind.params().get(param).map_or("?", |i| i.name)),
            None => "?".into(),
        },
        Target::TrackGain { track: t } => format!("{} · Volume", track(t)),
        Target::TrackPan { track: t } => format!("{} · Pan", track(t)),
        Target::ChannelGain { slot } => format!("{} · Volume", p.channels.iter().find(|c| c.slot == slot).map_or("?", |c| c.name.as_str())),
        Target::Macro { index } => p.macros.get(index as usize).map_or("?".into(), |m| m.name.clone()),
    }
}

/// The knob behind a target, and its value now.
pub fn target_knob(p: &Project, target: Target) -> Option<(&'static ParamInfo, f32)> {
    match target {
        Target::Synth { slot, param } => Some((param.info(), p.channels.iter().find(|c| c.slot == slot)?.synth()?[param as usize])),
        Target::Device { id, param } => {
            let (_, _, d) = p.device(id)?;
            Some((d.kind.params().get(param)?, *d.params.get(param)?))
        }
        Target::TrackGain { track } => Some((&VOLUME, p.mixer.get(track)?.gain)),
        Target::TrackPan { track } => Some((&PAN, p.mixer.get(track)?.pan)),
        Target::ChannelGain { slot } => Some((&crate::rack::VOLUME, p.channels.iter().find(|c| c.slot == slot)?.gain)),
        Target::Macro { index } => Some((&MACRO, p.macros.get(index as usize)?.value)),
    }
}

/// A position along a target's knob (0..1) as the knob shows it: "-6.0 dB", "1.2 kHz"…
pub fn describe_target(p: &Project, target: Target, norm: f32) -> String {
    match (target, target_knob(p, target)) {
        (Target::TrackGain { .. }, _) => widgets::db(norm * VOLUME.max),
        (Target::TrackPan { .. }, _) => match norm * 2.0 - 1.0 {
            v if v.abs() < 0.01 => "C".into(),
            v => format!("{} {:.0}", if v < 0.0 { "L" } else { "R" }, v.abs() * 100.0),
        },
        (_, Some((info, _))) => info.format(info.from_norm(norm)),
        _ => format!("{:.0}%", norm * 100.0),
    }
}

/// What a mixer track is, as an icon: the master, a bus (other tracks feed it), or the
/// instrument of the first channel playing into it.
fn track_icon(p: &Project, i: usize) -> &'static str {
    if i == 0 {
        return icons::SPEAKER_HIFI;
    }
    if p.mixer.iter().enumerate().skip(1).any(|(k, t)| k != i && (t.route == i || t.sends.iter().any(|s| s.to == i))) {
        return icons::ARROWS_MERGE;
    }
    match p.channels.iter().find(|c| c.track == i).map(|c| &c.instrument) {
        Some(Instrument::Sampler { .. }) => icons::WAVEFORM,
        Some(Instrument::Synth { .. }) => icons::WAVE_SAWTOOTH,
        Some(Instrument::Multi { .. }) => icons::PIANO_KEYS,
        Some(Instrument::Plugin { .. }) => icons::PLUG,
        None => icons::FADERS,
    }
}

/// A cable hanging between two jacks, lower the further apart they are.
/// A send's cable: the same curve, dashed.
fn cable(from: Pos2, to: Pos2, deepest: f32, stroke: Stroke) -> Shape {
    Shape::CubicBezier(CubicBezierShape::from_points_stroke(cable_points(from, to, deepest), false, Default::default(), stroke))
}

fn dashed_cable(from: Pos2, to: Pos2, deepest: f32, stroke: Stroke) -> Vec<Shape> {
    let curve = CubicBezierShape::from_points_stroke(cable_points(from, to, deepest), false, Default::default(), stroke);
    Shape::dashed_line(&curve.flatten(Some(0.5)), stroke, 6.0, 4.0)
}

fn cable_points(from: Pos2, to: Pos2, deepest: f32) -> [Pos2; 4] {
    let sag = (14.0 + (to.x - from.x).abs() * 0.12).min(deepest);
    [from, from + vec2(0.0, sag), to + vec2(0.0, sag), to]
}

fn source_name(p: &Project, source: Source) -> String {
    match source {
        Source::Macro(i) => p.macros.get(i as usize).map_or("?".into(), |m| m.name.clone()),
        Source::Lfo(i) => format!("LFO {}", i + 1),
    }
}

impl App {
    // ---- mixer ----------------------------------------------------------------

    pub fn mixer(&mut self, ui: &mut Ui) {
        let t = self.theme.clone();
        let n = self.session.project.mixer.len();
        self.zone_hint(ui, tr!("Mixer: click a strip, its effects open on the right · pull cables between the jacks to route (Shift: send)"));
        kit::bar(ui, &t, |ui| {
            ui.label(RichText::new(format!("{}  Mixer", icons::FADERS)).strong());
            kit::right(ui, |ui| {
                // sounds' own tracks (their panels make them): out of the way unless asked for
                let ghosts = self.session.project.mixer.iter().filter(|m| m.hidden).count();
                if ghosts > 0 {
                    let r = kit::icon_button(ui, &t, icons::GHOST, &tr!("Show the {} ghost tracks", ghosts), self.settings.show_ghosts);
                    if r.clicked() {
                        self.settings.show_ghosts = !self.settings.show_ghosts;
                    }
                    self.hint(&r, tr!("Ghost tracks: the tracks sounds get of their own when their panel adds an EQ or effects. They play either way; shown, they come after the others, faded."));
                }
                let r = kit::icon_button(ui, &t, icons::SIDEBAR_SIMPLE, tr!("Effects panel"), self.mixer_fx);
                if r.clicked() {
                    self.mixer_fx = !self.mixer_fx;
                }
                let r = ui.add_enabled_ui(n < MAX_TRACKS, |ui| kit::button(ui, &t, Some(icons::PLUS), tr!("Track"), false)).inner;
                if r.clicked() {
                    self.run("mixer.add_track", json!({}));
                    self.mixer_track = n;
                }
                self.hint(&r, tr!("Add a mixer track. Route channels to it from their settings (F8)."));
            });
        });
        // the selected track's effects slide in on the right; strips take the rest, full height
        let track = self.mixer_track.min(n - 1);
        let full = ui.available_width();
        let was = self.mixer_fx;
        let mut open = was;
        egui::Panel::right("mixer-fx")
            .frame(Frame::new().fill(t.bg_base))
            .resizable(true)
            .default_size((full * 0.4).max(300.0))
            .size_range(260.0..=(full * 0.75).max(320.0))
            .show_collapsible(ui, &mut open, |ui| self.chain_column(ui, &t, track));
        if open != was {
            self.mixer_fx = open; // the chevron inside the panel sets mixer_fx itself: keep that
        }
        // strips fill the height, but never shorter than their controls need (then the area
        // scrolls down to them and the cables)
        let sends = self.session.project.mixer.iter().map(|t| t.sends.len()).max().unwrap_or(0);
        let (view_h, need) = (ui.available_height(), 320.0 + 28.0 * sends as f32);
        ScrollArea::both().id_salt("strips").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.vertical(|ui| {
                let height = (view_h - CABLES_H).max(need);
                let strips = ui
                    .horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let heard = self.session.project.tracks_heard();
                        // the tracks, the new-track strip, then the ghosts (sounds' own tracks)
                        // when they're shown, faded; a hidden one has no strip (Rect::NOTHING)
                        let ghost: Vec<bool> = self.session.project.mixer.iter().map(|m| m.hidden).collect();
                        let mut strips = vec![Rect::NOTHING; n];
                        for i in (0..n).filter(|i| !ghost[*i]) {
                            strips[i] = self.strip(ui, &t, i, height, heard.get(i).copied().unwrap_or(true));
                        }
                        if n < MAX_TRACKS {
                            self.new_track_strip(ui, &t, height);
                        }
                        if self.settings.show_ghosts {
                            for i in (0..n).filter(|i| ghost[*i]) {
                                strips[i] = ui.scope(|ui| {
                                    ui.multiply_opacity(0.7);
                                    self.strip(ui, &t, i, height, heard.get(i).copied().unwrap_or(true))
                                }).inner;
                            }
                        }
                        strips
                    })
                    .inner;
                self.patch_bay(ui, &t, &strips);
            });
        });
    }

    /// The last strip: a new track, one click away.
    fn new_track_strip(&mut self, ui: &mut Ui, t: &Theme, height: f32) {
        let (rect, r) = ui.allocate_exact_size(vec2(STRIP_W * 0.6 + 2.0 * t.pad, height), Sense::click());
        let hot = r.hovered();
        let color = if hot { t.fg_text } else { t.fg_muted };
        let p = ui.painter();
        if hot {
            p.rect_filled(rect, CornerRadius::ZERO, t.bg_panel);
        }
        p.text(rect.center() - vec2(0.0, 10.0), Align2::CENTER_CENTER, icons::PLUS_CIRCLE, FontId::proportional(22.0), color);
        p.text(rect.center() + vec2(0.0, 14.0), Align2::CENTER_CENTER, tr!("New track"), FontId::proportional(11.0), color);
        if r.clicked() {
            self.run("mixer.add_track", json!({}));
            self.mixer_track = self.session.project.mixer.len() - 1;
        }
        self.hint(&r, tr!("Add a mixer track (into the master). Pull a cable from its jack to route it elsewhere."));
    }

    /// The bottom of every strip: its jack, and the cables between them in the strips' own
    /// colors, its route (solid) and its sends (dashed, as strong as they are loud). Pull a
    /// cable from a jack onto another strip to route the track there; with Shift, a send.
    fn patch_bay(&mut self, ui: &mut Ui, t: &Theme, strips: &[Rect]) {
        let Some(first) = strips.first() else { return };
        // a hidden ghost has no strip: no slot, jack or cable
        let shown = |i: usize| strips.get(i).is_some_and(|r| r.is_positive());
        let right = strips.iter().filter(|r| r.is_positive()).map(|r| r.right()).fold(first.right(), f32::max);
        let (bay, _) = ui.allocate_exact_size(vec2(right - first.left(), CABLES_H), Sense::hover());
        let jack = |i: usize| pos2(strips[i].center().x, bay.top() + 14.0);
        let deepest = bay.height() - 18.0;
        let mixer = self.session.project.mixer.clone();
        let focus = self.mixer_track;
        let colors: Vec<Color32> = mixer.iter().enumerate().map(|(i, m)| track_color(t, i, m.color)).collect();
        // each strip goes on down to here: the jack sits in its own slot, under a divider
        for (i, strip) in strips.iter().enumerate().filter(|(i, _)| shown(*i)) {
            let slot = Rect::from_x_y_ranges(strip.x_range(), bay.y_range());
            ui.painter().rect_filled(slot, CornerRadius::ZERO, if i == focus { t.bg_device } else { t.bg_panel });
            ui.painter().vline(slot.right() - 0.5, slot.y_range(), Stroke::new(1.0, t.line));
        }
        ui.painter().hline(bay.x_range(), bay.top() + 0.5, Stroke::new(1.0, t.line));
        let painter = ui.painter_at(bay);
        // a track added this frame has no strip yet: its cables come next frame
        for (i, track) in mixer.iter().enumerate().take(strips.len()).skip(1).filter(|(i, _)| shown(*i)) {
            let lit = i == focus || track.route == focus;
            let color = colors[i].gamma_multiply(if lit { 1.0 } else { 0.55 });
            if shown(track.route) {
                let to = jack(track.route);
                painter.add(cable(jack(i), to, deepest, Stroke::new(if lit { 2.5 } else { 2.0 }, color)));
                painter.circle_filled(to, 3.5, color); // the plug, in the other jack
            }
            for send in &track.sends {
                if shown(send.to) {
                    let loud = 0.35 + 0.65 * send.amount.clamp(0.0, 1.0);
                    painter.extend(dashed_cable(jack(i), jack(send.to), deepest * 0.75, Stroke::new(1.5, colors[i].gamma_multiply(loud * if lit { 1.0 } else { 0.7 }))));
                }
            }
        }
        let pointer = ui.input(|i| i.pointer.interact_pos());
        for i in (0..strips.len()).filter(|i| shown(*i)) {
            let at = jack(i);
            let r = ui.interact(Rect::from_center_size(at, vec2(18.0, 18.0)), ui.scope_id().with(("jack", i)), if i == 0 { Sense::hover() } else { Sense::drag() });
            let hot = r.hovered() || self.mixer_cable == Some(i);
            let ring = colors.get(i).copied().unwrap_or(t.line);
            painter.circle_filled(at, 7.0, t.bg_field);
            painter.circle_stroke(at, 7.0, Stroke::new(1.5, if hot { t.accent } else { ring.gamma_multiply(0.8) }));
            painter.circle_filled(at, 2.5, if hot { t.accent } else { ring });
            if r.drag_started() {
                self.mixer_cable = Some(i);
            }
            if i == 0 {
                self.hint(&r, tr!("The master: everything ends up here, then the speakers."));
            } else {
                self.hint(&r, tr!("Pull a cable onto another strip: this track plays into it · with Shift: a send instead"));
                if r.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
            }
        }
        // a cable in hand: it follows the mouse; let go over a strip to plug it in
        if let (Some(from), Some(p)) = (self.mixer_cable, pointer) {
            let over = strips.iter().position(|r| r.x_range().contains(p.x)).filter(|k| *k != from);
            let to = over.map_or(p, jack);
            painter.add(cable(jack(from), to, deepest, Stroke::new(2.0, t.accent)));
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some(k) = over {
                ui.painter().rect_stroke(strips[k], CornerRadius::same(t.radius_md), Stroke::new(2.0, t.accent), StrokeKind::Inside);
            }
            if ui.input(|i| i.pointer.primary_released()) {
                self.mixer_cable = None;
                if let Some(to) = over {
                    if ui.input(|i| i.modifiers.shift) {
                        self.run("mixer.set_send", json!({ "track": from, "to": to, "amount": 0.25 }));
                    } else {
                        self.run("mixer.set", json!({ "track": from, "route": to }));
                    }
                }
            }
        } else if self.mixer_cable.is_some() {
            self.mixer_cable = None;
        }
    }

    /// The strip's EQ: low, mid and high, the EQ Three's bands, as three small knobs.
    fn strip_eq(&mut self, ui: &mut Ui, t: &Theme, i: usize, eq: [f32; 3]) {
        let params = DeviceKind::Eq.params();
        let (row, _) = ui.allocate_exact_size(vec2(STRIP_W, 46.0), Sense::hover());
        for (band, (info, key)) in [(&params[0], "low"), (&params[2], "mid"), (&params[3], "high")].into_iter().enumerate() {
            let x = row.left() + STRIP_W * (band as f32 + 0.5) / 3.0;
            ui.painter().text(pos2(x, row.top() + 6.0), Align2::CENTER_CENTER, crate::i18n::t(info.name).to_uppercase(), FontId::proportional(9.0), t.fg_muted);
            let (k, v) = widgets::mini_knob(ui, t, pos2(x, row.top() + 22.0), info.to_norm(eq[band]), info.to_norm(info.default), ui.scope_id().with(("eq", i, band)));
            let db = v.map_or(eq[band], |v| info.from_norm(v));
            let shown = if db.abs() < 0.05 { "0".to_string() } else { format!("{db:+.1}") };
            ui.painter().text(pos2(x, row.top() + 39.0), Align2::CENTER_CENTER, shown, Theme::mono(9.0), if db.abs() < 0.05 { t.fg_muted } else { t.fg_text });
            self.apply_knob(&k, v.map(|v| info.from_norm(v)), "mixer.set", |v| json!({ "track": i, key: v }), info);
        }
    }

    /// Track `i`'s strip; `heard`: whether it sounds with the solos as they are.
    fn strip(&mut self, ui: &mut Ui, t: &Theme, i: usize, height: f32, heard: bool) -> Rect {
        // an earlier strip's menu may have deleted a track this frame
        let Some(track) = self.session.project.mixer.get(i).cloned() else { return Rect::NOTHING };
        let icon = look(track.icon.as_deref()).unwrap_or_else(|| track_icon(&self.session.project, i));
        let feeds: Vec<String> = self.session.project.channels.iter().filter(|c| c.track == i).map(|c| c.name.clone()).collect();
        let names: Vec<String> = self.session.project.mixer.iter().map(|t| t.name.clone()).collect();
        let selected = self.mixer_track == i;
        // every track has a color: a bar along its top and its icon
        let color = track_color(t, i, track.color);
        let pad = t.pad as i8;
        let frame = Frame::new().fill(if selected { t.bg_device } else { t.bg_panel }).inner_margin(egui::Margin { left: pad, right: pad, top: pad, bottom: 0 });
        // the strip's own click sits behind its controls (registered first), so the route
        // and send menus, mute and the rest get their clicks
        let shown = ui.scope_builder(UiBuilder::new().sense(Sense::click()), |ui| frame.show(ui, |ui| {
            ui.set_width(STRIP_W);
            ui.set_height(height - t.pad);
            ui.vertical_centered(|ui| {
                let mut title = egui::text::LayoutJob::default();
                title.append(&format!("{icon}  "), 0.0, egui::TextFormat::simple(FontId::proportional(13.0), color));
                title.append(&track.name, 0.0, egui::TextFormat::simple(FontId::proportional(13.0), if i == 0 { t.accent } else { t.fg_text }));
                let name = ui.add(egui::Label::new(title).truncate().sense(Sense::click()));
                if name.clicked() {
                    (self.mixer_track, self.mixer_fx) = (i, true);
                }
                self.track_menu(&name, i, &track.name);
                let what = if feeds.is_empty() { String::new() } else { tr!(" Plays: {}.", feeds.join(", ")) };
                self.hint(&name, tr!("Click to open this track's effects on the right · right-click to rename or delete.{}", what));
                let plays = if i == 0 { tr!("out").to_string() } else if feeds.is_empty() { String::new() } else { feeds.join(" · ") };
                ui.add(egui::Label::new(RichText::new(plays).small().color(t.fg_muted)).truncate());
                divider(ui, t, STRIP_W);
                let (r, v) = widgets::knob(ui, t, &PAN, track.pan);
                self.apply_knob(&r, v, "mixer.set", |v| json!({ "track": i, "pan": v }), &PAN);
                self.mod_menu(&r, Target::TrackPan { track: i });
                divider(ui, t, STRIP_W);
                self.strip_eq(ui, t, i, track.eq);
                divider(ui, t, STRIP_W);
                // built from the bottom up; the fader takes whatever height is left. Each block
                // between two dividers has the same air above and below it.
                ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let air = t.pad;
                    if i > 0 {
                        ui.add_space(air);
                        let free: Vec<(usize, &String)> =
                            names.iter().enumerate().filter(|(to, _)| *to != i && track.sends.iter().all(|s| s.to != *to)).collect();
                        if !free.is_empty() {
                            let r = ComboBox::from_id_salt(("send", i)).selected_text(tr!("+ Send")).width(STRIP_W).show_ui(ui, |ui| {
                                for (to, n) in free {
                                    if ui.selectable_label(false, n).clicked() {
                                        self.run("mixer.set_send", json!({ "track": i, "to": to, "amount": 0.25 }));
                                    }
                                }
                            });
                            self.hint(&r.response, tr!("Send a copy of this track to another one, e.g. a shared reverb."));
                            ui.add_space(t.gap + 2.0);
                        }
                        for send in track.sends.iter().rev() {
                            let to = names.get(send.to).map_or("?", String::as_str);
                            // its name, how much, and a knob for it
                            let (row, r) = ui.allocate_exact_size(vec2(STRIP_W, 22.0), Sense::click());
                            let knob_at = pos2(row.right() - 10.0, row.center().y);
                            let p = ui.painter();
                            p.text(knob_at - vec2(14.0, 0.0), Align2::RIGHT_CENTER, format!("{:.0}%", send.amount * 100.0), Theme::mono(10.0), t.fg_text);
                            let label = format!("{} {to}", icons::ARROW_BEND_DOWN_RIGHT);
                            let cut = kit::fit_text(p, row.left_center(), Align2::LEFT_CENTER, label, FontId::proportional(11.0), t.select, row.width() - 58.0).1;
                            let (k, v) = widgets::mini_knob(ui, t, knob_at, send.amount, SEND.default, ui.scope_id().with(("send", i, send.to)));
                            // all the way down keeps a trace of it: removing is the menu's job
                            self.apply_knob(&k, v.map(|v| v.max(0.01)), "mixer.set_send", |v| json!({ "track": i, "to": send.to, "amount": v }), &SEND);
                            if cut {
                                r.clone().on_hover_text(to);
                            }
                            self.hint(&r, tr!("Send to {}, after the fader · right-click to remove it", to));
                            let send_to = send.to;
                            for resp in [&r, &k] {
                                resp.context_menu(|ui| {
                                    if ui.button(tr!("{}  Remove the send", icons::TRASH)).clicked() {
                                        self.run("mixer.set_send", json!({ "track": i, "to": send_to, "amount": 0.0 }));
                                        ui.close();
                                    }
                                });
                            }
                            ui.add_space(t.gap + 2.0);
                        }
                        let route = names.get(track.route).map_or("?", String::as_str);
                        let r = ComboBox::from_id_salt(("route", i)).selected_text(format!("→ {route}")).width(STRIP_W).show_ui(ui, |ui| {
                            for (to, n) in names.iter().enumerate().filter(|(to, _)| *to != i) {
                                if ui.selectable_label(to == track.route, n).clicked() {
                                    self.run("mixer.set", json!({ "track": i, "route": to }));
                                }
                            }
                        });
                        self.hint(&r.response, tr!("Where this track goes: the master, or another track (a bus)."));
                        ui.add_space(air);
                        divider(ui, t, STRIP_W);
                    }
                    ui.add_space(air);
                    ui.allocate_ui_with_layout(vec2(STRIP_W, 24.0), Layout::left_to_right(Align::Center), |ui| {
                        let r = kit::icon_button(ui, t, if track.mute { icons::SPEAKER_SLASH } else { icons::SPEAKER_HIGH }, "Mute", track.mute);
                        if r.clicked() {
                            self.run("mixer.set", json!({ "track": i, "mute": !track.mute }));
                        }
                        self.hint(&r, tr!("Silence this track (and whatever it feeds only through it)."));
                        if i > 0 {
                            let at = pos2(ui.cursor().left() + 9.0, ui.max_rect().center().y);
                            let r = kit::chip(ui, ui.painter(), t, at, "S", track.solo, t.select, ui.scope_id().with(("solo", i)));
                            ui.add_space(18.0);
                            if r.clicked() {
                                self.run("mixer.set", json!({ "track": i, "solo": !track.solo }));
                            }
                            self.hint(&r, tr!("Solo: only the soloed tracks play, with the buses they feed and the tracks feeding them"));
                        }
                        let quiet = track.mute || !heard;
                        kit::right(ui, |ui| ui.label(RichText::new(widgets::db(track.gain)).font(Theme::mono(11.0)).color(if quiet { t.fg_muted } else { t.fg_text })));
                    });
                    ui.add_space(air);
                    divider(ui, t, STRIP_W);
                    ui.add_space(air);
                    let fader_h = (ui.available_height() - t.gap).max(36.0); // shrinks rather than climbing over the pan knob
                    let (r, v) = widgets::fader(ui, t, track.gain, self.meters[i], self.holds[i], fader_h);
                    self.apply_knob(&r, v, "mixer.set", |v| json!({ "track": i, "gain": v }), &VOLUME);
                    self.mod_menu(&r, Target::TrackGain { track: i });
                });
            });
        }));
        let rect = shown.response.rect;
        let top = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 3.0));
        ui.painter().rect_filled(top, CornerRadius::ZERO, color.gamma_multiply(if selected { 1.0 } else { 0.7 }));
        ui.painter().vline(rect.right() - 0.5, rect.y_range(), Stroke::new(1.0, t.line));
        if !selected {
            kit::spotlight(ui, t, rect, 0);
        }
        if shown.response.clicked() {
            (self.mixer_track, self.mixer_fx) = (i, true);
        }
        // right-click anywhere on it: its menu
        self.track_menu(&shown.response, i, &track.name);
        rect
    }

    fn track_menu(&mut self, resp: &Response, i: usize, name: &str) {
        if resp.double_clicked() {
            self.rename = Some(Renaming::new(Rename::Track(i), name));
        }
        let mut delete = false;
        resp.context_menu(|ui| {
            if ui.button(tr!("{}  Rename…", icons::PENCIL_SIMPLE)).clicked() {
                self.rename = Some(Renaming::new(Rename::Track(i), name));
                ui.close();
            }
            let (t, own) = (self.theme.clone(), self.session.project.mixer.get(i).map(|tr| (tr.color, tr.icon.clone())));
            let (color, icon) = own.unwrap_or_default();
            ui.menu_button(tr!("{}  Color", icons::PALETTE), |ui| {
                let fallback = track_color(&t, i, None).to_array();
                match color_choice(ui, &t, color, [fallback[0], fallback[1], fallback[2]]) {
                    Some(Some(c)) => self.run_merge("mixer.set", json!({ "track": i, "color": c }), "track-color"),
                    Some(None) => self.run("mixer.set", json!({ "track": i, "auto_color": true })),
                    None => {}
                }
            });
            ui.menu_button(tr!("{}  Icon", icons::STAR), |ui| {
                if let Some(name) = icon_choice(ui, &t, icon.as_deref()) {
                    self.run("mixer.set", json!({ "track": i, "icon": name.unwrap_or("") }));
                }
            });
            ui.separator();
            ui.menu_button(tr!("{}  Duck to", icons::ARROWS_IN_LINE_VERTICAL), |ui| self.duck_menu(ui, i));
            if i > 0 && ui.button(tr!("{}  Delete track", icons::TRASH)).clicked() {
                delete = true;
                ui.close();
            }
        });
        if delete {
            self.run("mixer.remove_track", json!({ "track": i }));
            self.mixer_track = self.mixer_track.min(self.session.project.mixer.len() - 1);
        }
    }

    // ---- effect chains ------------------------------------------------------

    /// The mixer's right panel: one track's effects, top to bottom in signal order.
    fn chain_column(&mut self, ui: &mut Ui, t: &Theme, track: usize) {
        let Some(tr) = self.session.project.mixer.get(track).cloned() else { return };
        kit::region(ui);
        kit::bar(ui, t, |ui| {
            label(ui, t, tr!("EFFECTS"));
            ui.add(egui::Label::new(RichText::new(&tr.name).strong().color(t.accent)).truncate());
            kit::right(ui, |ui| {
                if kit::icon_button(ui, t, icons::X, tr!("Close the effects panel"), false).clicked() {
                    self.mixer_fx = false;
                }
                let r = kit::button(ui, t, Some(icons::PLUS), tr!("Effect"), false);
                kit::menu(t, &r).show(|ui| self.effect_menu(ui, track));
                self.hint(&r, tr!("Add an effect to the end of this track's chain."));
                let r = kit::button(ui, t, Some(icons::ARROWS_IN_LINE_VERTICAL), tr!("Duck to…"), false);
                kit::menu(t, &r).show(|ui| self.duck_menu(ui, track));
                self.hint(&r, tr!("Sidechain in one click: this track dips every time the track you pick plays (bass under the kick)."));
            });
        });
        kit::scroll(ui, t, ("fx-column", track), false, t.bg_base, |ui| {
            if tr.devices.is_empty() {
                kit::padded(ui, t, |ui| ui.label(RichText::new(tr!("No effects yet. They run top to bottom, before the fader: + Effect adds one.")).small().color(t.fg_muted)));
            }
            for (pos, d) in tr.devices.iter().enumerate() {
                self.device_card(ui, t, d, pos, tr.devices.len(), track, true);
            }
        });
    }

    /// "+ Effect": the built-ins, then scanned plugin effects.
    fn effect_menu(&mut self, ui: &mut Ui, track: usize) {
        for kind in DeviceKind::ALL {
            if ui.button(kind.name()).clicked() {
                self.run("device.add", json!({ "track": track, "kind": kind }));
                ui.close();
            }
        }
        let plugins = self.effect_plugins();
        ui.separator();
        ui.menu_button(tr!("{}  Plugins ({})", icons::PLUG, plugins.len()), |ui| {
            if plugins.is_empty() {
                ui.label(RichText::new(tr!("None scanned yet: open the browser's Plugins tab.")).small());
            }
            ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for f in &plugins {
                    if ui.button(format!("{}  ·  {}", f.name, f.vendor)).clicked() {
                        let plugin = crate::plugins::plugin_ref(f);
                        self.run("device.add", json!({ "track": track, "kind": "plugin", "plugin": plugin }));
                        ui.close();
                    }
                }
            });
        });
    }

    /// A track's effects, top to bottom in signal order, with "+ Effect" and "Duck to…".
    pub fn chain(&mut self, ui: &mut Ui, t: &Theme, track: usize) {
        let Some(tr) = self.session.project.mixer.get(track).cloned() else { return };
        kit::bar_with(ui, t, t.bg_base, |ui| {
            label(ui, t, tr!("EFFECTS"));
            ui.add(egui::Label::new(RichText::new(&tr.name).color(t.accent)).truncate());
            kit::right(ui, |ui| {
                let r = kit::button(ui, t, Some(icons::PLUS), tr!("Effect"), false);
                kit::menu(t, &r).show(|ui| self.effect_menu(ui, track));
                self.hint(&r, tr!("Add an effect to the end of this track's chain."));
                let r = kit::button(ui, t, Some(icons::ARROWS_IN_LINE_VERTICAL), tr!("Duck to…"), false);
                kit::menu(t, &r).show(|ui| self.duck_menu(ui, track));
                self.hint(&r, tr!("Sidechain in one click: this track dips every time the track you pick plays (bass under the kick)."));
            });
        });
        if tr.devices.is_empty() {
            kit::padded(ui, t, |ui| ui.label(RichText::new(tr!("No effects yet. They run top to bottom, before the fader: + Effect adds one.")).small().color(t.fg_muted)));
        }
        for (pos, d) in tr.devices.iter().enumerate() {
            self.device_card(ui, t, d, pos, tr.devices.len(), track, true);
        }
    }

    /// Track list for "Duck to…": pick the trigger, get a compressor keyed by it.
    pub fn duck_menu(&mut self, ui: &mut Ui, track: usize) {
        let names: Vec<String> = self.session.project.mixer.iter().map(|t| t.name.clone()).collect();
        ui.label(RichText::new(tr!("{} ducks under…", names[track])).small());
        for (source, name) in names.iter().enumerate().filter(|(s, _)| *s != track && *s != 0) {
            if ui.button(format!("{}  {name}", icons::FADERS)).clicked() {
                self.run("mixer.duck", json!({ "track": track, "source": source }));
                self.say_as(Tone::Done, tr!("{} now ducks under {}: tweak the depth on its compressor", names[track], name));
                ui.close();
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn device_card(&mut self, ui: &mut Ui, t: &Theme, d: &Device, pos: usize, len: usize, track: usize, wide: bool) {
        let folded = self.folded.contains(&d.id);
        let shown = Frame::new().fill(t.bg_device).inner_margin(t.pad).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(t.gap, t.gap);
            if wide {
                ui.set_width(ui.available_width()); // a column of full-width cards
            }
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    let icon = if folded { icons::CARET_RIGHT } else { icons::CARET_DOWN };
                    if kit::icon_button(ui, t, icon, if folded { tr!("Unfold") } else { tr!("Fold to its title") }, false).clicked() && !self.folded.remove(&d.id) {
                        self.folded.insert(d.id);
                    }
                    let r = widgets::power(ui, t, !d.bypass);
                    if r.clicked() {
                        self.run("device.bypass", json!({ "id": d.id }));
                    }
                    self.hint(&r, tr!("On / off. Off passes the sound through untouched."));
                    let color = if d.bypass { t.fg_muted } else { t.fg_text };
                    let kind = d.plugin.as_ref().map_or(d.kind.name(), |p| p.name.as_str());
                    let name = if d.name.is_empty() { kind } else { d.name.as_str() };
                    let tint = d.color.map_or(t.fg_muted, |c| Theme::pattern(0, Some(c)));
                    let mut title = egui::text::LayoutJob::default();
                    title.append(&format!("{}  ", effect_icon(d)), 0.0, egui::TextFormat::simple(FontId::proportional(13.0), if d.bypass { t.fg_muted } else { tint }));
                    title.append(name, 0.0, egui::TextFormat::simple(FontId::proportional(13.0), color));
                    let r = ui.add(egui::Label::new(title).truncate().sense(Sense::click()));
                    self.effect_menu_on(&r, d);
                    if !d.name.is_empty() {
                        r.on_hover_text(kind);
                    }
                    // fixed slots after the name (a right-aligned group would stretch the card)
                    let (earlier, later) = if wide { (icons::CARET_UP, icons::CARET_DOWN) } else { (icons::CARET_LEFT, icons::CARET_RIGHT) };
                    for (icon, to, enabled, help) in [
                        (earlier, pos.wrapping_sub(1), pos > 0, tr!("Move earlier in the chain")),
                        (later, pos + 1, pos + 1 < len, tr!("Move later in the chain")),
                    ] {
                        if ui.add_enabled_ui(enabled, |ui| kit::icon_button(ui, t, icon, help, false)).inner.clicked() {
                            self.run("device.move", json!({ "id": d.id, "index": to }));
                        }
                    }
                    if kit::icon_button(ui, t, icons::X, tr!("Remove this effect (Ctrl Z brings it back)"), false).clicked() {
                        self.run("device.remove", json!({ "id": d.id }));
                    }
                });
                if folded {
                    return;
                }
                if let Some(plugin) = &d.plugin {
                    self.plugin_card(ui, t, ravune_engine::PluginTarget::Device(d.id), plugin);
                    return;
                }
                ui.horizontal_wrapped(|ui| {
                    for (p, info) in d.kind.params().iter().enumerate() {
                        let (r, v) = widgets::param(ui, t, info, d.params.get(p).copied().unwrap_or(info.default));
                        self.apply_knob(&r, v, "device.set", |v| json!({ "id": d.id, "param": p, "value": v }), info);
                        self.mod_menu(&r, Target::Device { id: d.id, param: p });
                    }
                });
                if d.kind == DeviceKind::Compressor {
                    let names: Vec<String> = self.session.project.mixer.iter().map(|t| t.name.clone()).collect();
                    ui.horizontal(|ui| {
                        label(ui, t, "SIDECHAIN");
                        let current = d.sidechain.and_then(|k| names.get(k)).map_or(tr!("Off (itself)"), String::as_str);
                        let r = ComboBox::from_id_salt(("sidechain", d.id)).selected_text(current).width(140.0).show_ui(ui, |ui| {
                            if ui.selectable_label(d.sidechain.is_none(), tr!("Off (itself)")).clicked() {
                                self.run("device.sidechain", json!({ "id": d.id }));
                            }
                            for (k, name) in names.iter().enumerate().filter(|(k, _)| *k != track && *k != 0) {
                                if ui.selectable_label(d.sidechain == Some(k), name).clicked() {
                                    self.run("device.sidechain", json!({ "id": d.id, "source": k }));
                                }
                            }
                        });
                        self.hint(&r.response, tr!("Which track this compressor listens to. Another track = sidechain (ducking)."));
                    });
                }
            });
        });
        let rect = shown.response.rect;
        // its color along the top, a line to the next card
        if let Some(c) = d.color {
            ui.painter().rect_filled(Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 3.0)), CornerRadius::ZERO, Theme::pattern(0, Some(c)));
        }
        if wide {
            ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, t.line));
        } else {
            ui.painter().vline(rect.right() - 0.5, rect.y_range(), Stroke::new(1.0, t.line));
        }
        kit::spotlight(ui, t, rect, 0);
    }

    /// An effect's menu (right-click its name): rename, color, icon, on / off, remove.
    fn effect_menu_on(&mut self, resp: &Response, d: &Device) {
        let label = if d.name.is_empty() { d.plugin.as_ref().map_or(d.kind.name(), |p| p.name.as_str()).to_string() } else { d.name.clone() };
        if resp.double_clicked() {
            self.rename = Some(Renaming::new(Rename::Device(d.id), label.clone()));
        }
        let t = self.theme.clone();
        resp.context_menu(|ui| {
            if ui.button(tr!("{}  Rename…", icons::PENCIL_SIMPLE)).clicked() {
                self.rename = Some(Renaming::new(Rename::Device(d.id), label.clone()));
                ui.close();
            }
            ui.menu_button(tr!("{}  Color", icons::PALETTE), |ui| match color_choice(ui, &t, d.color, PATTERN_COLORS[d.id as usize % PATTERN_COLORS.len()]) {
                Some(Some(c)) => self.run_merge("device.style", json!({ "id": d.id, "color": c }), "effect-color"),
                Some(None) => self.run("device.style", json!({ "id": d.id, "auto_color": true })),
                None => {}
            });
            ui.menu_button(tr!("{}  Icon", icons::STAR), |ui| {
                if let Some(name) = icon_choice(ui, &t, d.icon.as_deref()) {
                    self.run("device.style", json!({ "id": d.id, "icon": name.unwrap_or("") }));
                }
            });
            ui.separator();
            if ui.button(if d.bypass { tr!("{}  Turn on", icons::POWER) } else { tr!("{}  Turn off", icons::POWER) }).clicked() {
                self.run("device.bypass", json!({ "id": d.id }));
                ui.close();
            }
            if ui.button(tr!("{}  Remove", icons::TRASH)).clicked() {
                self.run("device.remove", json!({ "id": d.id }));
                ui.close();
            }
        });
    }

    // ---- modulation ---------------------------------------------------------

    pub fn modulation(&mut self, ui: &mut Ui) {
        let t = self.theme.clone();
        self.zone_hint(ui, tr!("Modulation: right-click any knob or fader to map a macro or LFO onto it"));
        kit::bar(ui, &t, |ui| {
            ui.label(RichText::new(tr!("{}  Modulation", icons::WAVE_SINE)).strong());
            kit::right(ui, |ui| {
                let free = self.session.project.lfos.len() < MAX_LFOS;
                let r = ui.add_enabled_ui(free, |ui| kit::button(ui, &t, Some(icons::PLUS), "LFO", false)).inner;
                if r.clicked() {
                    self.run("lfo.add", json!({}));
                }
                self.hint(&r, tr!("Add a low-frequency oscillator: a slow wave that moves the knobs you map it to."));
            });
        });
        kit::scroll(ui, &t, "modulation", false, t.bg_base, |ui| self.modulation_body(ui, &t));
    }

    fn modulation_body(&mut self, ui: &mut Ui, t: &Theme) {
        let t = t.clone();
        let sources = ScrollArea::horizontal().id_salt("mod-sources").show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let shown = card(&t).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(t.gap, t.gap);
                    ui.vertical(|ui| {
                        label(ui, &t, tr!("MACROS"));
                        for row in [0..4, 4..8] {
                            ui.horizontal(|ui| row.for_each(|i| self.macro_knob(ui, &t, i)));
                        }
                        ui.horizontal(|ui| self.variations(ui, &t));
                    });
                });
                card_edge(ui, &t, shown.response.rect);
                kit::spotlight(ui, &t, shown.response.rect, 0);
                for i in 0..self.session.project.lfos.len() {
                    self.lfo_card(ui, &t, i);
                }
            });
        });
        ui.painter().hline(sources.inner_rect.x_range(), sources.inner_rect.bottom() + 0.5, Stroke::new(1.0, t.line));
        kit::padded(ui, &t, |ui| self.mappings(ui, &t));
    }

    /// What each macro and LFO moves, and by how much.
    fn mappings(&mut self, ui: &mut Ui, t: &Theme) {
        let t = t.clone();
        label(ui, &t, tr!("MAPPINGS"));
        let p = self.session.project.clone();
        if p.mappings.is_empty() {
            ui.label(RichText::new(tr!("Right-click any knob or fader and pick a macro or LFO to modulate it.")).color(t.fg_muted));
        }
        for (k, m) in p.mappings.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(source_name(&p, m.source)).color(t.select));
                ui.label(RichText::new("→").color(t.fg_muted));
                ui.label(target_name(&p, m.target));
                let mut pct = m.amount * 100.0;
                let r = ui.add(egui::DragValue::new(&mut pct).range(-100.0..=100.0).speed(0.5).max_decimals(0).suffix("%"));
                if r.changed() {
                    self.run_merge("mod.set", json!({ "index": k, "amount": pct / 100.0 }), "mod-amount");
                }
                if r.drag_stopped() || r.lost_focus() {
                    self.session.close_merge();
                }
                self.hint(&r, tr!("How far the source moves the knob, as a share of its whole range. Negative = inverted."));
                if kit::icon_button(ui, &t, icons::X, tr!("Remove this mapping"), false).clicked() {
                    self.run("mod.remove", json!({ "index": k }));
                }
            });
        }
    }

    /// Rand, then the stored variations (click recalls, right-click forgets) and + to store one.
    fn variations(&mut self, ui: &mut Ui, t: &Theme) {
        let r = kit::button(ui, t, Some(icons::DICE_FIVE), "Rand", false);
        if r.clicked() {
            self.run("macro.randomize", json!({}));
        }
        self.hint(&r, tr!("Every macro that moves something to a random position: click again for another"));
        let count = self.session.project.variations.len();
        for k in 0..count {
            let now = self.session.project.variations[k].iter().zip(&self.session.project.macros).all(|(v, m)| (v - m.value).abs() < 1e-4);
            let r = kit::button(ui, t, None, &(k + 1).to_string(), now);
            if r.clicked() {
                self.run("macro.recall", json!({ "index": k }));
            }
            r.context_menu(|ui| {
                if ui.button(format!("{}  {}", icons::TRASH, tr!("Forget this variation"))).clicked() {
                    self.run("macro.forget", json!({ "index": k }));
                    ui.close();
                }
            });
            self.hint(&r, tr!("Variation {}: click to put the macros back there · right-click to forget it", k + 1));
        }
        if count < ravune_core::VARIATIONS {
            let r = kit::icon_button(ui, t, icons::PLUS, tr!("Store where the macros are now as a variation"), false);
            if r.clicked() {
                self.run("macro.store", json!({}));
            }
        }
    }

    fn macro_knob(&mut self, ui: &mut Ui, t: &Theme, i: usize) {
        let m = self.session.project.macros[i].clone();
        let (r, v) = widgets::knob(ui, t, &MACRO, m.value);
        ui.painter().text(pos2(r.rect.center().x, r.rect.top()), Align2::CENTER_TOP, &m.name, FontId::proportional(11.0), t.fg_muted);
        // Alt+drag: a cable onto the knob it should move, instead of turning this one
        let alt = ui.input(|i| i.modifiers.alt);
        self.apply_knob(&r, if alt { None } else { v }, "macro.set", |v| json!({ "index": i, "value": v }), &MACRO);
        self.mod_source(&r, Source::Macro(i as u8), alt);
        self.mod_menu(&r, Target::Macro { index: i as u8 });
    }

    /// A modulation source being dragged onto a knob (macros with Alt, LFOs by their handle):
    /// a cable from it to the pointer while it goes.
    fn mod_source(&mut self, r: &Response, source: Source, start: bool) {
        if start && r.drag_started() {
            egui::DragAndDrop::set_payload(&r.ctx, source);
        }
        if r.dragged() && egui::DragAndDrop::payload::<Source>(&r.ctx).is_some_and(|s| *s == source)
            && let Some(to) = r.ctx.pointer_hover_pos()
        {
            let painter = r.ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("mod-cable")));
            painter.add(cable(r.rect.center(), to, 30.0, Stroke::new(2.0, self.theme.select)));
            painter.circle_filled(to, 4.0, self.theme.select);
        }
    }

    fn lfo_card(&mut self, ui: &mut Ui, t: &Theme, i: usize) {
        // an earlier card's X may have deleted an LFO this frame
        let Some(&lfo) = self.session.project.lfos.get(i) else { return };
        let shown = card(t).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(t.gap, t.gap);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    let handle = ui.add(egui::Label::new(RichText::new(icons::DOTS_SIX_VERTICAL).color(t.fg_muted)).sense(Sense::drag()));
                    self.mod_source(&handle, Source::Lfo(i as u8), true);
                    self.hint(&handle.on_hover_cursor(egui::CursorIcon::Grab), tr!("Drag onto any knob or fader: this LFO moves it"));
                    label(ui, t, &format!("LFO {}", i + 1));
                    let r = kit::button(ui, t, Some(icons::TIMER), "Sync", lfo.sync);
                    if r.clicked() {
                        self.run("lfo.set", json!({ "index": i, "sync": !lfo.sync }));
                    }
                    self.hint(&r, tr!("Sync: lock the LFO to the tempo and the bar. Off: free-running rate in Hz."));
                    if kit::icon_button(ui, t, icons::X, tr!("Delete this LFO and its mappings"), false).clicked() {
                        self.run("lfo.remove", json!({ "index": i }));
                    }
                });
                ui.horizontal(|ui| {
                    let (r, v) = widgets::wave_picker(ui, t, "Shape", lfo.shape as f32, 5);
                    if let Some(v) = v {
                        self.run("lfo.set", json!({ "index": i, "shape": v as u8 }));
                    }
                    self.hint(&r, tr!("LFO shape: sine, triangle, saw, square, or random: a new level every cycle (sample and hold)."));
                    if lfo.sync {
                        let (r, v) = widgets::knob(ui, t, &LENGTH, lfo.beats);
                        // musical lengths only: 1/4, 1/2, 1, 2 … 16 beats
                        let v = v.map(|v| v.log2().round().exp2());
                        self.apply_knob(&r, v, "lfo.set", |v| json!({ "index": i, "beats": v }), &LENGTH);
                    } else {
                        let (r, v) = widgets::knob(ui, t, &RATE, lfo.hz);
                        self.apply_knob(&r, v, "lfo.set", |v| json!({ "index": i, "hz": v }), &RATE);
                    }
                });
            });
        });
        card_edge(ui, t, shown.response.rect);
        kit::spotlight(ui, t, shown.response.rect, 0);
    }

    /// Automation recording: while it's armed and the song plays, a knob being turned writes
    /// its moves into the automation clip under the playhead (a new one under everything if
    /// there's none), overwriting what was there as it goes. One drag is one undo step.
    fn record_automation(&mut self, resp: &Response, target: Target) {
        if resp.drag_stopped() && self.auto_last.is_some_and(|(t, ..)| t == target) {
            self.auto_last = None;
            self.session.end_group();
        }
        if let (Some(beat), true, true) = (self.beat, self.auto_rec && self.session.song_mode, resp.dragged()) {
            if self.auto_last.is_none() {
                self.session.begin_group("Automation recording");
                self.auto_last = Some((target, usize::MAX, 0));
            }
            self.record_point(target, (beat * PPQ as f64) as u32);
        }
    }

    /// Writes where `target`'s knob is now at song tick `tick`.
    fn record_point(&mut self, target: Target, tick: u32) {
        let Some((info, value)) = target_knob(&self.session.project, target) else { return };
        let v = info.to_norm(value);
        let p = &self.session.project;
        let under = |c: &Clip| matches!(&c.kind, ClipKind::Automation { target: t, .. } if t == &target) && (c.start..c.start + c.length).contains(&tick);
        let index = match p.clips.iter().position(under) {
            Some(i) => i,
            None => {
                // the knob's first clip runs from the top, like Create automation clip; past
                // its clips, a new one starts at the bar
                let first = !p.clips.iter().any(|c| matches!(&c.kind, ClipKind::Automation { target: t, .. } if t == &target));
                let start = if first { 0 } else { tick - tick % BAR };
                let (lane, length) = (p.clips.iter().map(|c| c.lane + 1).max().unwrap_or(0), p.song_end().saturating_sub(start).max(8 * BAR));
                self.run("clip.add", json!({ "lane": lane, "start": start, "length": length, "automation": target, "points": [[0, v], [length, v]] }));
                self.session.project.clips.len() - 1
            }
        };
        let clip = &self.session.project.clips[index];
        let ClipKind::Automation { points, .. } = &clip.kind else { return };
        let at = tick - clip.start + clip.offset;
        // overwrite from the last point this drag wrote (or from here) up to now
        let last = self.auto_last.filter(|&(t, i, _)| t == target && i == index).map(|(.., last)| last);
        if last == Some(at) {
            return;
        }
        let from = last.unwrap_or(at);
        let mut points: Vec<(u32, f32)> = points.iter().copied().filter(|(t, _)| *t < from || *t > at || (*t == from && from != at)).collect();
        points.push((at, v));
        points.sort_by_key(|(t, _)| *t);
        // ponytail: the clip's curves go straight while recording; keep each stretch's bend if that's missed
        self.run("clip.automation", json!({ "index": index, "points": points, "curves": [] }));
        self.auto_last = Some((target, index, at));
    }

    /// Right-click menu on any modulatable control: map it to a macro or LFO. Marks
    /// controls that already have modulation with a dot.
    pub fn mod_menu(&mut self, resp: &Response, target: Target) {
        self.record_automation(resp, target);
        // a source dropped here: mapped at half, like the menu does (macros aren't targets)
        if !matches!(target, Target::Macro { .. }) {
            if kit::drag_over::<Source>(resp).is_some() {
                resp.ctx.layer_painter(resp.layer_id).rect_stroke(resp.rect.shrink(1.0), 4.0, Stroke::new(2.0, self.theme.select), egui::StrokeKind::Inside);
            }
            if let Some(source) = kit::dropped::<Source>(resp) {
                self.run("mod.map", json!({ "source": *source, "target": target, "amount": 0.5 }));
                self.say_as(Tone::Done, tr!("Mapped at 50%: fine-tune the amount on the Modulation tab (F10)."));
            }
        }
        let p = &self.session.project;
        let mapped: Vec<usize> = p.mappings.iter().enumerate().filter(|(_, m)| m.target == target).map(|(i, _)| i).collect();
        if !mapped.is_empty() {
            resp.ctx.layer_painter(resp.layer_id).circle_filled(resp.rect.right_top() + vec2(-6.0, 6.0), 3.0, self.theme.select);
        }
        // an automation clip or a pattern's line moves it: a dot at the bottom
        if p.clips.iter().any(|c| matches!(&c.kind, ClipKind::Automation { target: t, .. } if *t == target))
            || p.patterns.iter().any(|pat| pat.automation.iter().any(|a| a.target == target))
        {
            resp.ctx.layer_painter(resp.layer_id).circle_filled(resp.rect.right_bottom() + vec2(-6.0, -6.0), 3.0, self.theme.accent);
        }
        // MIDI: an accent dot on the left when a controller moves it, a ring while it waits for one
        let controller = p.midi_map.iter().find(|m| m.target == target).map(|m| (m.channel, m.cc));
        let waiting = self.learning == Some(target);
        let dot = resp.rect.left_top() + vec2(6.0, 6.0);
        if controller.is_some() {
            resp.ctx.layer_painter(resp.layer_id).circle_filled(dot, 3.0, self.theme.accent);
        }
        if waiting {
            resp.ctx.layer_painter(resp.layer_id).circle_stroke(dot, 5.0, Stroke::new(1.5, self.theme.accent));
            resp.ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let (mut learn, mut forget) = (false, false);
        let macros: Vec<String> = p.macros.iter().map(|m| m.name.clone()).collect();
        let lfos = p.lfos.len();
        let (mut chosen, mut new_lfo, mut clear, mut automate, mut rename, mut in_pattern) = (None, false, false, false, None, false);
        let pattern_name = p.patterns.get(self.session.pattern).map_or(String::new(), |p| p.name.clone());
        resp.context_menu(|ui| {
            let r = ui.button(tr!("{}  Create automation clip", icons::CHART_LINE));
            if r.clicked() {
                automate = true;
                ui.close();
            }
            r.on_hover_text(tr!("A line on the playlist that moves this knob while the song plays: click it to add points, drag them."));
            let r = ui.button(tr!("{}  Automate in \"{}\"", icons::SQUARES_FOUR, pattern_name));
            if r.clicked() {
                in_pattern = true;
                ui.close();
            }
            r.on_hover_text(tr!("A line inside the current pattern: it moves this knob wherever the pattern plays. Drawn under the piano roll (AUTO)."));
            if let Target::Macro { index } = target {
                // macros are the sources: renamed, automated, learned, not modulated
                if ui.button(tr!("{}  Rename…", icons::PENCIL_SIMPLE)).clicked() {
                    rename = Some(index);
                    ui.close();
                }
            } else {
                ui.separator();
                ui.label(RichText::new(tr!("Modulate with")).small());
                for (i, name) in macros.iter().enumerate() {
                    if ui.button(name).clicked() {
                        chosen = Some(Source::Macro(i as u8));
                        ui.close();
                    }
                }
                ui.separator();
                for i in 0..lfos {
                    if ui.button(format!("LFO {}", i + 1)).clicked() {
                        chosen = Some(Source::Lfo(i as u8));
                        ui.close();
                    }
                }
                if lfos < MAX_LFOS && ui.button(tr!("New LFO")).clicked() {
                    new_lfo = true;
                    ui.close();
                }
                if !mapped.is_empty() {
                    ui.separator();
                    if ui.button(tr!("Remove modulation")).clicked() {
                        clear = true;
                        ui.close();
                    }
                }
            }
            ui.separator();
            let label = if waiting { tr!("{}  Waiting for a controller…", icons::CROSSHAIR) } else { format!("{}  MIDI learn", icons::CROSSHAIR) };
            if ui.button(label).clicked() {
                learn = true;
                ui.close();
            }
            if let Some((channel, cc)) = controller
                && ui.button(tr!("Forget MIDI (CC {}, channel {})", cc, channel + 1)).clicked()
            {
                forget = true;
                ui.close();
            }
        });
        if automate {
            // under everything on the playlist, as long as the song, flat at where the knob is now
            let p = &self.session.project;
            let v = target_knob(p, target).map_or(0.5, |(info, v)| info.to_norm(v));
            let (lane, length) = (p.clips.iter().map(|c| c.lane + 1).max().unwrap_or(0), p.song_end());
            self.run("clip.add", json!({ "lane": lane, "start": 0, "length": length, "automation": target, "points": [[0, v], [length, v]] }));
            self.playlist.selected = std::collections::BTreeSet::from([self.session.project.clips.len() - 1]);
            self.say_as(Tone::Done, tr!("Automation clip for {} on playlist lane {}: click its line to add points, drag them", target_name(&self.session.project, target), lane + 1));
        }
        if in_pattern {
            let p = &self.session.project;
            let v = target_knob(p, target).map_or(0.5, |(info, v)| info.to_norm(v));
            if !p.patterns.get(self.session.pattern).is_some_and(|pat| pat.automation.iter().any(|a| a.target == target)) {
                self.run("pattern.automation", json!({ "target": target, "points": [[0, v]] }));
            }
            self.piano.auto = Some(target);
            self.tab = crate::Tab::PianoRoll;
            self.say_as(Tone::Done, tr!("{} in \"{}\": draw it under the piano roll; it plays wherever the pattern does", target_name(&self.session.project, target), pattern_name));
        }
        if let Some(index) = rename {
            let name = self.session.project.macros.get(index as usize).map_or(String::new(), |m| m.name.clone());
            self.rename = Some(Renaming::new(Rename::Macro(index as usize), name));
        }
        if learn {
            self.learning = Some(target);
            self.say_as(Tone::Action, tr!("Move a knob or fader on your MIDI controller (Esc cancels)"));
        }
        if forget {
            self.run("midi.forget", json!({ "target": target }));
        }
        if new_lfo {
            self.run("lfo.add", json!({}));
            chosen = Some(Source::Lfo(lfos as u8));
        }
        if let Some(source) = chosen {
            self.run("mod.map", json!({ "source": source, "target": target, "amount": 0.5 }));
            self.say_as(Tone::Done, tr!("Mapped at 50%: fine-tune the amount on the Modulation tab (F10)."));
        }
        if clear {
            for i in mapped.into_iter().rev() {
                self.run("mod.remove", json!({ "index": i }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row's menu can delete its track, channel or LFO while the loop drawing the rows
    /// still has the old count: the rows after it are skipped instead of panicking.
    #[test]
    fn rows_past_a_deletion_are_skipped() {
        let mut app = App::for_tests();
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let t = app.theme.clone();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let past = app.session.project.mixer.len();
            assert_eq!(app.strip(ui, &t, past, 400.0, true), Rect::NOTHING);
            app.lfo_card(ui, &t, app.session.project.lfos.len());
            let channels = app.session.project.channels.len();
            app.rack_head(ui, &t, channels);
            app.rack_steps(ui, &t, channels, None);
        });
        out.textures_delta.clear(); // no GPU here to hand the font atlas to
    }

    /// Turning a knob while the song plays writes its moves into an automation clip, over
    /// what was there; past the clip's end a new one starts.
    #[test]
    fn turning_a_knob_records_automation() {
        let mut app = App::for_tests();
        let target = Target::TrackGain { track: 1 };
        let turn = |app: &mut App, gain: f32, tick: u32| {
            app.session.run("mixer.set", json!({ "track": 1, "gain": gain })).unwrap();
            app.record_point(target, tick);
        };
        turn(&mut app, 0.2, 0);
        turn(&mut app, 0.9, 2 * BAR);
        let ClipKind::Automation { points, .. } = &app.session.project.clips[0].kind else { panic!("not automation") };
        assert_eq!(points.iter().map(|p| p.0).collect::<Vec<_>>(), vec![0, 2 * BAR, 8 * BAR], "the moves, then the old line on");
        assert!(points[1].1 > points[0].1);
        app.auto_last = None; // a new drag over the same stretch replaces it
        turn(&mut app, 0.5, BAR);
        turn(&mut app, 0.5, 3 * BAR);
        let ClipKind::Automation { points, .. } = &app.session.project.clips[0].kind else { panic!("not automation") };
        assert_eq!(points.iter().map(|p| p.0).collect::<Vec<_>>(), vec![0, BAR, 3 * BAR, 8 * BAR], "the point at bar 2 gone");
        turn(&mut app, 0.5, 9 * BAR);
        assert_eq!((app.session.project.clips.len(), app.session.project.clips[1].start), (2, 9 * BAR), "past the end: a new clip from the bar");
    }
}
