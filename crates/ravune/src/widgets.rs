//! Ravune's custom widgets, painted only from theme tokens.

use std::f32::consts::{PI, TAU};

use eframe::egui::{
    Align2, CornerRadius, EventFilter, FontId, Id, Key, Mesh, Painter, Pos2, Rangef, Rect, Response, RichText, Sense, Shape,
    Stroke, StrokeKind, Ui, Vec2, WidgetInfo, pos2, vec2,
};
use ravune_core::{Curve, LFO_SHAPES, ParamInfo};
use ravune_engine::Bucket;
use serde::{Deserialize, Serialize};

use crate::kit;
use crate::theme::Theme;

/// Keyboard for a focused value control: Up / Down nudge it (Shift = fine), Delete resets.
/// Returns the new value in 0..1 travel, or `None`.
fn keyboard_travel(ui: &Ui, resp: &Response, travel: f32, steps: Option<f32>) -> Option<f32> {
    if !resp.has_focus() {
        return None;
    }
    ui.memory_mut(|m| m.set_focus_lock_filter(resp.id, EventFilter { vertical_arrows: true, ..Default::default() }));
    let (up, down, fine) = ui.input(|i| (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown), i.modifiers.shift));
    let step = steps.map_or(if fine { 0.002 } else { 0.02 }, |n| 1.0 / n);
    match (up, down) {
        (true, false) => Some((travel + step).min(1.0)),
        (false, true) => Some((travel - step).max(0.0)),
        _ => None,
    }
}

/// The wheel turns a control only once it's selected (clicked, dragged or tabbed to), so scrolling
/// a panel past it leaves it alone; then it takes the wheel and the panel doesn't scroll.
fn wheel(ui: &Ui, resp: &Response) -> f32 {
    if resp.clicked() || resp.drag_started() {
        resp.request_focus();
    }
    if !(resp.hovered() && resp.has_focus()) {
        return 0.0;
    }
    let delta = ui.input(|i| i.smooth_scroll_delta.y);
    if delta != 0.0 {
        ui.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
    }
    delta
}

fn reset_pressed(ui: &Ui, resp: &Response) -> bool {
    resp.has_focus() && ui.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace))
}

const CELL: Vec2 = vec2(58.0, 76.0);

/// Rotary knob: drag up/down (Shift = fine), scroll, double-click to reset.
/// Returns the new value when the user changed it.
/// A parameter's control: a switch for an on/off one, a knob for the rest.
pub fn param(ui: &mut Ui, t: &Theme, info: &ParamInfo, value: f32) -> (Response, Option<f32>) {
    if info.unit == "onoff" { switch(ui, t, info, value) } else { knob(ui, t, info, value) }
}

/// An on/off parameter, in a knob's cell so it lines up with them: its name, a switch,
/// On or Off. Click (or Space) flips it.
pub fn switch(ui: &mut Ui, t: &Theme, info: &ParamInfo, value: f32) -> (Response, Option<f32>) {
    let (rect, resp) = ui.allocate_exact_size(CELL, Sense::click());
    let on = value >= 0.5;
    let new = resp.clicked().then_some(if on { info.min } else { info.max });
    let shown = new.map_or(on, |v| v >= 0.5);
    resp.widget_info(|| WidgetInfo::selected(eframe::egui::Role::CheckBox, true, shown, info.name));
    kit::focus_ring(ui, t, &resp, t.radius_md);
    let painter = ui.painter_at(rect);
    kit::fit_text(&painter, pos2(rect.center().x, rect.top()), Align2::CENTER_TOP, info.name, FontId::proportional(11.0), t.fg_muted, rect.width());
    // a pill with its knob sliding to the lit side
    let pill = Rect::from_center_size(pos2(rect.center().x, rect.top() + 34.0), vec2(36.0, 20.0));
    let k = ui.ctx().animate_bool_with_time(resp.id.with("switch"), shown, t.motion);
    let hot = resp.hovered();
    painter.rect_filled(pill, CornerRadius::same(10), Theme::mix(t.bg_field, t.accent, k));
    let x = pill.left() + 10.0 + k * (pill.width() - 20.0);
    painter.circle_filled(pos2(x, pill.center().y), 7.0, if hot { Theme::mix(t.bg_control, t.fg_text, 0.35) } else { Theme::mix(t.bg_control, t.fg_text, 0.2) });
    let text = if shown { "On" } else { "Off" };
    painter.text(pos2(rect.center().x, rect.bottom()), Align2::CENTER_BOTTOM, crate::i18n::t(text), Theme::mono(11.0), if shown { t.accent } else { t.fg_text });
    (resp, new)
}

