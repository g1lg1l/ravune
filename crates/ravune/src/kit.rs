//! Layout kit. Every region is a stack of fixed-height bars and one edge-to-edge scroll
//! area, split by 1-px dividers. Space is padding inside elements, never outer margins, so
//! scroll bars land on the dividers. Motion ported from React Bits: a pointer spotlight on
//! cards, a sliding tab pill, a glowing focus ring, fading list edges.

use std::f32::consts::TAU;
use std::hash::Hash;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Frame, Id, LayerId, Layout, Mesh, Order, Painter, Pos2, Rect, Response, RichText,
    Role, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, pos2, vec2,
};

use crate::theme::Theme;

/// What's being dragged (of type `P`) while the pointer is anywhere over `area`, its own
/// widgets included. egui's `dnd_hover_payload` only counts the spots no child covers,
/// so a row full of buttons and knobs would hardly ever see a drop.
pub fn drag_over<P: std::any::Any + Send + Sync>(area: &Response) -> Option<std::sync::Arc<P>> {
    area.container_contains_pointer().then(|| egui::DragAndDrop::payload::<P>(&area.ctx)).flatten()
}

/// Like [`drag_over`], the frame it's let go: taken, so nothing else gets it too. Only a
/// payload of type `P`: egui's own take would throw away one of another type (a sound
/// let go over something that first asks for a plugin was lost that way).
pub fn dropped<P: std::any::Any + Send + Sync>(area: &Response) -> Option<std::sync::Arc<P>> {
    if egui::DragAndDrop::has_payload_of_type::<P>(&area.ctx) && area.container_contains_pointer() && area.ctx.input(|i| i.pointer.any_released()) {
        egui::DragAndDrop::take_payload::<P>(&area.ctx)
    } else {
        None
    }
}

/// Cubic ease-out, 0..1.
pub fn ease(k: f32) -> f32 {
    1.0 - (1.0 - k.clamp(0.0, 1.0)).powi(3)
}

/// A spring from 0 to 1 that overshoots a little and settles (Sileo's, 0.25 bounce).
pub fn spring(k: f32) -> f32 {
    if k >= 1.0 {
        return 1.0;
    }
    let k = k.max(0.0);
    1.0 - (-8.06 * k).exp() * ((7.11 * k).cos() + 1.134 * (7.11 * k).sin())
}

/// Eased 0..1 hover amount: hovers fade in and out instead of snapping.
pub fn hover(ui: &Ui, t: &Theme, resp: &Response) -> f32 {
    ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered() || resp.has_focus(), t.motion)
}

/// React Bits' Click Spark: short rays bursting from where something was switched on.
#[derive(Clone, Copy)]
pub struct Spark {
    pub pos: Pos2,
    pub at: f64,
    pub color: Color32,
}

const SPARK_LIFE: f64 = 0.38;

/// Draws the live sparks above everything, dropping finished ones.
pub fn sparks(ctx: &egui::Context, sparks: &mut Vec<Spark>, now: f64) {
    sparks.retain(|s| now - s.at < SPARK_LIFE);
    if sparks.is_empty() {
        return;
    }
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("sparks")));
    for s in sparks.iter() {
        let k = ((now - s.at) / SPARK_LIFE) as f32;
        let r0 = 5.0 + 14.0 * ease(k);
        let r1 = r0 + 7.0 * (1.0 - k);
        let color = s.color.gamma_multiply(1.0 - k);
        for i in 0..8 {
            let a = TAU * i as f32 / 8.0 + 0.3;
            let dir = vec2(a.cos(), a.sin());
            painter.line_segment([s.pos + dir * r0, s.pos + dir * r1], Stroke::new(1.6, color));
        }
    }
    ctx.request_repaint();
}

/// Starts a region (panel, window): children stack with no gaps; bars and content bring
/// their own padding.
pub fn region(ui: &mut Ui) {
    ui.spacing_mut().item_spacing = Vec2::ZERO;
}

/// A full-width strip `t.bar_h` tall with horizontal padding and a divider below.
/// Contents are laid out left to right, vertically centred.
pub fn bar<R>(ui: &mut Ui, t: &Theme, add: impl FnOnce(&mut Ui) -> R) -> R {
    bar_with(ui, t, t.bg_panel, add)
}

