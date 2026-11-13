//! Playlist: the song timeline. Any clip on any lane (FL-style). Right-click empty space to
//! insert a pattern, drag sounds and patterns in from the browser, drag clips to move them,
//! their edges to resize or trim. Moves snap to a grid that gets finer as you zoom in
//! (Alt: free). Lanes fold away from the header column on the left.

use std::collections::BTreeSet;

use eframe::egui::{self, Align2, CornerRadius, CursorIcon, DragAndDrop, FontId, Key, Modifiers, Pos2, Rangef, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use ravune_core::{BAR, Clip, ClipFx, ClipKind, EnvTarget, PATTERN_COLORS, PPQ, STEP, Stretch, bend_spots, curve_at, curve_samples, insert_point, remove_point, sample_name};
use serde_json::json;

use crate::App;
use crate::browser::{PatternDrag, PluginDrag, SampleDrag};
use crate::shell::{Rename, Renaming};
use crate::theme::Theme;
use crate::{icons, kit, widgets};
use crate::shell::Tone;

const RULER_H: f32 = 18.0;
/// The lane header column (sticky on the left).
const HEAD_W: f32 = 112.0;
const FOLDED_H: f32 = 18.0;
const EDGE: f32 = 6.0;

/// What a click does on the playlist or in the piano roll (FL Studio's tools).
#[derive(Clone, Copy, Default, PartialEq)]
pub enum Tool {
    #[default]
    Select,
    Draw,
    Paint,
    Erase,
    Slice,
    Mute,
}

impl Tool {
    pub fn icon(self) -> &'static str {
        match self {
            Tool::Select => icons::CURSOR,
            Tool::Draw => icons::PENCIL_SIMPLE,
            Tool::Paint => icons::PAINT_BRUSH,
            Tool::Erase => icons::ERASER,
            Tool::Slice => icons::SCISSORS,
            Tool::Mute => icons::SPEAKER_SLASH,
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Tool::Select => tr!("Select: drag clips, their edges resize and trim, drag a box on empty space"),
            Tool::Draw => tr!("Draw: click empty space to put the current pattern there (and drag it into place)"),
            Tool::Paint => tr!("Paint: drag along a lane to paint copies, one after the other"),
            Tool::Erase => tr!("Erase: click or drag over things to delete them (the right button erases too in the piano roll)"),
            Tool::Slice => tr!("Slice: click to cut in two where you click"),
            Tool::Mute => tr!("Mute: click or drag over clips to mute or unmute them"),
        }
    }
}

/// The tool buttons of an editor's bar.
pub fn tool_buttons(ui: &mut Ui, t: &Theme, tool: &mut Tool, offered: &[Tool]) {
    for &k in offered {
        if kit::icon_button(ui, t, k.icon(), k.help(), *tool == k).clicked() {
            *tool = k;
        }
    }
}

#[derive(Clone)]
enum Drag {
    /// Erase or mute: the clips done so far in this stroke.
    Tool { done: BTreeSet<usize> },
    /// Paint: the lane, where the next copy goes and how long each is.
    Paint { lane: u32, next: u32, length: u32 },
    /// The selection follows the clip that was grabbed: ticks between its start and the
    /// grab point, its lane, and where every selected clip started (index, start, lane).
    Move { grabbed: usize, dt: f64, lane: u32, from: Vec<(usize, u32, u32)> },
    /// The right edge of `grabbed`: every clip in `from` (index, start, length, offset as the
    /// drag began: the selection) grows or shrinks by as much.
    Resize { grabbed: usize, from: Vec<(usize, u32, u32, u32)> },
    /// The left edge, the same way: they start later or earlier, their content stays put.
    Trim { grabbed: usize, from: Vec<(usize, u32, u32, u32)> },
    /// A box from this point (relative to the timeline), added to `keep`.
    Select { from: Vec2, keep: BTreeSet<usize> },
    /// The song position, held on the ruler: it follows the mouse until let go.
    Position,
    /// A point of a line on a clip: (clip, point, which line).
    Point(usize, usize, Line),
    /// The bend of a stretch of a line: (clip, stretch, which line, its bend and the mouse's
    /// height when it was picked up).
    Bend(usize, usize, Line, f32, f32),
}

/// What's under the mouse on a line: the line itself, one of its points, or the ring in
/// the middle of a stretch (its bend).
#[derive(Clone, Copy, PartialEq)]
enum Grip {
    Line,
    Point(usize),
    Bend(usize),
}

/// A line drawn over a clip that can be edited right there: an automation clip's, or one of
/// an audio clip's envelopes.
#[derive(Clone, Copy, PartialEq)]
enum Line {
    Auto,
    Env(EnvTarget),
}

/// A line's points and the bends between them.
type Points = (Vec<(u32, f32)>, Vec<f32>);

/// The points of `line` on clip `c`, and the bends between them.
fn line_data(c: &Clip, line: Line) -> Option<Points> {
    match (line, &c.kind) {
        (Line::Auto, ClipKind::Automation { points, curves, .. }) => Some((points.clone(), curves.clone())),
        (Line::Env(target), ClipKind::Audio { .. }) => c.fx.envelope(target).map(|e| (e.points.clone(), e.curves.clone())),
        _ => None,
    }
}

/// The operator (and its props) that gives clip `i`'s `line` these points and bends.
fn set_line(i: usize, line: Line, points: Vec<(u32, f32)>, curves: Vec<f32>) -> (&'static str, serde_json::Value) {
    match line {
        Line::Auto => ("clip.automation", json!({ "index": i, "points": points, "curves": curves })),
        Line::Env(target) => ("clip.envelope", json!({ "index": i, "target": target, "points": points, "curves": curves })),
    }
}


/// Under a clip's name strip: where its notes, sound or automation line go.
fn clip_body(r: Rect) -> Rect {
    Rect::from_min_max(pos2(r.left(), r.top() + 12.0f32.min(r.height()) + 1.0), r.max - vec2(0.0, 2.0))
}

/// An automation value (0..1) as a height in a clip's body, and back.
fn auto_y(body: Rect, v: f32) -> f32 {
    body.bottom() - 2.0 - v * (body.height() - 4.0)
}

fn auto_v(body: Rect, y: f32) -> f32 {
    ((body.bottom() - 2.0 - y) / (body.height() - 4.0).max(1.0)).clamp(0.0, 1.0)
}

#[derive(Clone, Copy, PartialEq)]
enum Edge {
    Left,
    Right,
    Body,
}

/// What was under the mouse when the context menu opened.
#[derive(Clone, Copy)]
enum Menu {
    /// A clip and the song tick under the mouse.
    Clip(usize, u32),
    /// Empty space: lane and tick.
    Empty(u32, u32),
    /// The ruler, at a tick.
    Ruler(u32),
    /// A marker, by index.
    Marker(usize),
}

/// Something from the browser on its way to the playlist.
enum Incoming {
    Sample(String),
    Pattern(usize),
    Plugin(ravune_bridge::Found),
}

pub struct PlaylistView {
    /// Pixels per beat.
    pub zoom: f32,
    /// Lane height in pixels.
    pub lane_h: f32,
    pub snap: bool,
    folded: BTreeSet<u32>,
    drag: Option<Drag>,
    pub selected: BTreeSet<usize>,
    /// The clip whose properties are open, and whether they're docked on the right.
    pub props: Option<usize>,
    pub props_docked: bool,
    /// The envelope shown in the clip properties, and the point being dragged there.
    pub env_target: EnvTarget,
    pub env_drag: Option<usize>,
    /// …or the bend being dragged there: (stretch, its bend and the mouse's height then).
    pub env_bend: Option<(usize, f32, f32)>,
    /// The clip properties change every clip of the same sound, not just the one shown
    /// (the default: copies of a sound stay alike, as in FL Studio).
    pub props_all: bool,
    /// Right-dragging a loop region on the ruler: where it began (ticks).
    loop_drag: Option<u32>,
    /// Dragging the loop with the left button: which part (0 its start, 1 its end, 2 all of
    /// it), the region and the tick it was picked up at; whether it moved.
    loop_grab: Option<(u8, u32, u32, f64)>,
    loop_moved: bool,
    /// A marker in hand: its index, where on it it was picked up (ticks), where the press
    /// began (x) and whether it moved (else the click jumps there).
    marker_grab: Option<(usize, f64, f32, bool)>,
    /// Copied clips (Ctrl+C / X), pasted with Ctrl+V.
    clipboard: Vec<ravune_core::Clip>,
    /// Scroll along with the playhead.
    pub follow: bool,
    pub tool: Tool,
    menu: Option<Menu>,
}

impl Default for PlaylistView {
    fn default() -> Self {
        Self { zoom: 18.0, lane_h: 54.0, snap: true, folded: BTreeSet::new(), drag: None, selected: BTreeSet::new(), menu: None, props: None, props_docked: true, env_target: EnvTarget::Volume, env_drag: None, env_bend: None, props_all: true, loop_drag: None, loop_grab: None, loop_moved: false, marker_grab: None, clipboard: Vec::new(), follow: true, tool: Tool::Select }
    }
}

fn snap(tick: f64, grid: u32) -> u32 {
    (tick.max(0.0) / grid as f64).round() as u32 * grid
}

/// An edge dragged on a selection: `grabbed`'s goes to `to` (snapped) and every clip's moves
/// by as much. `from`: each clip's (index, start, length, offset) when the drag began.
/// The right edge gives the new lengths, none shorter than `shortest` (or than it was).
pub(crate) fn resized(from: &[(usize, u32, u32, u32)], grabbed: usize, to: u32, shortest: u32) -> Vec<(usize, u32)> {
    let Some(&(_, start, length, _)) = from.iter().find(|f| f.0 == grabbed) else { return Vec::new() };
    let by = to.max(start + shortest) as i64 - (start + length) as i64;
    from.iter().map(|&(k, _, length, _)| (k, (length as i64 + by).max(shortest.min(length) as i64) as u32)).collect()
}

/// The left edge gives the new (start, length, offset): each clip starts later or earlier,
/// its content stays put, and it stays after its content's start and before its own end.
pub(crate) fn trimmed(from: &[(usize, u32, u32, u32)], grabbed: usize, to: u32, shortest: u32) -> Vec<(usize, u32, u32, u32)> {
    let Some(&(_, start, _, _)) = from.iter().find(|f| f.0 == grabbed) else { return Vec::new() };
    let by = to as i64 - start as i64;
    from.iter()
        .map(|&(k, start, length, offset)| {
            let (content, end) = (start as i64 - offset as i64, start + length);
            let lo = content.max(0) as u32;
            let start = ((start as i64 + by).max(lo as i64) as u32).min(end.saturating_sub(shortest.min(length)).max(lo));
            (k, start, end - start, (start as i64 - content) as u32)
        })
        .collect()
}

/// The finest grid whose cells are still 12 px wide: zoom in to snap to smaller cells.
fn grid(beat_px: f32) -> u32 {
    [STEP / 4, STEP / 2, STEP, PPQ / 2, PPQ, BAR].into_iter().find(|g| *g as f32 * beat_px / PPQ as f32 >= 12.0).unwrap_or(BAR)
}

fn grid_name(grid: u32) -> &'static str {
    match grid {
        g if g == BAR => tr!("1 bar"),
        g if g == PPQ => "1/4",
        g if g == PPQ / 2 => "1/8",
        g if g == STEP => "1/16",
        g if g == STEP / 2 => "1/32",
        _ => "1/64",
    }
}