pub fn knob(ui: &mut Ui, t: &Theme, info: &ParamInfo, value: f32) -> (Response, Option<f32>) {
    let (rect, resp) = ui.allocate_exact_size(CELL, Sense::click_and_drag());
    let id = resp.id;
    let norm = info.to_norm(value);
    let mut new = None;
    if resp.drag_started() {
        ui.data_mut(|d| d.insert_temp(id, norm));
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let delta = -resp.drag_delta().y * if fine { 0.0015 } else { 0.006 };
        // accumulate unrounded travel so stepped knobs still move under small drags
        let travel = (ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(norm) + delta).clamp(0.0, 1.0);
        ui.data_mut(|d| d.insert_temp(id, travel));
        new = Some(info.from_norm(travel));
    } else {
        let scroll = wheel(ui, &resp);
        if scroll != 0.0 {
            new = Some(info.from_norm(norm + scroll * 0.0015));
        }
    }
    // semitones and cents: arrows move a tenth, Ctrl lands on whole semitones
    let steps = if info.unit == "semi" { Some((info.max - info.min) * 10.0) } else { (info.curve == Curve::Steps).then_some(info.max - info.min) };
    if let Some(travel) = keyboard_travel(ui, &resp, norm, steps) {
        new = Some(info.from_norm(travel));
    }
    if info.unit == "semi" {
        let whole = ui.input(|i| i.modifiers.command);
        new = new.map(|v| if whole { v.round() } else { (v * 100.0).round() / 100.0 });
    }
    if resp.double_clicked() || reset_pressed(ui, &resp) {
        new = Some(info.default);
    }

    let shown = new.unwrap_or(value);
    resp.widget_info(|| WidgetInfo::slider(true, shown as f64, format!("{} {}", info.name, info.format(shown))));
    kit::focus_ring(ui, t, &resp, t.radius_md);
    let painter = ui.painter_at(rect);
    let (c, r) = (pos2(rect.center().x, rect.top() + 34.0), 16.0);
    let (a0, sweep) = (0.75 * PI, 1.5 * PI);
    let a = a0 + sweep * info.to_norm(shown);
    arc(&painter, c, r, a0, a0 + sweep, Stroke::new(3.0, t.bg_field));
    arc(&painter, c, r, a0, a, Stroke::new(3.0, t.accent));
    let hot = resp.hovered() || resp.dragged();
    // turning: a soft halo around the arc
    let turning = ui.ctx().animate_bool_with_time(resp.id.with("glow"), resp.dragged(), t.motion);
    if turning > 0.0 {
        painter.circle_stroke(c, r + 2.5, Stroke::new(4.0, t.accent.gamma_multiply(0.18 * turning)));
    }
    painter.circle_filled(c, r - 5.0, if hot { Theme::mix(t.bg_control, t.fg_text, 0.12) } else { t.bg_control });
    let dir = vec2(a.cos(), a.sin());
    painter.line_segment([c + dir * 3.0, c + dir * (r - 6.0)], Stroke::new(1.5, t.fg_text));
    // a long name ends in "…" (the whole of it on hover)
    let resp = if kit::fit_text(&painter, pos2(c.x, rect.top()), Align2::CENTER_TOP, info.name, FontId::proportional(11.0), t.fg_muted, rect.width()).1 {
        resp.on_hover_text(info.name)
    } else {
        resp
    };
    let value_color = if resp.dragged() { t.accent } else { t.fg_text };
    // a long value gets a little smaller to fit, then ends in "…" (the whole of it on hover)
    let (text, mut font) = (info.format(shown), Theme::mono(11.0));
    let wide = painter.layout_no_wrap(text.clone(), font.clone(), value_color).size().x;
    if wide > rect.width() {
        font.size = (font.size * rect.width() / wide).max(9.0);
    }
    let resp = if kit::fit_text(&painter, pos2(c.x, rect.bottom()), Align2::CENTER_BOTTOM, text.clone(), font, value_color, rect.width()).1 {
        resp.on_hover_text(text)
    } else {
        resp
    };
    (resp, new)
}