pub fn bar_with<R>(ui: &mut Ui, t: &Theme, fill: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let rect = strip_rect(ui, t.bar_h, fill);
    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, t.line));
    strip_child(ui, t, rect, add)
}

/// Like [`bar`] without the divider, at any height (status bar).
pub fn strip<R>(ui: &mut Ui, t: &Theme, height: f32, fill: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let rect = strip_rect(ui, height, fill);
    strip_child(ui, t, rect, add)
}

fn strip_rect(ui: &mut Ui, height: f32, fill: Color32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
    rect
}

fn strip_child<R>(ui: &mut Ui, t: &Theme, rect: Rect, add: impl FnOnce(&mut Ui) -> R) -> R {
    let inner = rect.shrink2(vec2(t.pad, 0.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    child.spacing_mut().item_spacing = vec2(t.gap, t.gap);
    add(&mut child)
}

/// A one-line text field. egui leaves the text ~2 px above the middle of its box: a
/// taller top margin sets it (and the placeholder) right in the middle.
pub fn field(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).margin(egui::Margin { left: 8, right: 8, top: 5, bottom: 1 })
}

/// Right-aligned group inside a bar.
pub fn right<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.with_layout(Layout::right_to_left(Align::Center), add).inner
}

/// A 1-px vertical divider spanning the bar it sits in.
pub fn vsep(ui: &mut Ui, t: &Theme) {
    let h = ui.max_rect().height();
    let (rect, _) = ui.allocate_exact_size(vec2(1.0 + 2.0 * t.gap, h), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), Stroke::new(1.0, t.line));
}

/// Small-caps section label.
pub fn caps(ui: &mut Ui, t: &Theme, text: &str) -> Response {
    ui.label(RichText::new(text).font(Theme::mono(10.0)).extra_letter_spacing(1.0).color(t.fg_muted))
}

/// A titled group of controls: a divider edge to edge above it (none above a panel's
/// `first`), room around it, its title, then what's in it.
pub fn section<R>(ui: &mut Ui, t: &Theme, title: &str, first: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    if !first {
        ui.add_space(t.gap);
        let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().hline((line.left() - t.pad)..=(line.right() + t.pad), line.center().y, Stroke::new(1.0, t.line));
        ui.add_space(t.gap);
    }
    caps(ui, t, title);
    add(ui)
}

/// The rest of the region as a vertical scroll area, edge to edge; content gets `t.pad`
/// padding (or none, for full-width rows). The list edges fade when there is more to scroll.
pub fn scroll<R>(ui: &mut Ui, t: &Theme, id: impl Hash + std::fmt::Debug, padded: bool, fill: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let out = egui::ScrollArea::vertical().id_salt(id).auto_shrink(false).show(ui, |ui| {
        let pad = if padded { t.pad } else { 0.0 };
        Frame::new()
            .inner_margin(pad)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.spacing_mut().item_spacing = if padded { vec2(t.gap, t.gap) } else { Vec2::ZERO };
                add(ui)
            })
            .inner
    });
    let (view, offset, content) = (out.inner_rect, out.state.clamped_offset().y, out.content_size.y);
    let fade = 14.0;
    if offset > 1.0 {
        gradient(ui, Rect::from_min_size(view.min, vec2(view.width(), fade)), fill, true);
    }
    if offset + view.height() < content - 1.0 {
        gradient(ui, Rect::from_min_size(pos2(view.left(), view.bottom() - fade), vec2(view.width(), fade)), fill, false);
    }
    out.inner
}

/// Vertical fade from `color` (opaque edge) to transparent, `top` = opaque at the top.
fn gradient(ui: &Ui, rect: Rect, color: Color32, top: bool) {
    let clear = color.gamma_multiply(0.0);
    let (a, b) = if top { (color, clear) } else { (clear, color) };
    let mut mesh = Mesh::default();
    for (pos, c) in [(rect.left_top(), a), (rect.right_top(), a), (rect.right_bottom(), b), (rect.left_bottom(), b)] {
        mesh.colored_vertex(pos, c);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(Shape::mesh(mesh));
}

/// Padded block inside a scroll area or window (cards, forms).
pub fn padded<R>(ui: &mut Ui, t: &Theme, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .inner_margin(t.pad)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(t.gap, t.gap);
            add(ui)
        })
        .inner
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