impl App {
    pub fn playlist(&mut self, ui: &mut Ui, song_beat: Option<f64>) {
        let t = self.theme.clone();
        kit::region(ui);
        // the tool's help, in the status bar while the mouse is over the playlist
        self.zone_hint(ui, self.playlist.tool.help());
        let grid_now = grid(self.playlist.zoom);
        kit::bar(ui, &t, |ui| {
            ui.label(RichText::new(format!("{}  Playlist", icons::LIST)).strong());
            kit::vsep(ui, &t);
            tool_buttons(ui, &t, &mut self.playlist.tool, &[Tool::Select, Tool::Draw, Tool::Paint, Tool::Erase, Tool::Slice, Tool::Mute]);
            kit::vsep(ui, &t);
            // which pattern Draw and Paint put down
            let r = self.pattern_picker(ui, &t, "playlist-pattern");
            self.hint(&r, tr!("The pattern Draw and Paint put on the playlist (the rack and the piano roll show it too)."));
            kit::right(ui, |ui| {
                for (icon, factor, label) in [(icons::MAGNIFYING_GLASS_PLUS, 1.25, tr!("Zoom in (Ctrl wheel)")), (icons::MAGNIFYING_GLASS_MINUS, 0.8, tr!("Zoom out (Ctrl wheel)"))] {
                    if kit::icon_button(ui, &t, icon, label, false).clicked() {
                        self.playlist.zoom = (self.playlist.zoom * factor).clamp(4.0, 320.0);
                    }
                }
                kit::vsep(ui, &t);
                for (icon, factor, label) in [(icons::PLUS, 1.2, tr!("Taller lanes (Alt wheel)")), (icons::MINUS, 1.0 / 1.2, tr!("Shorter lanes (Alt wheel)"))] {
                    if kit::icon_button(ui, &t, icon, label, false).clicked() {
                        self.playlist.lane_h = (self.playlist.lane_h * factor).clamp(24.0, 160.0);
                    }
                }
                let all_folded = !self.playlist.folded.is_empty();
                let r = kit::icon_button(ui, &t, icons::ARROWS_IN_LINE_VERTICAL, if all_folded { tr!("Unfold every lane") } else { tr!("Fold every lane") }, all_folded);
                if r.clicked() {
                    self.fold_all_lanes(!all_folded);
                }
                let r = kit::icon_button(ui, &t, icons::ARROW_RIGHT, tr!("Follow the playhead while playing"), self.playlist.follow);
                if r.clicked() {
                    self.playlist.follow = !self.playlist.follow;
                }
                let on = self.session.loop_on;
                let r = kit::icon_button(ui, &t, icons::REPEAT, tr!("Loop on / off"), on);
                if r.clicked() {
                    match self.session.loop_region {
                        Some(_) => self.run("transport.loop", json!({ "on": !on })),
                        // nothing drawn yet: four bars from the song position
                        None => {
                            let from = self.session.position / BAR * BAR;
                            self.run("transport.loop", json!({ "from": from, "to": from + 4 * BAR }));
                        }
                    }
                }
                self.hint(&r, tr!("Loop: song mode plays only the region on the ruler, over and over. Right-drag the ruler to draw one; drag its edges or its middle to change it."));
                kit::vsep(ui, &t);
                let r = kit::button(ui, &t, Some(icons::MAGNET), grid_name(grid_now), self.playlist.snap);
                if r.clicked() {
                    self.playlist.snap = !self.playlist.snap;
                }
                self.hint(&r, tr!("Snap clips to the grid; it gets finer as you zoom in. Hold Alt to place freely."));
            });
        });
        if let Some(i) = self.playlist.props {
            self.clip_props(ui, i);
        }
        let ppt = self.playlist.zoom / PPQ as f32;
        let project = &self.session.project;
        let used = project.clips.iter().map(|c| c.lane + 1).max().unwrap_or(0);
        let lane_h = self.playlist.lane_h;
        let height_of = |lane: u32| if self.playlist.folded.contains(&lane) { FOLDED_H } else { lane_h };
        // lanes in use, two spare, then enough to fill the view
        let mut heights: Vec<f32> = (0..used.max(6) + 2).map(height_of).collect();
        while RULER_H + heights.iter().sum::<f32>() < ui.available_height() {
            heights.push(height_of(heights.len() as u32));
        }
        let width = HEAD_W + (project.song_end() + 16 * BAR) as f32 * ppt;
        let height = RULER_H + heights.iter().sum::<f32>();
        egui::ScrollArea::both().id_salt("playlist").auto_shrink(false).show(ui, |ui| {
            let size = vec2(width.max(ui.available_width()), height.max(ui.available_height()));
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
            self.playlist_body(ui, &t, rect, &resp, ppt, &heights, song_beat);
        });
    }