/// Four lights for the beats of the bar: the one playing flashes and fades (the downbeat in
/// the accent color); all dim when stopped.
pub fn beat_counter(ui: &mut Ui, t: &Theme, beat: Option<f64>, calm: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(4.0 * 9.0, 24.0), Sense::hover());
    let painter = ui.painter();
    for k in 0..4 {
        let c = pos2(rect.left() + 4.5 + k as f32 * 9.0, rect.center().y);
        let (now, fade) = match beat {
            Some(b) if (b.floor() as i64).rem_euclid(4) == k => (true, if calm { 1.0 } else { (1.0 - b.fract() as f32).powf(1.5) }),
            _ => (false, 0.0),
        };
        let color = if k == 0 { t.accent } else { t.fg_text };
        painter.circle_filled(c, 2.5, Theme::mix(t.bg_control, color, if now { 0.35 + 0.65 * fade } else { 0.0 }));
        if now && fade > 0.0 && !calm {
            painter.circle_filled(c, 2.5 + 3.0 * fade, color.gamma_multiply(0.15 * fade));
        }
    }
    resp
}

/// A small knob for tight rows (a channel's volume in the rack), centred on `center`:
/// drag up or down (Shift: finer), the wheel, double-click for `default`. 0..1.
pub fn mini_knob(ui: &mut Ui, t: &Theme, center: Pos2, value: f32, default: f32, id: Id) -> (Response, Option<f32>) {
    let rect = Rect::from_center_size(center, vec2(20.0, 20.0));
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let mut new = None;
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        new = Some((value - resp.drag_delta().y * if fine { 0.0015 } else { 0.006 }).clamp(0.0, 1.0));
    } else {
        let scroll = wheel(ui, &resp);
        if scroll != 0.0 {
            new = Some((value + scroll * 0.0015).clamp(0.0, 1.0));
        }
    }
    if resp.double_clicked() {
        new = Some(default);
    }
    let shown = new.unwrap_or(value);
    let hot = resp.hovered() || resp.dragged();
    let painter = ui.painter();
    let (c, r) = (rect.center(), 8.5);
    let (a0, sweep) = (0.75 * PI, 1.5 * PI);
    arc(painter, c, r, a0, a0 + sweep, Stroke::new(2.0, t.bg_field));
    arc(painter, c, r, a0, a0 + sweep * shown, Stroke::new(2.0, t.accent));
    painter.circle_filled(c, r - 3.0, if hot { Theme::mix(t.bg_control, t.fg_text, 0.15) } else { t.bg_control });
    let a = a0 + sweep * shown;
    painter.line_segment([c + vec2(a.cos(), a.sin()) * 1.5, c + vec2(a.cos(), a.sin()) * (r - 3.5)], Stroke::new(1.2, t.fg_text));
    resp.widget_info(|| WidgetInfo::slider(true, shown as f64, "Volume"));
    kit::focus_ring(ui, t, &resp, t.radius_md);
    (resp, new)
}

/// How much of a download is in: a ring filling clockwise from the top (0..1).
pub fn progress_ring(painter: &Painter, t: &Theme, center: Pos2, r: f32, done: f32) {
    painter.circle_stroke(center, r, Stroke::new(2.0, t.bg_field));
    let top = -0.5 * PI;
    arc(painter, center, r, top, top + 2.0 * PI * done.clamp(0.0, 1.0), Stroke::new(2.0, t.select));
}