/// Square icon button. `on` lights it like a hardware toggle. `label` names it for screen
/// readers and the tooltip (add the shortcut there).
/// One line of text no wider than `width`, cut with "…" when it doesn't fit. Returns where
/// it went and whether it was cut (then show the whole of it on hover).
pub fn fit_text(painter: &Painter, pos: Pos2, align: Align2, text: impl Into<String>, font: FontId, color: Color32, width: f32) -> (Rect, bool) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.into(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: width.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = painter.layout_job(job);
    let (rect, cut) = (align.anchor_size(pos, galley.size()), galley.elided);
    painter.galley(rect.min, galley, color);
    (rect, cut)
}

/// A small M / S toggle (mute, solo) centred on `center`, lit in `lit` while on.
#[allow(clippy::too_many_arguments)]
pub fn chip(ui: &Ui, painter: &Painter, t: &Theme, center: Pos2, label: &str, on: bool, lit: Color32, id: egui::Id) -> Response {
    let b = Rect::from_center_size(center, vec2(14.0, 13.0));
    let r = ui.interact(b, id, Sense::click());
    let fill = match on {
        true => lit,
        false if r.hovered() => t.bg_control,
        false => Theme::mix(t.bg_panel, t.bg_control, 0.5),
    };
    painter.rect_filled(b, CornerRadius::same(2), fill);
    painter.text(b.center(), Align2::CENTER_CENTER, label, Theme::mono(9.0), if on { t.bg_base } else { t.fg_muted });
    r
}

/// The dashed ring in the middle of a stretch of an automation line: drag it up or down
/// to bend the stretch. `hot` while it's under the mouse or in hand.
pub fn bend_handle(painter: &Painter, at: Pos2, color: Color32, hot: bool) {
    let r = if hot { 5.0 } else { 4.0 };
    let ring: Vec<Pos2> = (0..=24).map(|k| at + Vec2::angled(k as f32 / 24.0 * TAU) * r).collect();
    painter.extend(Shape::dashed_line(&ring, Stroke::new(1.2, color), 2.0, 2.0));
    if hot {
        painter.circle_filled(at, 1.5, color);
    }
}

/// Faint lines at a quarter, a half and three quarters of an automation line's height:
/// something to place points against.
pub fn quarter_lines(painter: &Painter, x: egui::Rangef, y_of: impl Fn(f32) -> f32, color: Color32) {
    for v in [0.25, 0.5, 0.75] {
        painter.hline(x, y_of(v).round() + 0.5, Stroke::new(1.0, color.gamma_multiply(if v == 0.5 { 0.6 } else { 0.35 })));
    }
}

/// An icon centred on its ink: glyphs carry uneven side bearings, so centring their text box
/// leaves them off to one side.
pub fn icon_at(painter: &Painter, center: Pos2, icon: &str, size: f32, color: Color32) {
    let galley = painter.layout_no_wrap(icon.to_string(), FontId::proportional(size), color);
    let ink = galley.mesh_bounds;
    painter.galley(center - ink.center().to_vec2(), galley, color);
}

/// [`icon_at`], turned by `angle` (radians, clockwise) about its ink's centre: one caret
/// serves open and closed.
pub fn icon_turned(painter: &Painter, center: Pos2, icon: &str, size: f32, color: Color32, angle: f32) {
    let galley = painter.layout_no_wrap(icon.to_string(), FontId::proportional(size), color);
    let ink = galley.mesh_bounds.center().to_vec2();
    let turn = egui::emath::Rot2::from_angle(angle);
    painter.add(egui::epaint::TextShape::new(center - turn * ink, galley, color).with_angle(angle));
}

/// shadcn/ui's button variants.
#[derive(Clone, Copy, PartialEq)]
pub enum Variant {
    Secondary,
    /// A border, no fill.
    Outline,
    /// Nothing until hovered.
    Ghost,
}

/// A text button in one of the [`Variant`]s, with an optional leading icon.
pub fn action(ui: &mut Ui, t: &Theme, variant: Variant, icon: Option<&str>, text: &str) -> Response {
    action_sized(ui, t, variant, icon, text, None)
}