    fn fold_all_lanes(&mut self, fold: bool) {
        self.playlist.folded.clear();
        if fold {
            let lanes = self.session.project.clips.iter().map(|c| c.lane + 1).max().unwrap_or(0).max(6) + 2;
            self.playlist.folded.extend(0..lanes);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn playlist_body(&mut self, ui: &mut Ui, t: &Theme, rect: Rect, resp: &egui::Response, ppt: f32, heights: &[f32], song_beat: Option<f64>) {
        let visible = ui.clip_rect();
        let painter = ui.painter_at(rect);
        let lanes = heights.len() as u32;
        let tops: Vec<f32> = heights.iter().scan(0.0, |y, h| Some(std::mem::replace(y, *y + h))).collect();
        let x_of = |tick: f64| rect.left() + HEAD_W + tick as f32 * ppt;
        let tick_at = |x: f32| ((x - rect.left() - HEAD_W) / ppt).max(0.0) as f64;
        let lane_top = |lane: u32| rect.top() + RULER_H + tops.get(lane as usize).copied().unwrap_or(0.0);
        let lane_h = |lane: u32| heights.get(lane as usize).copied().unwrap_or(FOLDED_H);
        let lane_at = |y: f32| tops.iter().rposition(|top| rect.top() + RULER_H + top <= y).unwrap_or(0) as u32;
        let ruler_y = rect.top().max(visible.top());
        let head = Rect::from_min_max(pos2(visible.left(), rect.top()), pos2(visible.left() + HEAD_W, rect.bottom()));
        let in_body = |p: Pos2| rect.contains(p) && p.x > head.right() && p.y > ruler_y + RULER_H;
        let beat_px = PPQ as f32 * ppt;
        let free = ui.input(|i| i.modifiers.alt) || !self.playlist.snap;
        let g = if free { 1 } else { grid(beat_px) };

        // lanes and grid
        for lane in 0..lanes {
            let r = Rect::from_min_size(pos2(rect.left(), lane_top(lane)), vec2(rect.width(), lane_h(lane)));
            painter.rect_filled(r, CornerRadius::ZERO, if lane % 2 == 0 { t.bg_base } else { Theme::mix(t.bg_base, t.bg_panel, 0.5) });
        }
        let first = ((visible.left() - rect.left() - HEAD_W) / beat_px).floor().max(0.0) as u32;
        let last = ((visible.right() - rect.left()) / beat_px).ceil() as u32;
        // every other 4 bars a shade apart, so phrases read at a glance
        let phrase = 16 * PPQ;
        for k in (first * PPQ / phrase..=last * PPQ / phrase).filter(|k| k % 2 == 1) {
            let (a, b) = (x_of((k * phrase) as f64).max(head.right()), x_of(((k + 1) * phrase) as f64));
            if b > a {
                painter.rect_filled(Rect::from_x_y_ranges(a..=b, (rect.top() + RULER_H)..=rect.bottom()), CornerRadius::ZERO, t.fg_text.gamma_multiply(0.035));
            }
        }
        // the snap grid, fainter than beats and bars
        let fine = grid(beat_px);
        if fine < PPQ {
            for k in (first * PPQ / fine)..=(last * PPQ / fine) {
                let tick = k * fine;
                if !tick.is_multiple_of(PPQ) {
                    let x = x_of(tick as f64);
                    let color = if tick.is_multiple_of(STEP) { Theme::mix(t.bg_base, t.line, 0.22) } else { Theme::mix(t.bg_base, t.line, 0.12) };
                    painter.line_segment([pos2(x, rect.top() + RULER_H), pos2(x, rect.bottom())], Stroke::new(1.0, color));
                }
            }
        }
        for b in first..=last {
            if b % 4 != 0 && beat_px < 8.0 {
                continue;
            }
            let x = x_of((b * PPQ) as f64);
            let color = if b % 4 == 0 { t.line } else { Theme::mix(t.bg_base, t.line, 0.45) };
            painter.line_segment([pos2(x, rect.top() + RULER_H), pos2(x, rect.bottom())], Stroke::new(1.0, color));
        }
        // the part that loops: a soft wash over its lanes, brighter at the top
        if let Some((a, b)) = self.session.loop_region.filter(|_| self.session.loop_on) {
            let lanes_area = Rect::from_x_y_ranges(x_of(a as f64).max(head.right())..=x_of(b as f64), (rect.top() + RULER_H)..=rect.bottom()).intersect(visible);
            if lanes_area.width() > 0.0 {
                let mut wash = egui::Mesh::default();
                let (top, bottom) = (t.accent.gamma_multiply(0.09), t.accent.gamma_multiply(0.03));
                wash.colored_vertex(lanes_area.left_top(), top);
                wash.colored_vertex(lanes_area.right_top(), top);
                wash.colored_vertex(lanes_area.right_bottom(), bottom);
                wash.colored_vertex(lanes_area.left_bottom(), bottom);
                wash.add_triangle(0, 1, 2);
                wash.add_triangle(0, 2, 3);
                painter.add(Shape::mesh(wash));
            }
        }

        // clips
        let project = &self.session.project;
        let clip_rect = |c: &ravune_core::Clip| {
            let top = lane_top(c.lane);
            Rect::from_min_max(pos2(x_of(c.start as f64), top + 2.0), pos2(x_of(c.end() as f64), top + lane_h(c.lane) - 2.0))
        };
        let hit = |p: Pos2| {
            project.clips.iter().enumerate().rev().find_map(|(i, c)| {
                let r = clip_rect(c);
                let edge = if p.x > r.right() - EDGE {
                    Edge::Right
                } else if p.x < r.left() + EDGE && r.width() > 3.0 * EDGE {
                    Edge::Left
                } else {
                    Edge::Body
                };
                r.contains(p).then_some((i, edge))
            })
        };
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        for (i, clip) in project.clips.iter().enumerate() {
            let r = clip_rect(clip);
            if !r.intersects(visible) {
                continue;
            }
            let auto_name;
            let (tint, name) = match &clip.kind {
                ClipKind::Pattern { pattern } => {
                    let p = project.patterns.get(*pattern);
                    (Theme::pattern(*pattern, p.and_then(|p| p.color)), p.map_or("?", |p| p.name.as_str()))
                }
                ClipKind::Audio { sample } if clip.fx.stretch != Stretch::Off => {
                    auto_name = format!("{} · {} {:.0}", sample_name(sample), crate::i18n::t(clip.fx.stretch.name()).to_lowercase(), clip.fx.bpm);
                    (t.select, auto_name.as_str())
                }
                ClipKind::Audio { sample } => (t.select, sample_name(sample)),
                ClipKind::Automation { target, .. } => {
                    auto_name = format!("{}  {}", icons::CHART_LINE, crate::mixer::target_name(project, *target));
                    (t.accent, auto_name.as_str())
                }
            };
            let tint = if clip.fx.mute || !project.lane_plays(clip.lane) { t.fg_muted.gamma_multiply(0.6) } else { tint };
            let lifted = matches!(&self.playlist.drag, Some(Drag::Move { from, .. }) if from.iter().any(|f| f.0 == i));
            if resp.is_pointer_button_down_on() && lifted {
                // picked up: the clip lifts off the lane
                let shadow = egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: t.bg_field.gamma_multiply(0.9) };
                painter.add(shadow.as_shape(r, CornerRadius::same(t.radius_sm)));
            }
            // see-through: the lanes and the grid show under the clip, a little more when it's
            // selected or under the mouse; the content carries it
            let selected = self.playlist.selected.contains(&i);
            let near = self.playlist.drag.is_none() && pointer.is_some_and(|p| r.contains(p));
            let radius = CornerRadius::same(t.radius_md);
            painter.rect_filled(r, radius, tint.gamma_multiply(if selected { 0.26 } else if near { 0.19 } else { 0.13 }));
            // playing: the clip under the playhead lights up, pulsing with the beat
            let sounding = song_beat.map(|b| b * PPQ as f64).is_some_and(|tick| clip.start as f64 <= tick && tick < clip.end() as f64) && !clip.fx.mute && project.lane_plays(clip.lane);
            if sounding {
                let pulse = if self.settings.reduce_motion { 0.0 } else { (1.0 - song_beat.unwrap_or(0.0).fract() as f32).powi(2) };
                painter.rect_filled(r, radius, tint.gamma_multiply(0.08 + 0.1 * pulse));
                painter.rect_stroke(r, radius, Stroke::new(1.0, tint.gamma_multiply(0.35 + 0.4 * pulse)), StrokeKind::Inside);
            }
            // a solid name strip in the clip's color, its name in whatever reads on it
            let head_h = 12.0f32.min(r.height());
            let label = Rect::from_min_max(r.min, pos2(r.right(), r.top() + head_h));
            let head = if selected { Theme::mix(tint, t.fg_text, 0.2) } else { Theme::mix(t.bg_base, tint, if near { 0.85 } else { 0.72 }) };
            painter.rect_filled(label, CornerRadius { nw: t.radius_md, ne: t.radius_md, sw: 0, se: 0 }, head);
            let [hr, hg, hb, _] = head.to_array();
            let ink = if 0.299 * hr as f32 + 0.587 * hg as f32 + 0.114 * hb as f32 > 150.0 { t.bg_base } else { t.fg_text };
            let kind = match &clip.kind {
                ClipKind::Pattern { .. } => icons::SQUARES_FOUR,
                ClipKind::Audio { .. } => icons::WAVEFORM,
                ClipKind::Automation { .. } => "",
            };
            let text = if kind.is_empty() { name.to_string() } else { format!("{kind} {name}") };
            let (_, cut) = kit::fit_text(&painter.with_clip_rect(label.intersect(visible)), label.left_center() + vec2(4.0, 0.0), Align2::LEFT_CENTER, text.as_str(), FontId::proportional(10.0), ink, label.width() - 8.0);
            if cut && pointer.is_some_and(|p| label.contains(p)) && self.playlist.drag.is_none() {
                resp.clone().on_hover_text(text);
            }
            let body = clip_body(r);
            if body.height() >= 4.0 {
                let content = painter.with_clip_rect(body.intersect(visible));
                match &clip.kind {
                    ClipKind::Pattern { pattern } => {
                        let Some(pat) = project.patterns.get(*pattern) else { continue };
                        let rows: Vec<u8> = project.channels.iter().map(|c| c.slot).filter(|s| !pat.notes(*s).is_empty()).collect();
                        let row_h = body.height() / rows.len().max(1) as f32;
                        let length = pat.ticks().max(1) as i64;
                        let mut base = clip.start as i64 - clip.offset as i64;
                        while base < clip.end() as i64 {
                            if x_of((base + length) as f64) >= visible.left() && x_of(base as f64) <= visible.right() {
                                for (k, slot) in rows.iter().enumerate() {
                                    let band = Rangef::new(body.top() + k as f32 * row_h, body.top() + (k + 1) as f32 * row_h);
                                    let notes = pat.notes(*slot);
                                    let steps = pat.is_steps(*slot);
                                    let lo = notes.iter().map(|n| n.key).min().unwrap_or(60);
                                    let span = (notes.iter().map(|n| n.key).max().unwrap_or(60) - lo) as f32 + 1.0;
                                    for n in notes.iter().filter(|n| (n.start as i64) < length) {
                                        let x = x_of((base + n.start as i64) as f64);
                                        let (w, y) = if steps {
                                            ((STEP as f32 * ppt - 1.0).clamp(1.5, 5.0), band.center())
                                        } else {
                                            ((n.length as f32 * ppt - 1.0).max(1.5), band.max - ((n.key - lo) as f32 + 0.5) / span * band.span())
                                        };
                                        let h = (band.span() / if steps { 2.0 } else { span }).clamp(1.0, 3.0);
                                        content.rect_filled(Rect::from_min_size(pos2(x, y - h / 2.0), vec2(w, h)), CornerRadius::same(1), Theme::mix(tint, t.fg_text, 0.2));
                                    }
                                }
                                // the knobs moving inside it, along with it
                                for line in &pat.automation {
                                    let pts = curve_samples(&line.points, &line.curves, 0.0, length as f64);
                                    let shape = pts.into_iter().map(|(tk, v)| pos2(x_of(base as f64 + tk), auto_y(body, v))).collect();
                                    content.add(Shape::line(shape, Stroke::new(1.0, t.accent.gamma_multiply(0.6))));
                                }
                                // where the pattern starts over
                                if base > clip.start as i64 {
                                    let x = x_of(base as f64);
                                    content.line_segment([pos2(x, body.top()), pos2(x, body.bottom())], Stroke::new(1.0, tint.gamma_multiply(0.35)));
                                }
                            }
                            base += length;
                        }
                    }
                    ClipKind::Audio { sample } => {
                        if let Ok(s) = self.library.get(sample) {
                            let ticks = sound_ticks(s.seconds(), &clip.fx, self.session.project.tempo);
                            let x0 = x_of(clip.start as f64 - clip.offset as f64);
                            let span = Rangef::new(x0, x0 + ticks as f32 * ppt);
                            widgets::waveform(&content, t, self.settings.wave, &s.overview, span, body.shrink2(vec2(0.0, 1.0)));
                        }
                        // fades as ramps in the corners, envelopes as lines over the sound
                        let y_of = |v: f32| auto_y(body, v);
                        let fade = Stroke::new(1.0, t.fg_text.gamma_multiply(0.7));
                        if clip.fx.fade_in > 0 {
                            content.line_segment([pos2(r.left(), body.bottom()), pos2(x_of((clip.start + clip.fx.fade_in) as f64), body.top())], fade);
                        }
                        if clip.fx.fade_out > 0 {
                            content.line_segment([pos2(x_of(clip.end().saturating_sub(clip.fx.fade_out) as f64), body.top()), pos2(r.right(), body.bottom())], fade);
                        }
                        for env in clip.fx.envelopes.iter().filter(|e| e.shown) {
                            let at = |tick: f64| x_of(clip.start as f64 + tick - clip.offset as f64);
                            let (from, to) = (clip.offset, clip.offset + clip.length);
                            let line = curve_samples(&env.points, &env.curves, from as f64, to as f64).into_iter().map(|(tk, v)| pos2(at(tk), y_of(v))).collect();
                            let color = crate::clipprops::envelope_color(t, env.target);
                            content.add(Shape::line(line, Stroke::new(1.2, color)));
                            // its points and bends, to drag, while the clip is under the mouse or selected
                            let hovered = pointer.filter(|p| body.contains(*p));
                            if hovered.is_some() || self.playlist.selected.contains(&i) {
                                for (k, mid, v) in bend_spots(&env.points, &env.curves).into_iter().filter(|(_, mid, _)| (from as f64..=to as f64).contains(mid)) {
                                    let spot = pos2(at(mid), y_of(v));
                                    let hot = hovered.is_some_and(|p| p.distance(spot) < 7.0) || matches!(self.playlist.drag, Some(Drag::Bend(c, n, Line::Env(e), ..)) if c == i && n == k && e == env.target);
                                    kit::bend_handle(&content, spot, color, hot);
                                }
                                for (k, (tk, v)) in env.points.iter().enumerate().filter(|(_, (tk, _))| (from..=to).contains(tk)) {
                                    let spot = pos2(at(*tk as f64), y_of(*v));
                                    let big = hovered.is_some_and(|p| p.distance(spot) < 7.0) || matches!(self.playlist.drag, Some(Drag::Point(c, n, Line::Env(e))) if c == i && n == k && e == env.target);
                                    content.circle_filled(spot, if big { 4.0 } else { 2.5 }, color);
                                }
                            }
                        }
                    }
                    ClipKind::Automation { target, points, curves } => {
                        // a grid to place points against, the line, the area under it, its points and bends
                        let (from, to) = (clip.offset as f64, (clip.offset + clip.length) as f64);
                        let at = |tick: f64| x_of(clip.start as f64 + tick - from);
                        kit::quarter_lines(&content, r.x_range(), |v| auto_y(body, v), tint.gamma_multiply(0.5));
                        let line: Vec<Pos2> = curve_samples(points, curves, from, to).into_iter().map(|(tk, v)| pos2(at(tk), auto_y(body, v))).collect();
                        let inside: Vec<(usize, Pos2)> = points
                            .iter()
                            .enumerate()
                            .filter(|(_, (tk, _))| (*tk as f64) >= from && (*tk as f64) <= to)
                            .map(|(k, (tk, v))| (k, pos2(at(*tk as f64), auto_y(body, *v))))
                            .collect();
                        let mut area = egui::Mesh::default();
                        let fill = tint.gamma_multiply(0.18);
                        for w in line.windows(2) {
                            let k = area.vertices.len() as u32;
                            for p in [w[0], w[1], pos2(w[1].x, body.bottom()), pos2(w[0].x, body.bottom())] {
                                area.colored_vertex(p, fill);
                            }
                            area.add_triangle(k, k + 1, k + 2);
                            area.add_triangle(k, k + 2, k + 3);
                        }
                        content.add(Shape::mesh(area));
                        content.add(Shape::line(line, Stroke::new(1.5, tint)));
                        let hovered = pointer.filter(|p| body.contains(*p));
                        if hovered.is_some() || selected {
                            for (k, mid, v) in bend_spots(points, curves).into_iter().filter(|(_, mid, _)| (from..=to).contains(mid)) {
                                let spot = pos2(at(mid), auto_y(body, v));
                                let hot = hovered.is_some_and(|p| p.distance(spot) < 7.0) || matches!(self.playlist.drag, Some(Drag::Bend(c, n, Line::Auto, ..)) if c == i && n == k);
                                kit::bend_handle(&content, spot, tint, hot);
                            }
                        }
                        for (k, at) in &inside {
                            let big = hovered.is_some_and(|p| p.distance(*at) < 7.0) || matches!(self.playlist.drag, Some(Drag::Point(c, n, Line::Auto)) if c == i && n == *k);
                            content.circle_filled(*at, if big { 4.5 } else { 3.0 }, tint);
                        }
                        // what it's set to where the mouse is
                        if let Some(p) = hovered {
                            let near = inside.iter().find(|(_, at)| p.distance(*at) < 7.0).map(|(k, _)| points[*k].1);
                            let v = near.or_else(|| curve_at(points, curves, from + ((p.x - r.left()) / ppt) as f64)).unwrap_or(0.5);
                            content.text(body.left_top() + vec2(4.0, 2.0), Align2::LEFT_TOP, crate::mixer::describe_target(project, *target, v), FontId::monospace(10.0), t.fg_text);
                        }
                    }
                }
            }
            // its edge, in its own color: darker at rest, lighter (no white frame) when selected
            let edge = if selected { Stroke::new(1.5, Theme::mix(tint, t.fg_text, 0.45)) } else { Stroke::new(1.0, Theme::mix(t.bg_base, tint, 0.5)) };
            painter.rect_stroke(r, radius, edge, StrokeKind::Inside);
            // the hand says it can be picked up; the edges resize and trim
            if let Some(p) = pointer.filter(|p| r.contains(*p) && self.playlist.drag.is_none()) {
                let edge = p.x > r.right() - EDGE || p.x < r.left() + EDGE && r.width() > 3.0 * EDGE;
                let on_line = matches!(clip.kind, ClipKind::Automation { .. }) && p.y >= body.top();
                ui.ctx().set_cursor_icon(if edge {
                    CursorIcon::ResizeHorizontal
                } else if on_line {
                    CursorIcon::Crosshair
                } else {
                    CursorIcon::Grab
                });
            }
        }

        // what's under the mouse and where every clip is, computed before any edit so the
        // project isn't borrowed below
        let under = pointer.and_then(hit);
        let rects: Vec<Rect> = project.clips.iter().map(clip_rect).collect();
        // on a line: anywhere in an automation clip's body, near an audio clip's envelope;
        // the clip, the line, and the point under the mouse if any
        let on_line = pointer.zip(under).and_then(|(p, (i, edge))| {
            let c = &project.clips[i];
            let body = clip_body(rects[i]);
            if edge != Edge::Body || p.y < body.top() {
                return None;
            }
            let at = |tick: f64, v: f32| pos2(x_of(c.start as f64 + tick - c.offset as f64), auto_y(body, v));
            let inside = |tick: f64| (c.offset as f64..=(c.offset + c.length) as f64).contains(&tick);
            let grip = |points: &[(u32, f32)], curves: &[f32]| {
                let point = points.iter().position(|pt| inside(pt.0 as f64) && at(pt.0 as f64, pt.1).distance(p) < 7.0).map(Grip::Point);
                point.or_else(|| bend_spots(points, curves).into_iter().find(|(_, mid, v)| inside(*mid) && at(*mid, *v).distance(p) < 7.0).map(|(k, ..)| Grip::Bend(k)))
            };
            match &c.kind {
                ClipKind::Automation { points, curves, .. } => Some((i, Line::Auto, grip(points, curves).unwrap_or(Grip::Line))),
                ClipKind::Audio { .. } => {
                    let tick = c.offset as f64 + tick_at(p.x) - c.start as f64;
                    c.fx.envelopes.iter().filter(|e| e.shown).find_map(|e| match grip(&e.points, &e.curves) {
                        Some(g) => Some((i, Line::Env(e.target), g)),
                        None => ((auto_y(body, e.at(tick)) - p.y).abs() < 5.0).then_some((i, Line::Env(e.target), Grip::Line)),
                    })
                }
                ClipKind::Pattern { .. } => None,
            }
        });
        if let Some((.., grip)) = on_line.filter(|_| self.playlist.drag.is_none() && matches!(self.playlist.tool, Tool::Select | Tool::Draw)) {
            ui.ctx().set_cursor_icon(match grip {
                Grip::Bend(_) => CursorIcon::ResizeVertical,
                Grip::Point(_) => CursorIcon::Grab,
                Grip::Line => CursorIcon::Crosshair,
            });
        }

        // files from the OS: highlight while they hover, place them where they land
        if ui.input(|i| !i.raw.hovered_files.is_empty()) {
            painter.rect_stroke(visible.shrink(1.0), CornerRadius::same(t.radius_md), Stroke::new(2.0, t.select), StrokeKind::Inside);
        }
        if let Some(p) = self.drop_at.filter(|p| visible.contains(*p))
            && !self.dropped.is_empty()
        {
            self.drop_on_playlist(Some((lane_at(p.y), snap(tick_at(p.x), g))));
        }

        // sounds and patterns dragged in from the browser: show where they'll land
        let incoming = if let Some(s) = DragAndDrop::payload::<SampleDrag>(ui.ctx()) {
            Some(Incoming::Sample(s.0.clone()))
        } else if let Some(p) = DragAndDrop::payload::<PluginDrag>(ui.ctx()) {
            Some(Incoming::Plugin(p.0.clone()))
        } else {
            DragAndDrop::payload::<PatternDrag>(ui.ctx()).map(|p| Incoming::Pattern(p.0))
        };
        if let Some(what) = incoming
            && let Some(p) = ui.input(|i| i.pointer.hover_pos()).filter(|p| in_body(*p) && visible.contains(*p))
        {
            let (lane, start) = (lane_at(p.y), snap(tick_at(p.x), g));
            let project = &self.session.project;
            let (length, name, tint) = match &what {
                Incoming::Sample(path) => {
                    let ticks = self.library.get(path).map_or(PPQ as f64, |s| s.seconds() as f64 / tick_seconds(project.tempo));
                    (ticks.max(1.0) as u32, sample_name(path).to_string(), t.select)
                }
                Incoming::Pattern(i) => match project.patterns.get(*i) {
                    Some(pat) => (pat.ticks(), pat.name.clone(), Theme::pattern(*i, pat.color)),
                    None => (BAR, "?".into(), t.accent),
                },
                Incoming::Plugin(f) => (BAR, format!("{}  {}", icons::PLUS, tr!("{} · new pattern", f.name)), t.select),
            };
            // an instrument dropped on a pattern clip joins that pattern
            let onto = rects.iter().rposition(|r| r.contains(p)).filter(|i| matches!(self.session.project.clips[*i].kind, ClipKind::Pattern { .. }));
            let target = match (&what, onto) {
                (Incoming::Plugin(_), Some(i)) => Some(rects[i]),
                _ => None,
            };
            let effect = matches!(&what, Incoming::Plugin(f) if !f.instrument);
            if let Some(r) = target {
                painter.rect_stroke(r.expand(1.0), CornerRadius::same(t.radius_sm), Stroke::new(2.0, t.select), StrokeKind::Outside);
            } else if !effect {
                let top = lane_top(lane);
                let band = Rect::from_min_size(pos2(visible.left(), top), vec2(visible.width(), lane_h(lane)));
                painter.rect_filled(band, CornerRadius::ZERO, tint.gamma_multiply(0.05));
                let ghost = Rect::from_min_max(pos2(x_of(start as f64), top + 2.0), pos2(x_of((start + length) as f64), top + lane_h(lane) - 2.0));
                painter.rect_filled(ghost, CornerRadius::same(t.radius_sm), tint.gamma_multiply(0.3));
                painter.rect_stroke(ghost, CornerRadius::same(t.radius_sm), Stroke::new(1.5, tint), StrokeKind::Inside);
                painter.with_clip_rect(ghost).text(ghost.left_top() + vec2(4.0, 2.0), Align2::LEFT_TOP, name, FontId::proportional(10.0), t.fg_text);
            }
            if ui.input(|i| i.pointer.any_released()) {
                match what {
                    Incoming::Plugin(f) => {
                        DragAndDrop::take_payload::<PluginDrag>(ui.ctx());
                        if !f.instrument {
                            self.say_as(Tone::Error, tr!("Effects go on a mixer track: drop it on the rack, or add it from the mixer"));
                        } else if let Some(i) = onto {
                            // into that pattern: a new channel, and the pattern opens to write its notes
                            let ClipKind::Pattern { pattern } = self.session.project.clips[i].kind else { unreachable!() };
                            self.add_plugin(&f);
                            self.run("pattern.select", json!({ "pattern": pattern }));
                            self.tab = crate::Tab::Rack;
                        } else {
                            // empty space: a channel and a pattern named after it, placed here
                            self.add_plugin(&f);
                            self.run("pattern.add", json!({ "name": f.name }));
                            self.run("clip.add", json!({ "lane": lane, "start": start }));
                            self.playlist.selected = BTreeSet::from([self.session.project.clips.len().saturating_sub(1)]);
                            self.tab = crate::Tab::Rack;
                        }
                        self.spark(ui, tint);
                        return;
                    }
                    Incoming::Sample(path) => {
                        DragAndDrop::take_payload::<SampleDrag>(ui.ctx());
                        self.add_audio_clip(&path, lane, start);
                    }
                    Incoming::Pattern(i) => {
                        DragAndDrop::take_payload::<PatternDrag>(ui.ctx());
                        self.run("clip.add", json!({ "lane": lane, "start": start, "pattern": i }));
                    }
                }
                self.spark(ui, tint);
                self.playlist.selected = BTreeSet::from([self.session.project.clips.len().saturating_sub(1)]);
            }
        }

        // ruler (sticky on top), start marker, playhead
        let ruler = Rect::from_min_size(pos2(rect.left(), ruler_y), vec2(rect.width(), RULER_H));
        painter.rect_filled(ruler, CornerRadius::ZERO, t.bg_panel);
        // bars always (zoomed far out, every 2nd, 4th, 8th… so the numbers never run into each
        // other); beats as 4.2 / 4.3 / 4.4 once there's room, 16ths as 4.2.3 when zoomed right in
        let every = [1u32, 2, 4, 8, 16, 32].into_iter().find(|k| beat_px * 4.0 * *k as f32 >= 30.0).unwrap_or(64);
        for bar in (first / 4..=last / 4).filter(|bar| bar % every == 0) {
            let x = x_of((bar * BAR) as f64);
            painter.text(pos2(x + 3.0, ruler_y + 3.0), Align2::LEFT_TOP, (bar + 1).to_string(), Theme::mono(10.0), t.fg_text);
        }
        if beat_px >= 34.0 {
            for b in (first..=last).filter(|b| b % 4 != 0) {
                let x = x_of((b * PPQ) as f64);
                painter.text(pos2(x + 3.0, ruler_y + 4.0), Align2::LEFT_TOP, format!("{}.{}", b / 4 + 1, b % 4 + 1), Theme::mono(9.0), t.fg_muted);
            }
        }
        if beat_px >= 160.0 {
            for s in (first * 4..=last * 4).filter(|s| s % 4 != 0) {
                let x = x_of((s * STEP) as f64);
                let label = format!("{}.{}.{}", s / 16 + 1, s / 4 % 4 + 1, s % 4 + 1);
                painter.text(pos2(x + 3.0, ruler_y + 5.0), Align2::LEFT_TOP, label, Theme::mono(8.0), Theme::mix(t.fg_muted, t.bg_panel, 0.3));
            }
        }
        // the loop region: a band on the ruler, its edges down the lanes
        if let Some((a, b)) = self.session.loop_region {
            let (xa, xb) = (x_of(a as f64), x_of(b as f64));
            let on = self.session.loop_on;
            let color = if on { t.accent } else { t.fg_muted };
            let band = Rect::from_x_y_ranges(xa..=xb, ruler.y_range()).intersect(ruler).intersect(Rect::everything_right_of(head.right()));
            if band.width() > 0.0 {
                let mut fill = egui::Mesh::default();
                let (top, bottom) = (color.gamma_multiply(if on { 0.38 } else { 0.18 }), color.gamma_multiply(if on { 0.16 } else { 0.08 }));
                fill.colored_vertex(band.left_top(), top);
                fill.colored_vertex(band.right_top(), top);
                fill.colored_vertex(band.right_bottom(), bottom);
                fill.colored_vertex(band.left_bottom(), bottom);
                fill.add_triangle(0, 1, 2);
                fill.add_triangle(0, 2, 3);
                painter.add(Shape::mesh(fill));
                painter.hline(band.x_range(), band.top() + 1.0, Stroke::new(2.0, color));
            }
            // its edges: brackets to grab, a thin line down the lanes while it's on
            for (x, dir) in [(xa, 1.0), (xb, -1.0)].into_iter().filter(|(x, _)| *x > head.right()) {
                if on {
                    painter.line_segment([pos2(x, ruler_y + RULER_H), pos2(x, rect.bottom())], Stroke::new(1.0, color.gamma_multiply(0.4)));
                }
                let bracket = vec![pos2(x + 5.0 * dir, ruler_y + 1.0), pos2(x, ruler_y + 1.0), pos2(x, ruler_y + RULER_H - 1.0), pos2(x + 5.0 * dir, ruler_y + RULER_H - 1.0)];
                painter.add(Shape::line(bracket, Stroke::new(2.0, color)));
            }
        }
        // markers: a flag chip on the ruler, a faint line down the lanes
        let mut chips: Vec<(usize, Rect)> = Vec::new();
        for (k, m) in self.session.project.markers.iter().enumerate() {
            let x = x_of(m.tick as f64);
            if x < head.right() || x > visible.right() {
                continue;
            }
            let galley = painter.layout_no_wrap(format!("{} {}", icons::FLAG, m.name), FontId::proportional(10.0), t.bg_base);
            let chip = Rect::from_min_size(pos2(x, ruler_y + 2.0), vec2(galley.size().x + 8.0, RULER_H - 4.0));
            let held = self.playlist.marker_grab.is_some_and(|g| g.0 == k);
            painter.line_segment([pos2(x, ruler_y + RULER_H), pos2(x, rect.bottom())], Stroke::new(1.0, t.select.gamma_multiply(if held { 0.7 } else { 0.3 })));
            painter.rect_filled(chip, CornerRadius { nw: 0, ne: 3, sw: 0, se: 3 }, if held { Theme::mix(t.select, t.fg_text, 0.3) } else { t.select });
            painter.galley(pos2(chip.left() + 4.0, chip.center().y - galley.size().y / 2.0), galley, t.bg_base);
            chips.push((k, chip));
        }
        let on_chip = |p: Pos2| chips.iter().find(|(_, r)| r.contains(p)).map(|(k, _)| *k);
        let sx = x_of(self.session.position as f64);
        let marker = vec![pos2(sx - 5.0, ruler_y + RULER_H - 7.0), pos2(sx + 5.0, ruler_y + RULER_H - 7.0), pos2(sx, ruler_y + RULER_H)];
        painter.add(Shape::convex_polygon(marker, t.accent, Stroke::NONE));
        if let Some(b) = song_beat {
            let x = x_of(b * PPQ as f64);
            if self.playlist.follow && self.playlist.drag.is_none() && (x > visible.right() - 24.0 || x < head.right()) {
                ui.scroll_with_delta(vec2(head.right() + 48.0 - x, 0.0));
            }
            for (w, a) in [(9.0, 0.06), (4.0, 0.14)] {
                painter.line_segment([pos2(x, ruler_y), pos2(x, rect.bottom())], Stroke::new(w, t.accent.gamma_multiply(a)));
            }
            painter.line_segment([pos2(x, ruler_y), pos2(x, rect.bottom())], Stroke::new(1.5, t.fg_text));
        }

        // lane headers (sticky on the left): fold caret, color, name, mute, solo; right-click
        // for the rest. The corner folds them all.
        painter.rect_filled(head, CornerRadius::ZERO, t.bg_panel);
        painter.line_segment([head.right_top(), head.right_bottom()], Stroke::new(1.0, t.line));
        for lane in 0..lanes {
            let r = Rect::from_min_size(pos2(head.left(), lane_top(lane)), vec2(HEAD_W, lane_h(lane)));
            if !r.intersects(visible) {
                continue;
            }
            let info = self.session.project.lane(lane);
            let plays = self.session.project.lane_plays(lane);
            let folded = self.playlist.folded.contains(&lane);
            let hit = ui.interact(r, ui.scope_id().with(("lane-head", lane)), Sense::click());
            if hit.hovered() {
                painter.rect_filled(r, CornerRadius::ZERO, Theme::mix(t.bg_panel, t.fg_text, 0.05));
            }
            if let Some(c) = info.color {
                painter.rect_filled(Rect::from_min_size(r.min, vec2(3.0, r.height())), CornerRadius::ZERO, Theme::pattern(0, Some(c)));
            }
            painter.line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, Theme::mix(t.bg_panel, t.line, 0.6)));
            let y = r.top() + FOLDED_H / 2.0;
            painter.text(pos2(r.left() + 11.0, y), Align2::CENTER_CENTER, if folded { icons::CARET_RIGHT } else { icons::CARET_DOWN }, FontId::proportional(11.0), t.fg_muted);
            let name = if info.name.is_empty() { format!("{}", lane + 1) } else { format!("{} {}", lane + 1, info.name) };
            let name_rect = Rect::from_min_max(pos2(r.left() + 20.0, r.top()), pos2(r.right() - 38.0, r.top() + FOLDED_H));
            if kit::fit_text(&painter, pos2(name_rect.left(), y), Align2::LEFT_CENTER, name.as_str(), Theme::mono(10.0), if plays { t.fg_text } else { t.fg_muted }, name_rect.width()).1 {
                hit.clone().on_hover_text(name.as_str());
            }
            // M and S, like a mixer
            for (k, (label, on, key, lit)) in [("M", info.mute, "mute", t.meter_warn), ("S", info.solo, "solo", t.select)].into_iter().enumerate() {
                let br = kit::chip(ui, &painter, t, pos2(r.right() - 28.0 + k as f32 * 16.0, y), label, on, lit, ui.scope_id().with(("lane-ms", lane, k)));
                if br.clicked() {
                    self.run("lane.set", json!({ "lane": lane, key: !on }));
                }
                self.hint(&br, if k == 0 { tr!("Mute this lane") } else { tr!("Solo this lane (only soloed lanes play)") });
            }
            if hit.clicked() && !self.playlist.folded.remove(&lane) {
                self.playlist.folded.insert(lane);
            }
            if hit.double_clicked() {
                self.rename = Some(Renaming::new(Rename::Lane(lane), info.name.clone()));
            }
            hit.context_menu(|ui| self.lane_menu(ui, lane));
            self.hint(&hit, tr!("Lane: click to fold · double-click to rename · right-click for color, insert, delete, select"));
        }
        let corner = Rect::from_min_size(pos2(head.left(), ruler_y), vec2(HEAD_W, RULER_H));
        painter.rect_filled(corner, CornerRadius::ZERO, t.bg_panel);
        let any_folded = !self.playlist.folded.is_empty();
        painter.text(corner.center(), Align2::CENTER_CENTER, if any_folded { icons::CARET_RIGHT } else { icons::CARET_DOWN }, FontId::proportional(11.0), t.fg_muted);
        let r = ui.interact(corner, ui.scope_id().with("lane-fold-all"), Sense::click());
        if r.clicked() {
            self.fold_all_lanes(!any_folded);
        }
        self.hint(&r, if any_folded { tr!("Unfold every lane") } else { tr!("Fold every lane") });