fn arc(painter: &Painter, c: Pos2, r: f32, a0: f32, a1: f32, stroke: Stroke) {
    if (a1 - a0).abs() < 0.01 {
        return;
    }
    let steps = ((a1 - a0).abs() * r / 2.0).ceil() as usize;
    let pts = (0..=steps)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / steps as f32;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    painter.line(pts, stroke);
}

/// Four waveform buttons in a knob-sized cell. Returns the picked wave index.
/// Picks one of the first `count` of [`LFO_SHAPES`] (4: the synth's waves): two per row, three
/// when there are more than four.
pub fn wave_picker(ui: &mut Ui, t: &Theme, label: &str, value: f32, count: usize) -> (Response, Option<f32>) {
    let cols = if count > 4 { 3 } else { 2 };
    let (rect, resp) = ui.allocate_exact_size(vec2(CELL.x.max(2.0 + cols as f32 * 28.0), CELL.y), Sense::click());
    let painter = ui.painter_at(rect);
    painter.text(pos2(rect.center().x, rect.top()), Align2::CENTER_TOP, label, FontId::proportional(11.0), t.fg_muted);
    let hover = resp.hover_pos();
    let mut picked = None;
    for i in 0..count {
        let min = pos2(rect.left() + 2.0 + (i % cols) as f32 * 28.0, rect.top() + 17.0 + (i / cols) as f32 * 22.0);
        let cell = Rect::from_min_size(min, vec2(26.0, 19.0));
        let (on, hot) = (value as usize == i, hover.is_some_and(|p| cell.contains(p)));
        if hot && resp.clicked() {
            picked = Some(i as f32);
        }
        let fill = if hot { Theme::mix(t.bg_control, t.fg_text, 0.12) } else { t.bg_control };
        let corners = CornerRadius::same(t.radius_sm);
        painter.rect_filled(cell, corners, fill);
        if on {
            painter.rect_stroke(cell, corners, Stroke::new(1.0, t.accent), StrokeKind::Inside);
        }
        let ink = if on { t.accent } else { t.fg_muted };
        wave_icon(&painter, cell.shrink2(vec2(6.0, 5.0)), i, Stroke::new(1.5, ink));
    }
    let name = LFO_SHAPES[value as usize % count];
    painter.text(pos2(rect.center().x, rect.bottom()), Align2::CENTER_BOTTOM, name, Theme::mono(11.0), t.fg_text);
    (resp, picked)
}

fn wave_icon(painter: &Painter, r: Rect, wave: usize, stroke: Stroke) {
    let pts = (0..=48)
        .map(|i| {
            let ph = i as f32 / 48.0;
            let y = match wave {
                0 => (ph * TAU).sin(),
                1 => 1.0 - 4.0 * (ph - 0.5).abs(),
                2 => 2.0 * ph - 1.0,
                3 => if ph < 0.5 { 1.0 } else { -1.0 },
                _ => [0.4, -0.6, 0.9, -0.2, -0.9][((ph * 5.0) as usize).min(4)],
            };
            pos2(r.left() + ph * r.width(), r.center().y - y * r.height() / 2.0)
        })
        .collect();
    painter.line(pts, stroke);
}

/// Live drawing of the ADSR envelope.
pub fn adsr_preview(ui: &mut Ui, t: &Theme, [a, d, s, r]: [f32; 4]) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(112.0, CELL.y), Sense::hover());
    let area = Rect::from_min_max(rect.min + vec2(0.0, 16.0), rect.max - vec2(0.0, 14.0));
    let painter = ui.painter_at(rect);
    painter.rect_filled(area, CornerRadius::same(t.radius_sm), t.bg_field);
    let inner = area.shrink(5.0);
    let w = [(a / 4.0).sqrt(), (d / 4.0).sqrt(), 0.35, (r / 8.0).sqrt()];
    let total: f32 = w.iter().sum();
    let x = |frac: f32| inner.left() + inner.width() * frac / total;
    let y = |level: f32| inner.bottom() - level * inner.height();
    let pts = vec![
        pos2(x(0.0), y(0.0)),
        pos2(x(w[0]), y(1.0)),
        pos2(x(w[0] + w[1]), y(s)),
        pos2(x(w[0] + w[1] + w[2]), y(s)),
        pos2(x(total), y(0.0)),
    ];
    painter.line(pts, Stroke::new(1.5, t.accent));
    painter.text(pos2(rect.center().x, rect.top()), Align2::CENTER_TOP, "ADSR", FontId::proportional(11.0), t.fg_muted);
    resp
}