/// An [`action`] `width` wide (its content centered), or as wide as its content.
pub fn action_sized(ui: &mut Ui, t: &Theme, variant: Variant, icon: Option<&str>, text: &str, width: Option<f32>) -> Response {
    let font = Theme::body();
    let text_w = ui.painter().layout_no_wrap(text.to_string(), font.clone(), t.fg_text).size().x;
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let natural = text_w + icon_w + 3.0 * t.pad;
    let (rect, resp) = ui.allocate_exact_size(vec2(width.unwrap_or(natural), Theme::CONTROL_H), Sense::click());
    let enabled = ui.is_enabled();
    let h = if enabled { hover(ui, t, &resp) } else { 0.0 };
    let (fill, stroke) = match variant {
        Variant::Secondary => (Theme::mix(t.bg_control, t.fg_text, 0.08 * h), None),
        Variant::Outline => (t.bg_control.gamma_multiply(h), Some(t.line)),
        Variant::Ghost => (t.bg_control.gamma_multiply(h), None),
    };
    let color = if enabled { t.fg_text } else { t.fg_muted };
    let shape = if resp.is_pointer_button_down_on() { rect.shrink(1.0) } else { rect };
    let painter = ui.painter();
    painter.rect_filled(shape, CornerRadius::same(t.radius_sm), fill);
    if let Some(stroke) = stroke {
        painter.rect_stroke(shape, CornerRadius::same(t.radius_sm), Stroke::new(1.0, stroke), StrokeKind::Inside);
    }
    let mut x = rect.left() + 1.5 * t.pad + ((rect.width() - natural) / 2.0).max(0.0);
    if let Some(icon) = icon {
        icon_at(painter, pos2(x + 7.0, rect.center().y), icon, Theme::TEXT_MD + 2.0, color);
        x += icon_w;
    }
    painter.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, text, font, color);
    focus_ring(ui, t, &resp, t.radius_sm);
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, text));
    resp
}

/// Buttons that share their row's width evenly: the actions under a section.
pub fn actions(ui: &mut Ui, t: &Theme, items: &[(Variant, Option<&str>, &str)]) -> Vec<Response> {
    let n = items.len().max(1) as f32;
    let w = ((ui.available_width() - t.gap * (n - 1.0)) / n).max(0.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = t.gap;
        items.iter().map(|(variant, icon, text)| action_sized(ui, t, *variant, *icon, text, Some(w))).collect()
    })
    .inner
}

/// A divider across a menu, edge to edge.
pub fn menu_line(ui: &mut Ui, t: &Theme) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(ui.max_rect().x_range(), r.center().y, Stroke::new(1.0, t.line));
}

/// A button as wide as where it sits that confirms once held for `hold_secs` (it fills up as
/// it's held), then briefly shows `done_label`. True the frame it confirms.
pub fn hold_button(ui: &mut Ui, t: &Theme, icon: Option<&str>, label: &str, done_label: &str, hold_secs: f32) -> bool {
    #[derive(Clone, Default)]
    struct HoldState {
        started: Option<f64>,
        released: Option<(f64, f32)>,
        progress: f32,
        done_until: Option<f64>,
        needs_release: bool,
    }

    let font = Theme::body();
    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), t.fg_text);
    let done_galley = ui.painter().layout_no_wrap(done_label.to_string(), font, t.fg_text);
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let size = vec2(ui.available_width().max(galley.size().x.max(done_galley.size().x) + icon_w + 2.0 * t.pad), Theme::CONTROL_H);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    let (now, primary, keyboard) = ui.input(|i| (i.time, i.pointer.primary_down(), i.key_down(egui::Key::Enter) || i.key_down(egui::Key::Space)));
    let held = enabled && ((resp.is_pointer_button_down_on() && primary) || (resp.has_focus() && keyboard));
    let id = resp.id;
    let mut state = ui.data(|d| d.get_temp::<HoldState>(id)).unwrap_or_default();
    let mut confirmed = false;
    if let Some(until) = state.done_until {
        state.needs_release = held;
        if now >= until {
            state.done_until = None;
            state.progress = 0.0;
        }
    } else if held && !state.needs_release {
        let started = *state.started.get_or_insert(now);
        state.released = None;
        state.progress = ((now - started) as f32 / hold_secs.max(0.001)).clamp(0.0, 1.0);
        if state.progress >= 1.0 {
            confirmed = true;
            state.started = None;
            state.done_until = Some(now + 1.2);
            state.needs_release = true;
        }
    } else {
        state.started = None;
        if !held {
            state.needs_release = false;
        }
        if state.progress > 0.0 {
            let (at, from) = *state.released.get_or_insert((now, state.progress));
            state.progress = from * (1.0 - ease(((now - at) / 0.2) as f32));
        }
    }
    if held || state.progress > 0.0 || state.done_until.is_some() {
        ui.ctx().request_repaint();
    }
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(t.radius_sm), t.bg_control);
    // it fills from the left as it's held: square, as everything here
    let fill = Theme::mix(t.bg_control, t.meter_hot, 0.5);
    if state.progress > 0.0 {
        let filled = Rect::from_min_size(rect.min, vec2(rect.width() * state.progress, rect.height()));
        painter.rect_filled(filled, CornerRadius::same(t.radius_sm), fill);
    }
    let done = state.done_until.is_some();
    let galley = if done { done_galley } else { galley };
    let color = if enabled { t.fg_text } else { t.fg_muted };
    let x = rect.center().x - (icon_w + galley.size().x) / 2.0;
    if let Some(icon) = icon {
        icon_at(painter, pos2(x + 7.0, rect.center().y), icon, 15.0, color);
    }
    painter.galley(pos2(x + icon_w, rect.center().y - galley.size().y / 2.0), galley, color);
    focus_ring(ui, t, &resp, t.radius_sm);
    resp.widget_info(|| WidgetInfo::selected(Role::Button, enabled, done, if done { done_label } else { label }));
    ui.data_mut(|d| d.insert_temp(id, state));
    confirmed
}