        // the tool next to the pointer, and for the scissors where the cut would go
        if let Some(p) = ui.input(|i| i.pointer.hover_pos()).filter(|p| in_body(*p) && visible.contains(*p))
            && self.playlist.tool != Tool::Select
        {
            if self.playlist.tool == Tool::Slice
                && let Some((i, _)) = under
            {
                let x = x_of(snap(tick_at(p.x), g.max(1)) as f64);
                let r = rects[i];
                painter.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], Stroke::new(1.5, t.fg_text));
            }
            ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("playlist-tool"))).text(p + vec2(12.0, 12.0), Align2::LEFT_TOP, self.playlist.tool.icon(), FontId::proportional(15.0), t.fg_text);
        }

        // ---- interaction ----
        // hold Space and drag: hand tool, scrolls both ways (clips stay put)
        let panning = ui.input(|i| i.key_down(Key::Space)) && !ui.ctx().text_edit_focused() && resp.hovered();
        if panning {
            ui.ctx().set_cursor_icon(if resp.dragged() { CursorIcon::Grabbing } else { CursorIcon::Grab });
            if resp.dragged() {
                ui.scroll_with_delta(resp.drag_delta());
                self.space_panned = true;
            }
        }
        let mut no_menu = false;
        if let Some(p) = pointer.filter(|_| !panning) {
            let adding = ui.input(|i| i.modifiers.command || i.modifiers.shift);
            if resp.hovered() && ui.input(|i| i.pointer.primary_pressed()) {
                self.session.close_merge();
                let on_loop = self.session.loop_region.filter(|_| p.y < ruler_y + RULER_H && p.x > head.right()).and_then(|(a, b)| {
                    let (xa, xb) = (x_of(a as f64), x_of(b as f64));
                    let part = if (p.x - xa).abs() <= 5.0 {
                        0
                    } else if (p.x - xb).abs() <= 5.0 {
                        1
                    } else if p.x > xa && p.x < xb {
                        2
                    } else {
                        return None;
                    };
                    Some((part, a, b))
                });
                if let Some(k) = on_chip(p) {
                    // a marker: drag it along, or click it to jump there
                    let tick = self.session.project.markers[k].tick as f64;
                    self.playlist.marker_grab = Some((k, tick_at(p.x) - tick, p.x, false));
                } else if let Some((part, a, b)) = on_loop {
                    // the loop: its edges stretch it, its middle moves it (a click there still sets the position)
                    self.playlist.loop_grab = Some((part, a, b, tick_at(p.x)));
                    self.playlist.loop_moved = false;
                } else if p.y < ruler_y + RULER_H && p.x > head.right() {
                    self.run("transport.set_position", json!({ "tick": snap(tick_at(p.x), g) }));
                    self.playlist.drag = Some(Drag::Position);
                } else if !in_body(p) {
                    // the lane headers handle themselves
                } else if matches!(self.playlist.tool, Tool::Erase | Tool::Mute) {
                    self.playlist.drag = Some(Drag::Tool { done: BTreeSet::new() });
                    self.tool_stroke(under.map(|(i, _)| i));
                } else if self.playlist.tool == Tool::Slice {
                    if let Some((i, _)) = under
                        && let Some(c) = self.session.project.clips.get(i)
                    {
                        let at = snap(tick_at(p.x), g.max(1));
                        if at > c.start && at < c.end() {
                            self.run("clip.split", json!({ "index": i, "at": at }));
                        }
                    }
                } else if let Some((i, line, grip)) = on_line.filter(|_| !adding && matches!(self.playlist.tool, Tool::Select | Tool::Draw)) {
                    self.playlist.selected = BTreeSet::from([i]);
                    self.playlist.drag = match grip {
                        Grip::Point(k) => Some(Drag::Point(i, k, line)),
                        Grip::Bend(k) => {
                            let bent = self.session.project.clips.get(i).and_then(|c| line_data(c, line)).and_then(|(_, curves)| curves.get(k).copied()).unwrap_or(0.0);
                            Some(Drag::Bend(i, k, line, bent, p.y))
                        }
                        Grip::Line => self.line_point(i, line, snap(tick_at(p.x), g), auto_v(clip_body(rects[i]), p.y)).map(|k| Drag::Point(i, k, line)),
                    };
                } else if under.is_none() && matches!(self.playlist.tool, Tool::Draw | Tool::Paint) {
                    // the current pattern goes here; draw then drags it, paint keeps painting copies
                    let (lane, start) = (lane_at(p.y), snap(tick_at(p.x), g));
                    self.run_merge("clip.add", json!({ "lane": lane, "start": start }), "draw");
                    let i = self.session.project.clips.len() - 1;
                    let length = self.session.project.clips[i].length;
                    self.playlist.selected = BTreeSet::from([i]);
                    self.spark(ui, t.accent);
                    self.playlist.drag = Some(if self.playlist.tool == Tool::Paint {
                        Drag::Paint { lane, next: start + length, length }
                    } else {
                        Drag::Move { grabbed: i, dt: tick_at(p.x) - start as f64, lane, from: vec![(i, start, lane)] }
                    });
                } else if let Some((i, edge)) = under {
                    if adding {
                        // Ctrl / Shift: in or out of the selection
                        if !self.playlist.selected.remove(&i) {
                            self.playlist.selected.insert(i);
                        }
                    } else if !self.playlist.selected.contains(&i) {
                        self.playlist.selected = BTreeSet::from([i]);
                    }
                    let clips = &self.session.project.clips;
                    let mut moving: BTreeSet<usize> = self.playlist.selected.clone();
                    moving.insert(i);
                    let edges = || moving.iter().filter_map(|&k| clips.get(k).map(|c| (k, c.start, c.length, c.offset))).collect();
                    self.playlist.drag = Some(match edge {
                        Edge::Right => Drag::Resize { grabbed: i, from: edges() },
                        Edge::Left => Drag::Trim { grabbed: i, from: edges() },
                        Edge::Body => Drag::Move {
                            grabbed: i,
                            dt: tick_at(p.x) - clips[i].start as f64,
                            lane: clips[i].lane,
                            from: moving.into_iter().filter_map(|k| clips.get(k).map(|c| (k, c.start, c.lane))).collect(),
                        },
                    });
                } else {
                    // empty space: drag a box over the clips to select them
                    if !adding {
                        self.playlist.selected.clear();
                    }
                    self.playlist.drag = Some(Drag::Select { from: p - rect.min, keep: self.playlist.selected.clone() });
                }
            } else if resp.is_pointer_button_down_on()
                && let Some(drag) = self.playlist.drag.clone()
            {
                match drag {
                    Drag::Bend(i, k, line, bent, y0) => {
                        // up bows the stretch up, down bows it down
                        if let Some((points, mut curves)) = self.session.project.clips.get(i).and_then(|c| line_data(c, line))
                            && k + 1 < points.len()
                        {
                            curves.resize(points.len() - 1, 0.0);
                            let curve = (bent + (y0 - p.y) / 60.0).clamp(-1.0, 1.0);
                            if (curves[k] - curve).abs() > 1e-3 {
                                curves[k] = curve;
                                let (op, props) = set_line(i, line, points, curves);
                                self.run_merge(op, props, "auto-bend");
                            }
                            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
                        }
                    }
                    Drag::Point(i, k, line) => {
                        if let Some(c) = self.session.project.clips.get(i)
                            && let Some((points, curves)) = line_data(c, line)
                            && k < points.len()
                        {
                            // a point stays between its neighbours and inside the clip
                            let (from, to) = (c.offset, c.offset + c.length);
                            let lo = if k > 0 { points[k - 1].0 + 1 } else { 0 }.max(from);
                            let hi = points.get(k + 1).map_or(to, |n| n.0.saturating_sub(1)).min(to).max(lo);
                            let tick = ((snap(tick_at(p.x), g) as i64 - c.start as i64 + from as i64).max(0) as u32).clamp(lo, hi);
                            let mut moved = points.clone();
                            moved[k] = (tick, auto_v(clip_body(rects[i]), p.y));
                            if moved[k] != points[k] {
                                let (op, props) = set_line(i, line, moved, curves);
                                self.run_merge(op, props, "auto-point");
                            }
                        }
                    }
                    Drag::Tool { .. } => self.tool_stroke(under.map(|(i, _)| i)),
                    Drag::Paint { lane, mut next, length } => {
                        while tick_at(p.x) >= next as f64 {
                            self.run_merge("clip.add", json!({ "lane": lane, "start": next }), "draw");
                            next += length.max(1);
                        }
                        self.playlist.drag = Some(Drag::Paint { lane, next, length });
                    }
                    Drag::Move { grabbed, dt, lane, from } => {
                        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                        // the grabbed clip snaps; the others keep their distance to it, and
                        // nothing goes before the start or above the first lane
                        let start0 = from.iter().find(|f| f.0 == grabbed).map_or(0, |f| f.1) as i64;
                        let earliest = from.iter().map(|f| f.1).min().unwrap_or(0) as i64;
                        let (top, bottom) = (from.iter().map(|f| f.2).min().unwrap_or(0) as i64, from.iter().map(|f| f.2).max().unwrap_or(0) as i64);
                        let ticks = (snap(tick_at(p.x) - dt, g) as i64 - start0).max(-earliest);
                        let lanes_by = (lane_at(p.y) as i64 - lane as i64).clamp(-top, (lanes as i64 - 1 - bottom).max(-top));
                        for (k, start, lane) in from {
                            let (start, lane) = ((start as i64 + ticks) as u32, (lane as i64 + lanes_by) as u32);
                            if self.session.project.clips.get(k).is_some_and(|c| (c.start, c.lane) != (start, lane)) {
                                self.run_merge("clip.set", json!({ "index": k, "start": start, "lane": lane }), "clip-drag");
                            }
                        }
                    }
                    Drag::Resize { grabbed, from } => {
                        for (k, length) in resized(&from, grabbed, snap(tick_at(p.x), g), g.max(STEP / 4)) {
                            if self.session.project.clips.get(k).is_some_and(|c| c.length != length) {
                                self.run_merge("clip.set", json!({ "index": k, "length": length }), "clip-drag");
                            }
                        }
                    }
                    Drag::Trim { grabbed, from } => {
                        for (k, start, length, offset) in trimmed(&from, grabbed, snap(tick_at(p.x), g), g.max(STEP / 4)) {
                            if self.session.project.clips.get(k).is_some_and(|c| c.start != start) {
                                self.run_merge("clip.set", json!({ "index": k, "start": start, "length": length, "offset": offset }), "clip-drag");
                            }
                        }
                    }
                    Drag::Position => {
                        let tick = snap(tick_at(p.x), g);
                        if tick != self.session.position {
                            self.run("transport.set_position", json!({ "tick": tick }));
                        }
                        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                    }
                    Drag::Select { from, mut keep } => {
                        let area = Rect::from_two_pos(rect.min + from, p);
                        painter.rect_filled(area, CornerRadius::same(t.radius_sm), t.select.gamma_multiply(0.12));
                        painter.rect_stroke(area, CornerRadius::same(t.radius_sm), Stroke::new(1.0, t.select), StrokeKind::Inside);
                        keep.extend(rects.iter().enumerate().filter(|(_, r)| r.intersects(area)).map(|(k, _)| k));
                        self.playlist.selected = keep;
                    }
                }
            }
            // right-drag along the ruler: a loop region (only that part plays, over and over)
            if resp.drag_started_by(egui::PointerButton::Secondary)
                && let Some(o) = ui.input(|i| i.pointer.press_origin())
                && o.y < ruler_y + RULER_H
                && o.x > head.right()
            {
                self.playlist.loop_drag = Some(snap(tick_at(o.x), g));
            }
            if let Some(anchor) = self.playlist.loop_drag {
                let at = snap(tick_at(p.x), g);
                let region = (anchor.min(at), anchor.max(at));
                if region.1 > region.0 && self.session.loop_region != Some(region) {
                    self.run("transport.loop", json!({ "from": region.0, "to": region.1 }));
                }
                if !ui.input(|i| i.pointer.secondary_down()) {
                    self.playlist.loop_drag = None;
                }
            }
            if resp.secondary_clicked()
                && let Some(k) = on_chip(p)
            {
                self.playlist.menu = Some(Menu::Marker(k));
            } else if resp.secondary_clicked() && p.y < ruler_y + RULER_H && p.x > head.right() {
                self.playlist.menu = Some(Menu::Ruler(snap(tick_at(p.x), g)));
            } else if resp.secondary_clicked()
                && let Some((i, line, Grip::Point(k))) = on_line
            {
                // right-click a point: it goes (the clip menu stays shut)
                no_menu = true;
                if let Some((mut points, mut curves)) = line_data(&self.session.project.clips[i], line)
                    && points.len() > 1
                {
                    remove_point(&mut points, &mut curves, k);
                    let (op, props) = set_line(i, line, points, curves);
                    self.run(op, props);
                }
            } else if resp.secondary_clicked() && in_body(p) {
                if let Some((i, _)) = under
                    && !self.playlist.selected.contains(&i)
                {
                    self.playlist.selected = BTreeSet::from([i]);
                }
                self.playlist.menu = Some(match under {
                    Some((i, _)) => Menu::Clip(i, snap(tick_at(p.x), STEP)),
                    None => Menu::Empty(lane_at(p.y), snap(tick_at(p.x), g)),
                });
            }
            // double-click a marker: rename it; the ruler: the playhead jumps there, even while playing
            if resp.double_clicked()
                && let Some(k) = on_chip(p)
            {
                self.rename = Some(Renaming::new(Rename::Marker(k), &self.session.project.markers[k].name));
            } else if resp.double_clicked() && p.y < ruler_y + RULER_H && p.x > head.right() {
                self.run("transport.jump", json!({ "tick": snap(tick_at(p.x), g) }));
            }
            if resp.double_clicked()
                && let Some((i, _)) = under
            {
                match self.session.project.clips[i].kind {
                    ClipKind::Pattern { pattern } => {
                        self.run("pattern.select", json!({ "pattern": pattern }));
                        self.tab = crate::Tab::Rack;
                    }
                    // on a line a double-click is two points (on a ring: straight again); elsewhere it opens the clip
                    ClipKind::Audio { .. } | ClipKind::Automation { .. } if on_line.is_none() => self.playlist.props = Some(i),
                    ClipKind::Audio { .. } | ClipKind::Automation { .. } => {
                        if let Some((i, line, Grip::Bend(k))) = on_line
                            && let Some((points, mut curves)) = line_data(&self.session.project.clips[i], line)
                            && k < curves.len()
                        {
                            curves[k] = 0.0;
                            let (op, props) = set_line(i, line, points, curves);
                            self.run(op, props);
                        }
                    }
                }
            }
            if resp.hovered() {
                let zoom = ui.input(|i| i.zoom_delta()); // Ctrl+wheel or pinch
                if zoom != 1.0 {
                    self.playlist.zoom = (self.playlist.zoom * zoom).clamp(4.0, 320.0);
                }
                // Alt+wheel: lane height (and the scroll area doesn't scroll)
                let (alt, wheel) = ui.input(|i| (i.modifiers.alt, i.smooth_scroll_delta.y + i.smooth_scroll_delta.x));
                if alt && wheel != 0.0 {
                    self.playlist.lane_h = (self.playlist.lane_h * (1.0 + wheel * 0.004)).clamp(24.0, 160.0);
                    ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
                }
            }
        }
        // a marker in hand: it stays between its neighbors, so its index doesn't change
        if let Some((k, offset, from_x, moved)) = self.playlist.marker_grab
            && let Some(p) = pointer
        {
            let markers = &self.session.project.markers;
            if k < markers.len() {
                let lo = if k > 0 { markers[k - 1].tick + 1 } else { 0 };
                let hi = markers.get(k + 1).map_or(u32::MAX, |m| m.tick.saturating_sub(1));
                let tick = snap((tick_at(p.x) - offset).max(0.0), g).clamp(lo, hi.max(lo));
                let moved = moved || (p.x - from_x).abs() > 3.0;
                if moved && tick != markers[k].tick {
                    self.run_merge("marker.set", json!({ "index": k, "tick": tick }), "marker-drag");
                }
                self.playlist.marker_grab = Some((k, offset, from_x, moved));
                ui.ctx().set_cursor_icon(if moved { CursorIcon::Grabbing } else { CursorIcon::PointingHand });
                if ui.input(|i| i.pointer.primary_released()) {
                    if !moved {
                        let tick = self.session.project.markers[k].tick;
                        self.run("transport.jump", json!({ "tick": tick }));
                    }
                    self.session.close_merge();
                    self.playlist.marker_grab = None;
                }
            } else {
                self.playlist.marker_grab = None;
            }
        } else if let Some(p) = pointer
            && on_chip(p).is_some()
        {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        // the loop in hand
        if let Some((part, a, b, at)) = self.playlist.loop_grab
            && let Some(p) = pointer
        {
            let step = g.max(1);
            let here = snap(tick_at(p.x), step);
            let region = match part {
                0 => (here.min(b.saturating_sub(step)), b),
                1 => (a, here.max(a + step)),
                _ => {
                    let from = snap((a as f64 + tick_at(p.x) - at).max(0.0), step);
                    (from, from + (b - a))
                }
            };
            if Some(region) != self.session.loop_region {
                self.run("transport.loop", json!({ "from": region.0, "to": region.1 }));
                self.playlist.loop_moved = true;
            }
            ui.ctx().set_cursor_icon(if part == 2 { CursorIcon::Grabbing } else { CursorIcon::ResizeHorizontal });
            if ui.input(|i| i.pointer.primary_released()) {
                if part == 2 && !self.playlist.loop_moved {
                    self.run("transport.set_position", json!({ "tick": snap(at, g) }));
                }
                self.playlist.loop_grab = None;
            }
        } else if let Some((a, b)) = self.session.loop_region
            && let Some(p) = pointer.filter(|p| p.y < ruler_y + RULER_H && p.x > head.right())
        {
            let (xa, xb) = (x_of(a as f64), x_of(b as f64));
            if (p.x - xa).abs() <= 5.0 || (p.x - xb).abs() <= 5.0 {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            } else if p.x > xa && p.x < xb {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
            }
        }
        if ui.input(|i| i.pointer.primary_released()) && self.playlist.drag.take().is_some() {
            self.session.close_merge();
        }
        if !no_menu {
            resp.context_menu(|ui| match self.playlist.menu {
                Some(Menu::Empty(lane, at)) => self.insert_menu(ui, lane, at),
                Some(Menu::Clip(i, at)) => self.clip_menu(ui, i, at),
                Some(Menu::Ruler(at)) => self.ruler_menu(ui, at),
                Some(Menu::Marker(k)) => self.marker_menu(ui, k),
                None => {
                    ui.label(tr!("Right-click a lane to insert a pattern."));
                }
            });
        }

        // keys: clipboard, select all, and for the selection delete, duplicate, nudge, mute
        let typing = ui.ctx().egui_wants_keyboard_input();
        let here = resp.contains_pointer();
        // over the piano roll, Ctrl+C / X / V are its own
        let piano = self.piano.hovered.is_some_and(|f| f + 1 >= ui.ctx().cumulative_frame_nr());
        if !typing && !piano {
            let (copy, cut, paste) = ui.input(|i| {
                let has = |f: fn(&egui::Event) -> bool| i.events.iter().any(f);
                (has(|e| matches!(e, egui::Event::Copy)), has(|e| matches!(e, egui::Event::Cut)), has(|e| matches!(e, egui::Event::Paste(_))))
            });
            if (copy || cut) && !self.playlist.selected.is_empty() {
                self.copy_clips(ui.ctx());
                if cut {
                    self.delete_clips();
                }
            }
            if paste && (here || !self.playlist.selected.is_empty()) && !self.playlist.clipboard.is_empty() {
                let top = self.playlist.clipboard.iter().map(|c| c.lane).min().unwrap_or(0);
                let (lane, start) = ui.input(|i| i.pointer.hover_pos()).filter(|p| in_body(*p)).map_or((top, self.session.position), |p| (lane_at(p.y), snap(tick_at(p.x), g)));
                self.paste_clips(lane, start);
            }
            if here && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
                self.select_clips(|_| true);
            }
        }
        if !self.playlist.selected.is_empty() && !typing {
            let step = grid(beat_px) as i64;
            if ui.input_mut(|inp| inp.consume_key(Modifiers::NONE, Key::Delete)) {
                self.delete_clips();
            } else if ui.input_mut(|inp| inp.consume_key(Modifiers::COMMAND, Key::D)) {
                self.duplicate_clips();
            } else if self.playlist.props.is_none() && ui.input_mut(|inp| inp.consume_key(Modifiers::NONE, Key::Escape)) {
                self.playlist.selected.clear();
            } else if here && ui.input_mut(|inp| inp.consume_key(Modifiers::NONE, Key::Num0)) {
                self.mute_clips();
            } else if here {
                for (key, ticks, lanes_by) in [(Key::ArrowLeft, -step, 0), (Key::ArrowRight, step, 0), (Key::ArrowUp, 0, -1), (Key::ArrowDown, 0, 1)] {
                    if ui.input_mut(|inp| inp.consume_key(Modifiers::NONE, key)) {
                        self.nudge_clips(ticks, lanes_by, lanes);
                    }
                }
            }
        }
        self.hint(resp, tr!("Playlist: right-click for menus · drag a box to select · Ctrl+C / X / V · Ctrl+D duplicates forward · arrows nudge · 0 mutes · Alt = no snap · Alt+wheel = lane height"));
    }

    /// Right-click on empty space: insert, paste, select, this lane, view.
    fn insert_menu(&mut self, ui: &mut Ui, lane: u32, at: u32) {
        let list: Vec<(String, Option<[u8; 3]>)> = self.session.project.patterns.iter().map(|p| (p.name.clone(), p.color)).collect();
        let current = self.session.pattern;
        let text = |i: usize, (name, color): &(String, Option<[u8; 3]>)| RichText::new(format!("{}  {name}", icons::SQUARES_FOUR)).color(Theme::pattern(i, *color));
        if ui.button(text(current, &list[current]).strong()).clicked() {
            self.run("clip.add", json!({ "lane": lane, "start": at, "pattern": current }));
            self.playlist.selected = BTreeSet::from([self.session.project.clips.len() - 1]);
            ui.close();
        }
        if list.len() > 1 {
            ui.menu_button(format!("{}  {}", icons::PLUS, tr!("Insert pattern")), |ui| {
                for (i, entry) in list.iter().enumerate() {
                    if ui.button(text(i, entry)).clicked() {
                        self.run("clip.add", json!({ "lane": lane, "start": at, "pattern": i }));
                        self.playlist.selected = BTreeSet::from([self.session.project.clips.len() - 1]);
                        ui.close();
                    }
                }
            });
        }
        if ui.button(format!("{}  {}", icons::FILE_PLUS, tr!("New pattern here"))).clicked() {
            self.run("pattern.add", json!({}));
            self.run("clip.add", json!({ "lane": lane, "start": at }));
            self.playlist.selected = BTreeSet::from([self.session.project.clips.len() - 1]);
            self.tab = crate::Tab::Rack;
            ui.close();
        }
        ui.separator();
        let n = self.playlist.clipboard.len();
        let paste = if n > 1 { format!("{}  {}", icons::COPY, tr!("Paste {} clips here", n)) } else { format!("{}  {}", icons::COPY, tr!("Paste here")) };
        if ui.add_enabled(n > 0, egui::Button::new(paste).shortcut_text("Ctrl+V")).clicked() {
            self.paste_clips(lane, at);
            ui.close();
        }
        ui.menu_button(format!("{}  {}", icons::CROSSHAIR, tr!("Select")), |ui| self.select_menu(ui, lane, None));
        ui.menu_button(format!("{}  {}", icons::LIST, tr!("Lane {}", lane + 1)), |ui| self.lane_menu(ui, lane));
        ui.separator();
        ui.checkbox(&mut self.playlist.snap, tr!("Snap to the grid (Alt: free)"));
        ui.checkbox(&mut self.playlist.follow, tr!("Follow the playhead"));
    }

    /// Select: everything, a lane, a pattern's clips, nothing.
    fn select_menu(&mut self, ui: &mut Ui, lane: u32, pattern: Option<usize>) {
        if ui.add(egui::Button::new(tr!("Everything")).shortcut_text("Ctrl+A")).clicked() {
            self.select_clips(|_| true);
            ui.close();
        }
        if ui.button(tr!("Everything on lane {}", lane + 1)).clicked() {
            self.select_clips(|c| c.lane == lane);
            ui.close();
        }
        if let Some(p) = pattern
            && ui.button(tr!("Every clip of this pattern")).clicked()
        {
            self.select_clips(|c| c.kind == ClipKind::Pattern { pattern: p });
            ui.close();
        }
        if ui.add(egui::Button::new(tr!("Nothing")).shortcut_text("Esc")).clicked() {
            self.playlist.selected.clear();
            ui.close();
        }
    }

    /// Right-click on a clip: it and the selection it's part of.
    fn clip_menu(&mut self, ui: &mut Ui, i: usize, at: u32) {
        let Some(clip) = self.session.project.clips.get(i).cloned() else { return };
        let n = self.playlist.selected.len().max(1);
        let many = if n > 1 { tr!(" {} clips", n) } else { String::new() };
        if ui.button(format!("{}  {}", icons::FADERS_HORIZONTAL, tr!("Properties…"))).clicked() {
            self.playlist.props = Some(i);
            ui.close();
        }
        let pattern = match &clip.kind {
            ClipKind::Pattern { pattern } => {
                let pattern = *pattern;
                for (icon, label, tab) in [(icons::SQUARES_FOUR, tr!("Edit in the rack"), crate::Tab::Rack), (icons::PIANO_KEYS, tr!("Edit in the piano roll"), crate::Tab::PianoRoll)] {
                    if ui.button(format!("{icon}  {label}")).clicked() {
                        self.run("pattern.select", json!({ "pattern": pattern }));
                        self.tab = tab;
                        ui.close();
                    }
                }
                let name = self.session.project.patterns.get(pattern).map_or(String::new(), |p| p.name.clone());
                if ui.button(format!("{}  {}", icons::PENCIL_SIMPLE, tr!("Rename pattern…"))).clicked() {
                    self.rename = Some(Renaming::new(Rename::Pattern(pattern), name));
                    ui.close();
                }
                ui.menu_button(format!("{}  {}", icons::CIRCLE, tr!("Color")), |ui| self.pattern_color_menu(ui, pattern));
                let r = ui.button(format!("{}  {}", icons::SPARKLE, tr!("Make unique")));
                if r.clicked() {
                    self.run("clip.make_unique", json!({ "index": i }));
                    ui.close();
                }
                self.hint(&r, tr!("This clip gets its own copy of the pattern: edit it without changing the other clips."));
                Some(pattern)
            }
            ClipKind::Automation { points, .. } => {
                ui.menu_button(format!("{}  {}", icons::CHART_LINE, tr!("Shape")), |ui| {
                    for (label, points) in crate::clipprops::shapes(EnvTarget::Volume, clip.offset, clip.offset + clip.length) {
                        if ui.button(crate::i18n::t(label)).clicked() {
                            self.run("clip.automation", json!({ "index": i, "points": points }));
                            ui.close();
                        }
                    }
                    if ui.button(tr!("Flip upside down")).clicked() {
                        let flipped: Vec<(u32, f32)> = points.iter().map(|(t, v)| (*t, 1.0 - v)).collect();
                        self.run("clip.automation", json!({ "index": i, "points": flipped }));
                        ui.close();
                    }
                });
                None
            }
            ClipKind::Audio { sample } => {
                let sample = sample.clone();
                let r = ui.button(format!("{}  {}", icons::ARROWS_SPLIT, tr!("Split into drums and tones")));
                if r.clicked() {
                    self.split_clip(i);
                    ui.close();
                }
                r.on_hover_text(tr!("Two new clips under it: the hits, and everything that holds a pitch (the original is muted)"));
                let r = ui.button(format!("{}  {}", icons::MUSIC_NOTES_PLUS, tr!("Notes from this sound")));
                if r.clicked() {
                    self.notes_from_clip(i);
                    ui.close();
                }
                r.on_hover_text(tr!("Hears one voice (a bassline, a vocal, a lead) and writes its notes into a new pattern for a VA-1"));
                ui.separator();
                ui.menu_button(format!("{}  Gain", icons::SPEAKER_HIGH), |ui| {
                    for (label, db) in [("+3 dB", 3.0), ("-3 dB", -3.0), ("-6 dB", -6.0), (tr!("Reset to 0 dB"), f32::NAN)] {
                        if ui.button(label).clicked() {
                            self.each_selected("clip-gain", |c| json!({ "gain": if db.is_nan() { 0.0 } else { c.fx.gain + db } }));
                            ui.close();
                        }
                    }
                });
                ui.menu_button(format!("{}  {}", icons::WAVE_TRIANGLE, tr!("Fades")), |ui| {
                    for (label, fin, fout) in [(tr!("Fade in, 1 beat"), Some(PPQ), None), (tr!("Fade out, 1 beat"), None, Some(PPQ)), (tr!("Fade in and out, 1 beat"), Some(PPQ), Some(PPQ)), (tr!("No fades"), Some(0), Some(0))] {
                        if ui.button(label).clicked() {
                            self.each_selected("clip-fades", |c| json!({ "fade_in": fin.unwrap_or(c.fx.fade_in), "fade_out": fout.unwrap_or(c.fx.fade_out) }));
                            ui.close();
                        }
                    }
                });
                let names: Vec<String> = self.session.project.mixer.iter().map(|m| m.name.clone()).collect();
                ui.menu_button(format!("{}  {}", icons::FADERS, tr!("Plays into")), |ui| {
                    for (k, name) in names.iter().enumerate() {
                        if ui.selectable_label(clip.fx.track == k, name).clicked() {
                            self.each_selected("clip-track", |_| json!({ "track": k }));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button(format!("{}  {}", icons::PLUS, tr!("New track"))).clicked() {
                        let clips = self.selected_clips();
                        self.clips_to_new_track(&clips);
                        ui.close();
                    }
                });
                ui.menu_button(format!("{}  {}", icons::METRONOME, tr!("Follow the tempo")), |ui| {
                    for mode in Stretch::ALL {
                        let help = match mode {
                            Stretch::Off => tr!("Its own speed"),
                            Stretch::Resample => tr!("The song's tempo, like a turntable: faster is higher"),
                            Stretch::Stretch => tr!("The song's tempo, same pitch"),
                        };
                        if ui.selectable_label(clip.fx.stretch == mode, crate::i18n::t(mode.name())).on_hover_text(help).clicked() {
                            let audio: Vec<usize> = self.selected_audio(i);
                            for k in audio {
                                self.follow_tempo(k, mode, None, "clip-stretch");
                            }
                            self.session.close_merge();
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button(format!("{}  {}", icons::GRID_FOUR, tr!("Fit to grid"))).on_hover_text(tr!("Stretch it to a whole number of bars at the song's tempo")).clicked() {
                        for k in self.selected_audio(i) {
                            self.fit_to_grid(k);
                        }
                        ui.close();
                    }
                    if ui.button(format!("{}  {}", icons::METRONOME, tr!("Detect its tempo"))).clicked() {
                        for k in self.selected_audio(i) {
                            if let Some(bpm) = self.sound_bpm(k) {
                                let mode = self.session.project.clips[k].fx.stretch;
                                self.follow_tempo(k, mode, Some(bpm), "clip-bpm-detect");
                                self.say_as(Tone::Done, tr!("Made at {} BPM, by the sound of it", bpm));
                            } else {
                                self.say(tr!("No steady beat in it: type its tempo in the clip's properties"));
                            }
                        }
                        self.session.close_merge();
                        ui.close();
                    }
                });
                if ui.button(format!("{}  {}", icons::WAVEFORM, tr!("Load into a new channel"))).clicked() {
                    self.run("channel.add", json!({ "sample": sample }));
                    self.tab = crate::Tab::Rack;
                    ui.close();
                }
                None
            }
        };
        ui.separator();
        let muted = clip.fx.mute;
        if ui.add(egui::Button::new(format!("{}  {}{many}", if muted { icons::SPEAKER_HIGH } else { icons::SPEAKER_SLASH }, if muted { tr!("Unmute") } else { tr!("Mute") })).shortcut_text("0")).clicked() {
            self.mute_clips();
            ui.close();
        }
        for (icon, label, keys, what) in [(icons::SCISSORS, tr!("Cut"), "Ctrl+X", 0), (icons::COPY, tr!("Copy"), "Ctrl+C", 1), (icons::COPY, tr!("Duplicate"), "Ctrl+D", 2), (icons::TRASH, tr!("Delete"), tr!("Del"), 3)] {
            if ui.add(egui::Button::new(format!("{icon}  {label}{many}")).shortcut_text(keys)).clicked() {
                match what {
                    0 | 1 => {
                        self.copy_clips(ui.ctx());
                        if what == 0 {
                            self.delete_clips();
                        }
                    }
                    2 => self.duplicate_clips(),
                    _ => self.delete_clips(),
                }
                ui.close();
            }
        }
        if n == 1 && ui.button(format!("{}  {}", icons::SQUARE_SPLIT_HORIZONTAL, tr!("Split here"))).clicked() {
            self.run("clip.split", json!({ "index": i, "at": at }));
            ui.close();
        }
        ui.menu_button(format!("{}  {}", icons::CROSSHAIR, tr!("Select")), |ui| self.select_menu(ui, clip.lane, pattern));
    }

    /// Right-click on the ruler.
    fn ruler_menu(&mut self, ui: &mut Ui, at: u32) {
        if ui.button(format!("{}  {}", icons::PLAY, tr!("Start playing from here"))).clicked() {
            self.run("transport.set_position", json!({ "tick": at }));
            ui.close();
        }
        if ui.add(egui::Button::new(format!("{}  {}", icons::ARROW_RIGHT, tr!("Jump here now"))).shortcut_text(tr!("double-click"))).clicked() {
            self.run("transport.jump", json!({ "tick": at }));
            ui.close();
        }
        if ui.button(format!("{}  {}", icons::ARROW_COUNTER_CLOCKWISE, tr!("Back to the start"))).clicked() {
            self.run("transport.jump", json!({ "tick": 0 }));
            ui.close();
        }
        if ui.button(format!("{}  {}", icons::FLAG, tr!("Add a marker here"))).clicked() {
            self.run("marker.add", json!({ "tick": at }));
            ui.close();
        }
        ui.separator();
        let bar = at / BAR * BAR;
        let r = ui.button(format!("{}  {}", icons::ARROW_CLOCKWISE, tr!("Loop this bar")));
        if r.clicked() {
            self.run("transport.loop", json!({ "from": bar, "to": bar + BAR }));
            ui.close();
        }
        r.on_hover_text(tr!("Or right-drag along the ruler to loop any part"));
        if self.session.loop_region.is_some() && ui.button(format!("{}  {}", icons::X, tr!("Clear the loop"))).clicked() {
            self.run("transport.loop", json!({ "clear": true }));
            ui.close();
        }
        ui.separator();
        ui.checkbox(&mut self.playlist.follow, tr!("Follow the playhead"));
        ui.checkbox(&mut self.playlist.snap, tr!("Snap to the grid (Alt: free)"));
    }

    /// Right-click on a marker.
    fn marker_menu(&mut self, ui: &mut Ui, k: usize) {
        let Some(m) = self.session.project.markers.get(k).cloned() else { return };
        ui.label(RichText::new(format!("{}  {}", icons::FLAG, m.name)).strong());
        if ui.button(format!("{}  {}", icons::ARROW_RIGHT, tr!("Jump here now"))).clicked() {
            self.run("transport.jump", json!({ "tick": m.tick }));
            ui.close();
        }
        if ui.add(egui::Button::new(format!("{}  {}", icons::PENCIL_SIMPLE, tr!("Rename…"))).shortcut_text(tr!("double-click"))).clicked() {
            self.rename = Some(Renaming::new(Rename::Marker(k), &m.name));
            ui.close();
        }
        if ui.button(format!("{}  {}", icons::TRASH, tr!("Delete the marker"))).clicked() {
            self.run("marker.remove", json!({ "index": k }));
            ui.close();
        }
    }

    /// A lane's menu: its header, or Lane › in the empty-space menu.
    fn lane_menu(&mut self, ui: &mut Ui, lane: u32) {
        let info = self.session.project.lane(lane);
        let t = self.theme.clone();
        if ui.button(format!("{}  {}", icons::PENCIL_SIMPLE, tr!("Rename…"))).clicked() {
            self.rename = Some(Renaming::new(Rename::Lane(lane), info.name.clone()));
            ui.close();
        }
        ui.menu_button(format!("{}  {}", icons::CIRCLE, tr!("Color")), |ui| {
            for row in PATTERN_COLORS.chunks(6) {
                ui.horizontal(|ui| {
                    for c in row {
                        if kit::swatch(ui, &t, Theme::pattern(0, Some(*c)), 20.0, tr!("Use this color")).clicked() {
                            self.run("lane.set", json!({ "lane": lane, "color": c }));
                            ui.close();
                        }
                    }
                });
            }
            if ui.button(tr!("No color")).clicked() {
                self.run("lane.set", json!({ "lane": lane, "clear_color": true }));
                ui.close();
            }
        });
        for (label, key, on) in [("Mute", "mute", info.mute), ("Solo", "solo", info.solo)] {
            let mut v = on;
            if ui.checkbox(&mut v, label).changed() {
                self.run("lane.set", json!({ "lane": lane, key: v }));
            }
        }
        ui.separator();
        if ui.button(tr!("Select everything on it")).clicked() {
            self.select_clips(|c| c.lane == lane);
            ui.close();
        }
        let folded = self.playlist.folded.contains(&lane);
        if ui.button(if folded { tr!("Unfold") } else { tr!("Fold") }).clicked() {
            if !self.playlist.folded.remove(&lane) {
                self.playlist.folded.insert(lane);
            }
            ui.close();
        }
        ui.separator();
        for (label, at) in [(tr!("Insert a lane above"), lane), (tr!("Insert a lane below"), lane + 1)] {
            if ui.button(format!("{}  {label}", icons::PLUS)).clicked() {
                self.run("lane.insert", json!({ "lane": at }));
                self.playlist.selected.clear();
                ui.close();
            }
        }
        if ui.button(format!("{}  {}", icons::TRASH, tr!("Delete the lane and its clips"))).clicked() {
            self.run("lane.remove", json!({ "lane": lane }));
            self.playlist.selected.clear();
            self.playlist.props = None;
            ui.close();
        }
    }
}

impl App {
    /// Selected clips that still exist, in order.
    fn selected_clips(&self) -> Vec<usize> {
        self.playlist.selected.iter().copied().filter(|i| *i < self.session.project.clips.len()).collect()
    }

    pub fn select_all_clips(&mut self) {
        self.select_clips(|_| true);
    }

    pub fn playlist_selection(&self) -> Vec<usize> {
        self.selected_clips()
    }

    fn select_clips(&mut self, keep: impl Fn(&Clip) -> bool) {
        self.playlist.selected = self.session.project.clips.iter().enumerate().filter(|(_, c)| keep(c)).map(|(i, _)| i).collect();
    }

    /// Ctrl+C: the selection into the clipboard (and a note on the OS one, so Ctrl+V comes back).
    fn copy_clips(&mut self, ctx: &egui::Context) {
        self.playlist.clipboard = self.selected_clips().into_iter().map(|i| self.session.project.clips[i].clone()).collect();
        ctx.copy_text(format!("{} Ravune clips", self.playlist.clipboard.len()));
    }

    /// Ctrl+V: the copied clips with their top-left at (lane, start); they become the selection.
    fn paste_clips(&mut self, lane: u32, start: u32) {
        let clips = self.playlist.clipboard.clone();
        let (Some(top), Some(first)) = (clips.iter().map(|c| c.lane).min(), clips.iter().map(|c| c.start).min()) else { return };
        let before = self.session.project.clips.len();
        self.session.close_merge();
        for c in &clips {
            self.run_merge("clip.add", clip_json(c, lane + c.lane - top, start + c.start - first), "paste");
        }
        self.session.close_merge();
        self.playlist.selected = (before..self.session.project.clips.len()).collect();
    }

    /// Arrow keys: the selection moves a grid cell or a lane, as one gesture.
    fn nudge_clips(&mut self, ticks: i64, lanes_by: i64, lanes: u32) {
        let picked: Vec<(usize, u32, u32)> = self.selected_clips().into_iter().map(|i| (i, self.session.project.clips[i].start, self.session.project.clips[i].lane)).collect();
        let earliest = picked.iter().map(|p| p.1).min().unwrap_or(0) as i64;
        let (top, bottom) = (picked.iter().map(|p| p.2).min().unwrap_or(0) as i64, picked.iter().map(|p| p.2).max().unwrap_or(0) as i64);
        let ticks = ticks.max(-earliest);
        let lanes_by = lanes_by.clamp(-top, (lanes as i64 - 1 - bottom).max(-top));
        for (i, start, lane) in picked {
            self.run_merge("clip.set", json!({ "index": i, "start": start as i64 + ticks, "lane": lane as i64 + lanes_by }), "nudge");
        }
    }

    /// 0: mute or unmute the selection (it follows the first clip).
    pub fn mute_clips(&mut self) {
        let mute = !self.selected_clips().first().is_some_and(|i| self.session.project.clips[*i].fx.mute);
        self.each_selected("mute-clips", |_| json!({ "mute": mute }));
    }

    /// The same clip.set_fx change on every selected clip, as one undo step.
    fn each_selected(&mut self, key: &str, props: impl Fn(&Clip) -> serde_json::Value) {
        self.session.close_merge();
        for i in self.selected_clips() {
            let mut p = props(&self.session.project.clips[i]);
            p["index"] = json!(i);
            self.run_merge("clip.set_fx", p, key);
        }
        self.session.close_merge();
    }

    /// A new point on clip `i`'s `line` at song tick `tick`; its index.
    fn line_point(&mut self, i: usize, line: Line, tick: u32, v: f32) -> Option<usize> {
        let c = self.session.project.clips.get(i)?;
        let (mut points, mut curves) = line_data(c, line)?;
        let tick = tick.clamp(c.start, c.end()) - c.start + c.offset;
        let k = match points.iter().position(|(t, _)| *t >= tick) {
            Some(k) if points[k].0 == tick => {
                points[k].1 = v;
                k
            }
            found => {
                let k = found.unwrap_or(points.len());
                insert_point(&mut points, &mut curves, k, (tick, v));
                k
            }
        };
        let (op, props) = set_line(i, line, points, curves);
        self.run_merge(op, props, "auto-point");
        Some(k)
    }

    /// The audio clips a clip menu acts on: the selection, or just clip `i`.
    fn selected_audio(&self, i: usize) -> Vec<usize> {
        let clips = &self.session.project.clips;
        let audio = |k: &usize| clips.get(*k).is_some_and(|c| matches!(c.kind, ClipKind::Audio { .. }));
        let picked: Vec<usize> = self.playlist.selected.iter().copied().filter(audio).collect();
        if picked.is_empty() { vec![i] } else { picked }
    }

    /// Erase / mute under the mouse, once per clip a stroke.
    fn tool_stroke(&mut self, under: Option<usize>) {
        let (Some(i), Some(Drag::Tool { mut done })) = (under, self.playlist.drag.clone()) else { return };
        if done.contains(&i) {
            return;
        }
        match self.playlist.tool {
            Tool::Erase => {
                self.run_merge("clip.remove", json!({ "index": i }), "erase");
                // the indices after it moved down
                done = done.into_iter().map(|k| if k > i { k - 1 } else { k }).collect();
                self.playlist.selected.clear();
                self.playlist.props = None;
            }
            _ => {
                let mute = !self.session.project.clips[i].fx.mute;
                self.run_merge("clip.set_fx", json!({ "index": i, "mute": mute }), "mute-tool");
                done.insert(i);
            }
        }
        self.playlist.drag = Some(Drag::Tool { done });
    }

    /// Deletes the selection, as one undo step.
    pub fn delete_clips(&mut self) {
        self.session.close_merge();
        for i in self.selected_clips().into_iter().rev() {
            self.run_merge("clip.remove", json!({ "index": i }), "delete-clips");
        }
        self.session.close_merge();
        self.playlist.selected.clear();
        self.playlist.props = None;
    }

    /// Ableton-style duplicate: a copy of the selection right after it, and the copy becomes
    /// the selection, so pressing it again keeps going along the timeline.
    pub fn duplicate_clips(&mut self) {
        let picked: Vec<ravune_core::Clip> = self.selected_clips().into_iter().map(|i| self.session.project.clips[i].clone()).collect();
        let (Some(first), Some(end)) = (picked.iter().map(|c| c.start).min(), picked.iter().map(|c| c.end()).max()) else { return };
        let span = end - first;
        let before = self.session.project.clips.len();
        self.session.close_merge();
        for c in &picked {
            self.run_merge("clip.add", clip_json(c, c.lane, c.start + span), "duplicate-clips");
        }
        self.session.close_merge();
        self.playlist.selected = (before..self.session.project.clips.len()).collect();
    }

    /// The sound of audio clip `i`, as the library has it.
    fn clip_sound(&mut self, i: usize) -> Option<(ravune_core::Clip, String, std::sync::Arc<ravune_engine::Sample>)> {
        let clip = self.session.project.clips.get(i)?.clone();
        let ClipKind::Audio { sample } = &clip.kind else { return None };
        let path = sample.clone();
        match self.library.get(&path) {
            Ok(s) => Some((clip, path, s)),
            Err(e) => {
                self.say_as(Tone::Error, e);
                None
            }
        }
    }

    /// Splits audio clip `i` into its hits and its tones: two new files and clips on new
    /// lanes, playing where it played; the original is muted. One undo step.
    pub fn split_clip(&mut self, i: usize) {
        let Some((clip, path, sound)) = self.clip_sound(i) else { return };
        let (hits, tones) = ravune_engine::split_hpss(&sound.frames);
        let dir = crate::browser::library_dir().with_file_name("Splits");
        let name = sample_name(&path).trim_end_matches(".wav").to_string();
        let mut files = Vec::new();
        for (part, frames) in [(tr!("drums"), &hits), (tr!("tones"), &tones)] {
            let file = (1..).map(|k| dir.join(if k == 1 { format!("{name} - {part}.wav") } else { format!("{name} - {part} {k}.wav") })).find(|p| !p.exists()).unwrap();
            if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| ravune_engine::write_wav(&file, frames, 1, sound.rate as u32)) {
                self.say_as(Tone::Error, tr!("Couldn't save the split: {}", e));
                return;
            }
            files.push(file.to_string_lossy().into_owned());
        }
        let lane = self.session.project.clips.iter().map(|c| c.lane + 1).max().unwrap_or(0);
        self.end_agent_group();
        self.session.begin_group("Split into drums and tones");
        for (k, file) in files.iter().enumerate() {
            let mut props = clip_json(&clip, lane + k as u32, clip.start);
            props["sample"] = json!(file);
            let _ = self.session.run("clip.add", props);
        }
        let _ = self.session.run("clip.set_fx", json!({ "index": i, "mute": true }));
        self.session.end_group();
        self.library.refresh();
        self.say_as(Tone::Done, tr!("Split {}: the drums and the tones are on new lanes, the original is muted", name));
    }

    /// Hears the melody of audio clip `i` and writes it as notes: a new VA-1 channel, a new
    /// pattern as long as the clip, and that pattern on a new lane where the clip plays.
    pub fn notes_from_clip(&mut self, i: usize) {
        let Some((clip, path, sound)) = self.clip_sound(i) else { return };
        let heard = ravune_engine::melody_notes(&sound.frames, sound.rate);
        let tempo = self.session.project.tempo;
        let ticks = |seconds: f32| sound_ticks(seconds, &clip.fx, tempo);
        let notes: Vec<(u32, u32, u8, u8)> = heard
            .iter()
            .map(|n| (ticks(n.start).round() as i64 - clip.offset as i64, ticks(n.length).round().max(6.0) as u32, n.key, n.velocity))
            .filter(|(start, ..)| *start >= 0 && (*start as u32) < clip.length)
            .map(|(start, length, key, velocity)| (start as u32, length, key, velocity))
            .collect();
        if notes.is_empty() {
            self.say_as(Tone::Error, tr!("No melody heard in it (one voice at a time works best)"));
            return;
        }
        let name = sample_name(&path).trim_end_matches(".wav").to_string();
        let steps = clip.length.div_ceil(STEP).clamp(1, ravune_core::MAX_STEPS as u32);
        let lane = self.session.project.clips.iter().map(|c| c.lane + 1).max().unwrap_or(0);
        self.end_agent_group();
        self.session.begin_group("Notes from a sound");
        let done = (|| -> Result<(), String> {
            self.session.run("channel.add", json!({ "name": tr!("Notes of {}", name) }))?;
            let channel = self.session.project.channels.len() - 1;
            self.session.run("pattern.add", json!({}))?;
            let pattern = self.session.pattern;
            self.session.run("pattern.rename", json!({ "pattern": pattern, "name": tr!("Notes of {}", name) }))?;
            self.session.run("pattern.set_length", json!({ "pattern": pattern, "steps": steps }))?;
            for (start, length, key, velocity) in &notes {
                self.session.run("note.add", json!({ "channel": channel, "pattern": pattern, "key": key, "start": start, "length": length, "velocity": velocity }))?;
            }
            self.session.run("clip.add", json!({ "lane": lane, "start": clip.start, "length": clip.length, "pattern": pattern }))
        })();
        self.session.end_group();
        match done {
            Ok(()) => self.say_as(Tone::Done, tr!("{} notes from {}: a new VA-1 channel and pattern, on a new lane", notes.len(), name)),
            Err(e) => self.say_as(Tone::Error, e),
        }
    }

    /// An audio clip as long as the sample at the current tempo.
    pub fn add_audio_clip(&mut self, path: &str, lane: u32, start: u32) {
        if ravune_core::is_instrument(path) {
            self.run("channel.add", json!({ "sample": path }));
            self.say_as(Tone::Done, tr!("Instruments play from the rack: added it as a channel"));
            return;
        }
        match self.library.get(path) {
            Ok(s) => {
                let length = ((s.seconds() as f64 / tick_seconds(self.session.project.tempo)).round() as u32).max(1);
                self.run("clip.add", json!({ "lane": lane, "start": start, "sample": path, "length": length }));
            }
            Err(e) => self.say_as(Tone::Error, e),
        }
    }

    /// Places the files dropped from the OS, one lane each, from `at` (lane, tick), or,
    /// with no spot, on fresh lanes at the start marker.
    pub fn drop_on_playlist(&mut self, at: Option<(u32, u32)>) {
        let p = &self.session.project;
        let (lane, start) = at.unwrap_or((p.clips.iter().map(|c| c.lane + 1).max().unwrap_or(0), self.session.position));
        for (k, path) in std::mem::take(&mut self.dropped).into_iter().enumerate() {
            self.add_audio_clip(&path.to_string_lossy(), lane + k as u32, start);
        }
    }
}

/// clip.add props for a copy of `c` at (lane, start).
fn clip_json(c: &Clip, lane: u32, start: u32) -> serde_json::Value {
    let mut props = json!({ "lane": lane, "start": start, "length": c.length, "offset": c.offset, "fx": c.fx });
    match &c.kind {
        ClipKind::Pattern { pattern } => props["pattern"] = json!(pattern),
        ClipKind::Audio { sample } => props["sample"] = json!(sample),
        ClipKind::Automation { target, points, curves } => {
            props["automation"] = json!(target);
            props["points"] = json!(points);
            props["curves"] = json!(curves);
        }
    }
    props
}

/// How many ticks a sound `seconds` long lasts on the playlist, played as `fx` says.
pub fn sound_ticks(seconds: f32, fx: &ClipFx, tempo: f32) -> f64 {
    seconds as f64 / fx.speed(tempo) / tick_seconds(tempo)
}

/// Seconds per tick at a tempo.
pub fn tick_seconds(tempo: f32) -> f64 {
    60.0 / (tempo as f64 * PPQ as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edge_moves_on_every_selected_clip() {
        // a 4-beat clip at the start, and a 2-beat one at beat 8 whose sound began a beat earlier
        let from = [(0, 0, 4 * PPQ, 0), (1, 8 * PPQ, 2 * PPQ, PPQ)];
        assert_eq!(resized(&from, 0, 5 * PPQ, STEP), vec![(0, 5 * PPQ), (1, 3 * PPQ)], "a beat longer, both");
        assert_eq!(resized(&from, 0, PPQ, STEP), vec![(0, PPQ), (1, STEP)], "none shorter than a step");
        assert_eq!(trimmed(&from, 1, 9 * PPQ, STEP), vec![(0, PPQ, 3 * PPQ, PPQ), (1, 9 * PPQ, PPQ, 2 * PPQ)], "both start a beat later, the sound stays put");
        assert_eq!(trimmed(&from, 1, 6 * PPQ, STEP), vec![(0, 0, 4 * PPQ, 0), (1, 7 * PPQ, 3 * PPQ, 0)], "none before the song or its sound starts");
    }

    /// A bassline on the playlist becomes notes for a VA-1, then splits into two clips.
    #[test]
    fn a_sound_becomes_notes_and_splits() {
        let rate = 22_050.0f32;
        let mut x = vec![0.0f32; (rate * 2.0) as usize];
        for (start, key) in [(0.0f32, 45u8), (0.5, 48), (1.0, 52), (1.5, 43)] {
            let f = 440.0 * ((key as f32 - 69.0) / 12.0).exp2();
            for i in 0..(0.4 * rate) as usize {
                let t = i as f32 / rate;
                x[(start * rate) as usize + i] += (2.0 * (f * t).fract() - 1.0) * 0.5 * (-t * 3.0).exp();
            }
        }
        let file = std::env::temp_dir().join("ravune-smart-bass.wav");
        ravune_engine::write_wav(&file, &x, 1, rate as u32).unwrap();
        let mut app = App::for_tests();
        let path = file.to_string_lossy().into_owned();
        app.session.run("clip.add", json!({ "lane": 0, "start": 0, "sample": path, "length": 4 * PPQ })).unwrap();
        let channels = app.session.project.channels.len();
        app.notes_from_clip(0);
        assert_eq!(app.session.project.channels.len(), channels + 1, "{:?}", app.toast);
        let slot = app.session.project.channels.last().unwrap().slot;
        let keys: Vec<u8> = app.session.current_pattern().notes(slot).iter().map(|n| n.key).collect();
        assert_eq!(keys, vec![45, 48, 52, 43]);
        assert!(matches!(app.session.project.clips.last().unwrap().kind, ClipKind::Pattern { .. }));
        app.session.run("edit.undo", json!({})).unwrap();
        assert_eq!((app.session.project.channels.len(), app.session.project.clips.len()), (channels, 1), "one undo");

        app.split_clip(0);
        let clips = &app.session.project.clips;
        assert_eq!(clips.len(), 3, "{:?}", app.toast);
        assert!(clips[0].fx.mute && !clips[1].fx.mute && !clips[2].fx.mute, "the original is muted");
    }
}