pub struct PianoInput {
    pub press: Option<u8>,
    pub release: Option<u8>,
}

/// On-screen keyboard. `held` lights keys; `label` shows the computer key for a note.
pub fn piano(
    ui: &mut Ui,
    t: &Theme,
    low: u8,
    octaves: u8,
    held: impl Fn(u8) -> bool,
    label: impl Fn(u8) -> Option<&'static str>,
    mouse_note: &mut Option<u8>,
) -> PianoInput {
    const WHITE_STEPS: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
    let whites = octaves as usize * 7 + 1;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 96.0), Sense::click_and_drag());
    let ww = rect.width() / whites as f32;
    let mut white_keys = Vec::with_capacity(whites);
    let mut black_keys = Vec::new();
    for i in 0..whites {
        let note = low + 12 * (i / 7) as u8 + WHITE_STEPS[i % 7];
        let x = rect.left() + i as f32 * ww;
        white_keys.push((note, Rect::from_min_size(pos2(x + 0.5, rect.top()), vec2(ww - 1.0, rect.height()))));
        if i + 1 < whites && !matches!(note % 12, 4 | 11) {
            let bw = ww * 0.58;
            let r = Rect::from_min_size(pos2(x + ww - bw / 2.0, rect.top()), vec2(bw, rect.height() * 0.6));
            black_keys.push((note + 1, r));
        }
    }

    let hit = |p: Pos2| {
        black_keys.iter().chain(white_keys.iter()).find(|(_, r)| r.contains(p)).map(|(n, _)| *n)
    };
    let under = if resp.is_pointer_button_down_on() { resp.interact_pointer_pos().and_then(hit) } else { None };
    let mut input = PianoInput { press: None, release: None };
    if under != *mouse_note {
        input = PianoInput { press: under, release: *mouse_note };
        *mouse_note = under;
    }

    let painter = ui.painter_at(rect);
    let ctx = ui.ctx().clone();
    let draw = |keys: &[(u8, Rect)], base, ink| {
        for &(note, r) in keys {
            let lit = ctx.animate_bool_with_time(resp.id.with(note), held(note), 0.08);
            let corners = CornerRadius { nw: 0, ne: 0, sw: t.radius_sm, se: t.radius_sm };
            painter.rect_filled(r, corners, Theme::mix(base, t.accent, lit));
            let ink = Theme::mix(ink, t.bg_base, lit);
            if let Some(l) = label(note) {
                painter.text(pos2(r.center().x, r.bottom() - 5.0), Align2::CENTER_BOTTOM, l, Theme::mono(10.0), ink);
            }
            if note % 12 == 0 {
                let name = format!("C{}", note as i32 / 12 - 1);
                painter.text(pos2(r.center().x, r.bottom() - 20.0), Align2::CENTER_BOTTOM, name, Theme::mono(9.0), ink);
            }
        }
    };
    draw(&white_keys, t.key_white, t.key_label);
    draw(&black_keys, t.key_black, t.fg_muted);
    input
}