pub fn icon_button(ui: &mut Ui, t: &Theme, icon: &str, label: &str, on: bool) -> Response {
    tinted_icon_button(ui, t, icon, label, on, t.accent)
}

/// An icon button that lights up in `tint` while on (record: red).
pub fn tinted_icon_button(ui: &mut Ui, t: &Theme, icon: &str, label: &str, on: bool, tint: egui::Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 24.0), Sense::click());
    let enabled = ui.is_enabled();
    let h = if enabled { hover(ui, t, &resp) } else { 0.0 };
    // pressed: the button sinks a little
    let shape = if resp.is_pointer_button_down_on() { rect.shrink(1.5) } else { rect };
    let painter = ui.painter();
    if on {
        painter.rect_filled(shape, CornerRadius::same(t.radius_md), Theme::mix(t.bg_control, tint, 0.16 + 0.08 * h));
    } else if h > 0.0 {
        painter.rect_filled(shape, CornerRadius::same(t.radius_md), t.bg_control.gamma_multiply(h));
    }
    let color = match (enabled, on) {
        (false, _) => t.fg_muted.gamma_multiply(0.5),
        (_, true) => tint,
        _ => Theme::mix(t.fg_muted, t.fg_text, h),
    };
    icon_at(painter, shape.center(), icon, if shape == rect { 16.0 } else { 15.0 }, color);
    focus_ring(ui, t, &resp, t.radius_md);
    resp.widget_info(|| WidgetInfo::selected(Role::Button, enabled, on, label));
    resp.on_hover_text(label)
}

/// A color chip (pattern colors); click it to pick another.
pub fn swatch(ui: &mut Ui, t: &Theme, color: egui::Color32, size: f32, label: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let h = hover(ui, t, &resp);
    let painter = ui.painter();
    painter.rect_filled(rect.shrink(2.0 - 2.0 * h), CornerRadius::same(t.radius_sm), color);
    painter.rect_stroke(rect.shrink(2.0 - 2.0 * h), CornerRadius::same(t.radius_sm), Stroke::new(1.0, t.line), StrokeKind::Inside);
    focus_ring(ui, t, &resp, t.radius_sm);
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));
    resp.on_hover_text(label)
}

/// Text button with an optional leading icon, sized to its content.
pub fn button(ui: &mut Ui, t: &Theme, icon: Option<&str>, text: &str, on: bool) -> Response {
    let font = FontId::proportional(13.0);
    let text_color = if on { t.accent } else { t.fg_text };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font.clone(), text_color);
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let size = vec2(galley.size().x + icon_w + 2.0 * t.pad, 24.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    let h = if enabled { hover(ui, t, &resp) } else { 0.0 };
    let fill = if on { Theme::mix(t.bg_control, t.accent, 0.16 + 0.08 * h) } else { Theme::mix(t.bg_control, t.fg_text, 0.08 * h) };
    let painter = ui.painter();
    let shape = if resp.is_pointer_button_down_on() { rect.shrink(1.0) } else { rect };
    painter.rect_filled(shape, CornerRadius::same(t.radius_md), fill);
    let mut x = rect.left() + t.pad;
    if let Some(icon) = icon {
        icon_at(painter, pos2(x + 7.0, rect.center().y), icon, 15.0, if on { t.accent } else { t.fg_muted });
        x += icon_w;
    }
    let color = if enabled { text_color } else { t.fg_muted };
    painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, color);
    focus_ring(ui, t, &resp, t.radius_md);
    resp.widget_info(|| WidgetInfo::selected(Role::Button, enabled, on, text));
    resp
}

/// Pill navigation (React Bits' Pill Nav): the active pill slides to the picked tab.
/// `compact` shows only the icons of inactive tabs and lets the active one grow open.
/// Returns the tab the user picked this frame.
pub fn tabs<T: Copy + PartialEq>(ui: &mut Ui, t: &Theme, id: Id, current: T, items: &[(T, &str, &str)], compact: bool) -> Option<T> {
    let font = FontId::proportional(13.0);
    let mut picked = None;
    let mut slots = Vec::with_capacity(items.len());
    let pad = t.pad - 2.0;
    for (i, &(value, icon, label)) in items.iter().enumerate() {
        let full = ui.painter().layout_no_wrap(label.to_string(), font.clone(), t.fg_text).size().x + 20.0 + 2.0 * pad;
        let open = if compact { ui.ctx().animate_bool_with_time(id.with(i), value == current, t.motion) } else { 1.0 };
        let icon_only = 14.0 + 2.0 * pad;
        let w = icon_only + (full - icon_only) * open;
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
        if resp.clicked() {
            picked = Some(value);
        }
        resp.widget_info(|| WidgetInfo::selected(Role::Tab, true, value == current, label));
        let resp = resp.on_hover_text(label);
        slots.push((value, icon, label, rect, resp));
    }
    let painter = ui.painter();
    if let Some((.., rect, _)) = slots.iter().find(|s| s.0 == current) {
        let ctx = ui.ctx();
        let x = ctx.animate_value_with_time(id.with("x"), rect.left(), t.motion);
        let w = ctx.animate_value_with_time(id.with("w"), rect.width(), t.motion);
        painter.rect_filled(Rect::from_min_size(pos2(x, rect.top()), vec2(w, rect.height())), CornerRadius::same(t.radius_md), t.bg_control);
    }
    for (value, icon, label, rect, resp) in &slots {
        let active = *value == current;
        let color = if active { t.fg_text } else if resp.hovered() { Theme::mix(t.fg_muted, t.fg_text, 0.6) } else { t.fg_muted };
        let icon_color = if active { t.accent } else { color };
        icon_at(painter, pos2(rect.left() + pad + 7.0, rect.center().y), icon, 15.0, icon_color);
        if rect.width() > 30.0 {
            let label_color = color.gamma_multiply(((rect.width() - 30.0) / 30.0).min(1.0));
            painter.with_clip_rect(*rect).text(pos2(rect.left() + pad + 20.0, rect.center().y), Align2::LEFT_CENTER, *label, font.clone(), label_color);
        }
        focus_ring(ui, t, resp, t.radius_md);
    }
    picked
}