/// Horizontal level meter; linear amplitude in, dB-scaled bar out.
pub fn meter(ui: &mut Ui, t: &Theme, level: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(84.0, 6.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::ZERO, t.bg_field);
    let db = 20.0 * level.max(1e-6).log10();
    let fill = ((db + 48.0) / 48.0).clamp(0.0, 1.0);
    if fill > 0.0 {
        let color = if db > -3.0 { t.meter_hot } else if db > -12.0 { t.meter_warn } else { t.meter_ok };
        painter.rect_filled(Rect::from_min_size(rect.min, vec2(rect.width() * fill, rect.height())), CornerRadius::ZERO, color);
    }
    resp
}

/// Mixer fader with the track's meter beside it. Gain is linear (0..1.25, 1 = 0 dB); travel
/// is cubic so the useful range gets most of the throw. Drag (Shift = fine), scroll,
/// double-click = 0 dB. Returns the new gain when the user changed it.
pub fn fader(ui: &mut Ui, t: &Theme, gain: f32, level: f32, hold: f32, height: f32) -> (Response, Option<f32>) {
    const MAX: f32 = 1.25;
    let to_travel = |g: f32| (g / MAX).clamp(0.0, 1.0).cbrt();
    let (rect, resp) = ui.allocate_exact_size(vec2(58.0, height.max(36.0)), Sense::click_and_drag());
    let groove = Rect::from_min_max(pos2(rect.left() + 24.0, rect.top() + 6.0), pos2(rect.left() + 28.0, rect.bottom() - 6.0));
    let travel = to_travel(gain);
    let mut new = None;
    if resp.drag_started() {
        ui.data_mut(|d| d.insert_temp(resp.id, travel));
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let delta = -resp.drag_delta().y / groove.height() * if fine { 0.25 } else { 1.0 };
        let moved = (ui.data(|d| d.get_temp::<f32>(resp.id)).unwrap_or(travel) + delta).clamp(0.0, 1.0);
        ui.data_mut(|d| d.insert_temp(resp.id, moved));
        new = Some(MAX * moved.powi(3));
    } else {
        let scroll = wheel(ui, &resp);
        if scroll != 0.0 {
            new = Some(MAX * (travel + scroll * 0.001).clamp(0.0, 1.0).powi(3));
        }
    }
    if let Some(travel) = keyboard_travel(ui, &resp, travel, None) {
        new = Some(MAX * travel.powi(3));
    }
    if resp.double_clicked() || reset_pressed(ui, &resp) {
        new = Some(1.0);
    }
    resp.widget_info(|| WidgetInfo::slider(true, new.unwrap_or(gain) as f64, format!("Volume {}", db(new.unwrap_or(gain)))));
    kit::focus_ring(ui, t, &resp, t.radius_sm);

    let painter = ui.painter_at(rect);
    let y = |travel: f32| groove.bottom() - travel * groove.height();
    painter.rect_filled(groove, CornerRadius::same(2), t.bg_field);
    let unity = y(to_travel(1.0));
    painter.line_segment([pos2(groove.left() - 6.0, unity), pos2(groove.right() + 6.0, unity)], Stroke::new(1.0, t.line));
    // the scale: dB marks on the left (fewer when the fader is short), ticks on the right
    let marks: &[f32] = if groove.height() > 110.0 { &[6.0, 0.0, -6.0, -12.0, -24.0, -48.0] } else { &[0.0, -12.0] };
    for &mark in marks {
        let at = y(to_travel(10f32.powf(mark / 20.0)));
        painter.line_segment([pos2(groove.right() + 3.0, at), pos2(groove.right() + 6.0, at)], Stroke::new(1.0, t.line));
        let label = if mark > 0.0 { format!("+{mark:.0}") } else { format!("{mark:.0}") };
        painter.text(pos2(groove.left() - 6.0, at), Align2::RIGHT_CENTER, label, Theme::mono(8.0), t.fg_muted);
    }
    // a cap with a centre line, like a hardware fader (Traktor's look)
    let handle = Rect::from_center_size(pos2(groove.center().x, y(to_travel(new.unwrap_or(gain)))), vec2(20.0, 12.0));
    let hot = resp.hovered() || resp.dragged();
    painter.rect_filled(handle, CornerRadius::same(2), if resp.dragged() { t.accent } else if hot { t.fg_text } else { t.fg_muted });
    painter.hline(handle.x_range().shrink(3.0), handle.center().y, Stroke::new(1.0, t.bg_base));

    // LED meter: -60..+6 dB in 3-px segments, green / yellow / red by level, bottom up
    let bar = Rect::from_min_max(pos2(rect.left() + 42.0, groove.top()), pos2(rect.left() + 50.0, groove.bottom()));
    painter.rect_filled(bar, CornerRadius::ZERO, t.bg_field);
    let fill_of = |gain: f32| ((20.0 * gain.max(1e-6).log10() + 60.0) / 66.0).clamp(0.0, 1.0);
    let (fill, held) = (fill_of(level), fill_of(hold));
    const SEGMENT: f32 = 4.0;
    let count = (bar.height() / SEGMENT).floor().max(1.0) as usize;
    for k in 0..count {
        let at = (k as f32 + 0.5) / count as f32;
        let db = at * 66.0 - 60.0;
        let color = if db > -0.5 { t.meter_hot } else if db > -12.0 { t.meter_warn } else { t.meter_ok };
        let lit = at <= fill || (held > 0.02 && (at - held).abs() <= 0.5 / count as f32);
        let top = bar.bottom() - (k as f32 + 1.0) * SEGMENT;
        let segment = Rect::from_min_max(pos2(bar.left() + 1.0, top + 1.0), pos2(bar.right() - 1.0, top + SEGMENT));
        painter.rect_filled(segment, CornerRadius::ZERO, if lit { color } else { Theme::mix(t.bg_field, color, 0.12) });
    }
    (resp, new)
}