/// A segmented switch (PAT | SONG, waveform looks…). Returns the picked index.
pub fn segmented(ui: &mut Ui, t: &Theme, current: usize, labels: &[&str]) -> Option<usize> {
    let mut picked = None;
    let font = FontId::proportional(12.0);
    let widths: Vec<f32> = labels.iter().map(|l| ui.painter().layout_no_wrap(l.to_string(), font.clone(), t.fg_text).size().x + 2.0 * t.pad).collect();
    let (rect, _) = ui.allocate_exact_size(vec2(widths.iter().sum::<f32>() + 4.0, 24.0), Sense::hover());
    let id = ui.scope_id().with(("segmented", labels));
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(t.radius_md), t.bg_field);
    let mut cells = Vec::with_capacity(labels.len());
    let mut x = rect.left() + 2.0;
    for (label, w) in labels.iter().zip(&widths) {
        let cell = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(*w, rect.height() - 4.0));
        cells.push((cell, ui.interact(cell, id.with(label), Sense::click())));
        x += w;
    }
    // the lit segment slides to the new choice
    if let Some((cell, _)) = cells.get(current) {
        let cx = ui.ctx().animate_value_with_time(id.with("x"), cell.left(), t.motion);
        let cw = ui.ctx().animate_value_with_time(id.with("w"), cell.width(), t.motion);
        painter.rect_filled(Rect::from_min_size(pos2(cx, cell.top()), vec2(cw, cell.height())), CornerRadius::same(t.radius_sm), t.accent);
    }
    for (i, ((cell, resp), label)) in cells.iter().zip(labels).enumerate() {
        if resp.clicked() {
            picked = Some(i);
        }
        let on = i == current;
        if !on && resp.hovered() {
            painter.rect_filled(*cell, CornerRadius::same(t.radius_sm), t.bg_control);
        }
        let color = if on { t.bg_base } else { t.fg_text };
        painter.text(cell.center(), Align2::CENTER_CENTER, *label, font.clone(), color);
        focus_ring(ui, t, resp, t.radius_sm);
        resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, on, *label));
    }
    picked
}

// ---------------------------------------------------------------------------
// Lists
// ---------------------------------------------------------------------------

/// A full-width list row (React Bits' Animated List): hover and selection fills, focusable
/// (arrows move between rows, Enter clicks). The caller paints the content in `rect`.
pub fn row(ui: &mut Ui, t: &Theme, id: Id, selected: bool) -> Response {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), t.row_h), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    // a clicked row takes the keyboard: ↑ ↓ walk the list from there
    if resp.clicked() {
        resp.request_focus();
    }
    let k = ui.ctx().animate_bool_with_time(id.with("sel"), selected, t.motion);
    let alpha = appear(ui, t, id, rect);
    let mut painter = ui.painter().clone();
    painter.multiply_opacity(alpha);
    if resp.hovered() && !selected {
        painter.rect_filled(rect, CornerRadius::ZERO, Theme::mix(t.bg_panel, t.fg_text, 0.05));
    }
    if k > 0.0 {
        painter.rect_filled(rect, CornerRadius::ZERO, Theme::mix(t.bg_panel, t.select, 0.2 * k));
        painter.rect_filled(Rect::from_min_size(rect.min, vec2(2.0, rect.height())), CornerRadius::ZERO, t.select.gamma_multiply(k));
    }
    if resp.has_focus() {
        painter.rect_stroke(rect.shrink(0.5), CornerRadius::ZERO, Stroke::new(1.0, t.select), StrokeKind::Inside);
    }
    resp
}

/// How visible a list row is: rows that come into view (a folder opens, you scroll) fade
/// in, lower ones a beat later, so lists cascade. Painters of the row's content use it too.
fn appear(ui: &Ui, t: &Theme, id: Id, rect: Rect) -> f32 {
    if t.motion <= 0.0 || !rect.intersects(ui.clip_rect()) {
        return 1.0;
    }
    let now = ui.input(|i| i.time);
    let pass = ui.ctx().cumulative_pass_nr();
    let key = id.with("appear");
    let at = match ui.data(|d| d.get_temp::<(u64, f64)>(key)) {
        Some((seen, at)) if seen + 1 >= pass => at,
        _ => {
            // (re)appearing (not drawn last pass): start after the rows above it
            let depth = ((rect.top() - ui.clip_rect().top()) / t.row_h).clamp(0.0, 20.0) as f64;
            now + depth * 0.014
        }
    };
    ui.data_mut(|d| d.insert_temp(key, (pass, at)));
    let k = ease(((now - at) as f32 / 0.18).clamp(0.0, 1.0));
    if k < 1.0 {
        ui.ctx().request_repaint();
    }
    ui.data_mut(|d| d.insert_temp(id.with("alpha"), k));
    k
}

/// The current fade of a row drawn with [`row`], for painting its content.
pub fn row_alpha(ui: &Ui, resp: &Response) -> f32 {
    ui.data(|d| d.get_temp::<f32>(resp.id.with("alpha"))).unwrap_or(1.0)
}

// ---------------------------------------------------------------------------
// Light
// ---------------------------------------------------------------------------