/// Gain as dB text for fader readouts.
pub fn db(gain: f32) -> String {
    if gain < 1e-4 { "-inf dB".into() } else { format!("{:+.1} dB", 20.0 * gain.log10()) }
}

/// How sample waveforms are colored (Settings).
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum WaveStyle {
    /// One color that brightens where the highs are.
    #[default]
    Mono,
    /// Hue from the low/mid/high balance.
    Rgb,
    /// Low, mid and high drawn as layers, like CDJs and rekordbox.
    ThreeBand,
}

/// Draws a sample's overview stretched across `span` (screen x), within `rect`'s height.
/// One line per pixel column, each the max of the buckets under it; only visible columns.
pub fn waveform(painter: &Painter, t: &Theme, style: WaveStyle, wave: &[Bucket], span: Rangef, rect: Rect) {
    let n = wave.len();
    let clip = painter.clip_rect().intersect(rect);
    if n == 0 || span.span() <= 0.0 || clip.width() <= 0.0 {
        return;
    }
    let (mid, half) = (rect.center().y, rect.height() * 0.5);
    // one column per physical pixel, edge to edge: crisp at any interface scale, no gaps
    let ppp = painter.ctx().pixels_per_point();
    let px = 1.0 / ppp;
    let per_pt = n as f32 / span.span();
    let mut layers = vec![Mesh::default(); if style == WaveStyle::ThreeBand { 3 } else { 1 }];
    let mut x = (clip.left().max(span.min) * ppp).floor() / ppp;
    let end = clip.right().min(span.max);
    while x < end {
        let (f0, f1) = ((x - span.min) * per_pt, (x + px - span.min) * per_pt);
        let a = (f0.max(0.0) as usize).min(n - 1);
        let c = if f1 - f0 >= 1.0 {
            // many buckets under this pixel: their envelope
            let b = (f1.ceil() as usize).clamp(a + 1, n);
            wave[a..b].iter().fold(wave[a], |c, k| Bucket {
                lo: c.lo.min(k.lo),
                hi: c.hi.max(k.hi),
                bands: std::array::from_fn(|i| c.bands[i].max(k.bands[i])),
            })
        } else {
            // zoomed in: between two buckets, so the shape flows instead of stepping
            let f = ((f0 + f1) * 0.5 - 0.5).clamp(0.0, (n - 1) as f32);
            let (i, k) = (f as usize, f.fract());
            let (p, q) = (wave[i], wave[(i + 1).min(n - 1)]);
            Bucket { lo: p.lo + (q.lo - p.lo) * k, hi: p.hi + (q.hi - p.hi) * k, bands: std::array::from_fn(|b| p.bands[b] + (q.bands[b] - p.bands[b]) * k) }
        };
        // at least one physical pixel tall, so quiet parts still read
        let column = |mesh: &mut Mesh, top: f32, bottom: f32, color| {
            let (y0, y1) = (mid - top * half, mid - bottom * half);
            let grow = ((px - (y1 - y0)) * 0.5).max(0.0);
            mesh.add_colored_rect(Rect::from_min_max(pos2(x, y0 - grow), pos2(x + px, y1 + grow)), color);
        };
        match style {
            WaveStyle::ThreeBand => {
                for (i, scale) in [(0, 1.0), (1, 1.0), (2, 0.6)] {
                    let h = c.bands[i] * scale;
                    column(&mut layers[i], h, -h, t.wave_bands[i]);
                }
            }
            WaveStyle::Mono => {
                let sum = c.bands.iter().sum::<f32>().max(1e-6);
                column(&mut layers[0], c.hi, c.lo, Theme::mix(t.wave_mono[0], t.wave_mono[1], 2.0 * c.bands[2] / sum));
            }
            WaveStyle::Rgb => {
                let top = c.bands.iter().copied().fold(1e-6, f32::max);
                column(&mut layers[0], c.hi, c.lo, Theme::blend(t.wave_rgb, c.bands.map(|b| b / top)));
            }
        }
        x += px;
    }
    for mesh in layers {
        painter.add(Shape::mesh(mesh));
    }
}