/// React Bits' Spotlight Card: a soft light that follows the pointer across a card and
/// reaches cards nearby; the border catches it.
pub fn spotlight(ui: &Ui, t: &Theme, rect: Rect, radius: u8) {
    let Some(p) = ui.ctx().pointer_hover_pos() else { return };
    const REACH: f32 = 160.0;
    let distance = rect.distance_to_pos(p);
    if distance > REACH * 0.5 {
        return;
    }
    let strength = 1.0 - distance / (REACH * 0.5);
    let clip = rect.intersect(ui.clip_rect());
    let painter = ui.painter().with_clip_rect(clip);
    let center = t.fg_text.gamma_multiply(0.07 * strength);
    let mut mesh = Mesh::default();
    mesh.colored_vertex(p, center);
    const N: u32 = 24;
    for i in 0..=N {
        let a = TAU * i as f32 / N as f32;
        mesh.colored_vertex(p + vec2(a.cos(), a.sin()) * REACH, center.gamma_multiply(0.0));
    }
    for i in 0..N {
        mesh.add_triangle(0, i + 1, i + 2);
    }
    painter.add(Shape::mesh(mesh));
    ui.painter().rect_stroke(rect, CornerRadius::same(radius), Stroke::new(1.0, Theme::mix(t.line, t.fg_muted, 0.7 * strength)), StrokeKind::Inside);
}

/// Keyboard focus ring (after React Bits' Star Border): it lands with a quick glow and
/// settles to a thin line.
pub fn focus_ring(ui: &Ui, t: &Theme, resp: &Response, radius: u8) {
    let k = ui.ctx().animate_bool_with_time(resp.id.with("focus"), resp.has_focus(), t.motion);
    if k <= 0.0 {
        return;
    }
    let rect = resp.rect.expand(1.0);
    let painter = ui.painter();
    painter.rect_stroke(rect, CornerRadius::same(radius + 1), Stroke::new(1.5, t.select.gamma_multiply(k)), StrokeKind::Outside);
    let glow = rect.expand(2.0 + 3.0 * (1.0 - k));
    painter.rect_stroke(glow, CornerRadius::same(radius + 3), Stroke::new(3.0, t.select.gamma_multiply(0.15 * k)), StrokeKind::Outside);
}

// ---------------------------------------------------------------------------
// Modals and windows
// ---------------------------------------------------------------------------

/// A modal dialog: a title bar with a close button, padded content. Esc or a click outside
/// closes it. Returns the content's value and whether it should close.
pub fn modal<R>(ctx: &egui::Context, t: &Theme, id: &str, title: &str, width: f32, add: impl FnOnce(&mut Ui) -> R) -> (R, bool) {
    modal_flush(ctx, t, id, title, width, |ui| {
        Frame::new()
            .inner_margin(t.pad + 4.0)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(t.gap * 2.0, t.gap * 2.0);
                add(ui)
            })
            .inner
    })
}

/// A modal whose content runs edge to edge under the title bar (lists, with their scroll
/// bar on the right edge): the content pads itself. Returns what `add` returned and whether
/// it should close.
pub fn modal_flush<R>(ctx: &egui::Context, t: &Theme, id: &str, title: &str, width: f32, add: impl FnOnce(&mut Ui) -> R) -> (R, bool) {
    let frame = Frame::new()
        .fill(t.bg_device)
        .stroke(Stroke::new(1.0, t.line))
        .corner_radius(t.radius_md + 2)
        .shadow(ctx.global_style().visuals.window_shadow);
    let out = egui::Modal::new(Id::unique(("modal", id))).frame(frame).show(ctx, |ui| {
        ui.set_width(width);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate); // never grow past the title bar
        region(ui);
        let close = bar_with(ui, t, t.bg_device, |ui| {
            ui.label(RichText::new(title).strong().color(t.fg_text));
            right(ui, |ui| icon_button(ui, t, crate::icons::X, tr!("Close (Esc)"), false).clicked())
        });
        (add(ui), close)
    });
    let close = out.inner.1 || out.should_close();
    (out.inner.0, close)
}

/// Frame for floating editor windows: content draws its own bars, so no inner margin.
pub fn window_frame(ctx: &egui::Context, t: &Theme) -> Frame {
    Frame::new()
        .fill(t.bg_base)
        .stroke(Stroke::new(1.0, t.line))
        .corner_radius(t.radius_md + 2)
        .shadow(ctx.global_style().visuals.window_shadow)
}