/// Device on/off switch: a lit LED when on, an empty ring when bypassed.
pub fn power(ui: &mut Ui, t: &Theme, on: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
    let c = rect.center();
    if on {
        ui.painter().circle_filled(c, 5.0, t.accent);
    } else {
        let color = if resp.hovered() { t.fg_text } else { t.fg_muted };
        ui.painter().circle_stroke(c, 4.5, Stroke::new(1.5, color));
    }
    resp
}

/// Song position as bars.beats.sixteenths, Ableton-style.
pub fn position(ui: &mut Ui, t: &Theme, beat: f64) -> Response {
    let (bar, b, sixteenth) = ((beat / 4.0) as u32 + 1, beat as u32 % 4 + 1, (beat.fract() * 4.0) as u32 + 1);
    let (rect, resp) = ui.allocate_exact_size(vec2(78.0, 22.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(t.radius_sm), t.bg_field);
    let text = format!("{bar:>3}. {b}. {sixteenth}");
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, Theme::mono(12.0), t.fg_text);
    resp
}

/// Wordmark with the raven (assets/mark.png, tinted with the accent). While playing it
/// flares on each bar and fades through it (`phase`: 0 to 1 across the bar).
pub fn logo(ui: &mut Ui, t: &Theme, phase: Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
    let tint = match phase {
        Some(p) => t.accent.gamma_multiply(1.0 - 0.45 * p),
        None => t.accent,
    };
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    ui.painter().image(raven(ui.ctx()).id(), rect, uv, tint);
    ui.label(RichText::new("RAVUNE").font(FontId::proportional(13.0)).strong().extra_letter_spacing(2.5).color(t.fg_text));
}

/// The raven as a texture, uploaded once.
fn raven(ctx: &eframe::egui::Context) -> eframe::egui::TextureHandle {
    let id = Id::unique("ravune-mark");
    if let Some(texture) = ctx.data(|d| d.get_temp::<eframe::egui::TextureHandle>(id)) {
        return texture;
    }
    let png = eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/mark.png")).expect("assets/mark.png is a PNG");
    let image = eframe::egui::ColorImage::from_rgba_unmultiplied([png.width as usize, png.height as usize], &png.rgba);
    let texture = ctx.load_texture("ravune-mark", image, eframe::egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
    texture
}
