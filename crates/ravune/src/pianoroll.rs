//! Piano roll v1: draw, move, resize and delete notes; velocity lane; key/scale
//! highlighting with optional fold-to-scale; ghost notes from the pattern's other channels.

use std::collections::BTreeSet;

use eframe::egui::{self, Align2, CornerRadius, CursorIcon, Key, Modifiers, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use ravune_core::{CHORDS, NOTE_NAMES, PPQ, STEP, Scale, Target, bend_spots, curve_at, curve_samples, insert_point, note_name, remove_point};
use serde_json::json;

use crate::playlist::{Tool, tool_buttons};
use crate::theme::Theme;
use crate::{App, icons, kit};

const KEYS: f32 = 42.0;
const VEL_H: f32 = 64.0;
const EDGE: f32 = 5.0;
/// "Auto" (0) follows the zoom: the finer you look, the finer notes move and stretch.
const SNAPS: [(&str, u32); 7] = [("Auto", 0), ("1/4", PPQ), ("1/8", PPQ / 2), ("1/16", STEP), ("1/32", STEP / 2), ("1/64", STEP / 4), ("Off", 1)];

/// The finest grid whose cells are still 10 px wide.
fn auto_grid(ppt: f32) -> u32 {
    [STEP / 4, STEP / 2, STEP, PPQ / 2, PPQ, 4 * PPQ].into_iter().find(|g| *g as f32 * ppt >= 10.0).unwrap_or(4 * PPQ)
}

#[derive(Clone, Copy)]
enum Grab {
    /// Ticks and keys between the note's start and where it was grabbed.
    Move(f64, i32),
    Resize,
}

pub struct PianoView {
    /// Pixels per beat.
    pub zoom: f32,
    /// Key row height in pixels.
    pub row: f32,
    /// Grid in ticks; 0 = follow the zoom.
    pub snap: u32,
    /// Keep drawn/moved notes inside the song's scale.
    pub fold: bool,
    /// What the lane under the grid edits: velocity, fine pitch or brightness.
    pub lane: Lane,
    /// …or, when set, a knob's line inside the pattern.
    pub auto: Option<Target>,
    /// The point of that line being dragged.
    auto_drag: Option<usize>,
    /// …or the bend being dragged: (stretch, its bend and the mouse's height then).
    auto_bend: Option<(usize, f32, f32)>,
    /// Clicking stamps this chord (an index into CHORDS) instead of one note.
    pub chord: Option<usize>,
    pub tool: Tool,
    /// Painting notes: the key and where the next one goes.
    paint: Option<(u8, u32)>,
    drag: Option<(usize, Grab)>,
    last_len: u32,
    preview: Option<u8>,
    /// The pattern and channel the view was last centred for.
    scrolled: Option<(usize, u8)>,
    /// Selected notes (indices into this pattern and channel's notes), and which those are.
    pub selected: BTreeSet<usize>,
    selected_for: Option<(usize, u8)>,
    /// A Ctrl-drag box: where it started, and what was selected before it.
    boxing: Option<(Pos2, BTreeSet<usize>)>,
    /// Moving the selection: each selected note's start and key when it was picked up.
    group: Vec<(usize, u32, u8)>,
    /// The right-click menu over the selection is up (so the right button didn't erase).
    sel_menu: bool,
    /// Notes copied with Ctrl+C / Ctrl+X, for Ctrl+V.
    clipboard: Vec<ravune_core::Note>,
}

impl Default for PianoView {
    fn default() -> Self {
        Self { zoom: 72.0, row: 12.0, snap: 0, fold: false, lane: Lane::Velocity, auto: None, auto_drag: None, auto_bend: None, chord: None, tool: Tool::Draw, paint: None, drag: None, last_len: STEP, preview: None, scrolled: None, selected: BTreeSet::new(), selected_for: None, boxing: None, group: Vec::new(), sel_menu: false, clipboard: Vec::new() }
    }
}

/// The per-note values the lane under the grid edits.
#[derive(Clone, Copy, PartialEq)]
pub enum Lane {
    Velocity,
    /// Fine pitch, -2..2 semitones (samplers and VA-1).
    Fine,
    /// Brightness, 0..1 around 0.5 (VA-1's filter, per note).
    Bright,
}

impl Lane {
    const ALL: [(Lane, &'static str, &'static str); 3] = [
        // labels and help go through `crate::i18n::t` where they're drawn
        (Lane::Velocity, "VEL", "Velocity: how hard each note is played."),
        (Lane::Fine, "PITCH", "Fine pitch per note, ±2 semitones: bends, detuned hits, slides (samplers and VA-1)."),
        (Lane::Bright, "BRIGHT", "Brightness per note: VA-1 opens or closes its filter for that note only."),
    ];

    /// A note's value, 0..1 on the lane, and the lane's resting point.
    fn get(self, n: &ravune_core::Note) -> (f32, f32) {
        match self {
            Lane::Velocity => (n.velocity as f32 / 127.0, 0.0),
            Lane::Fine => ((n.fine + 2.0) / 4.0, 0.5),
            Lane::Bright => (n.bright, 0.5),
        }
    }

    /// The note.set props that put a note at `v` (0..1 on the lane).
    fn set(self, index: usize, v: f32) -> serde_json::Value {
        match self {
            Lane::Velocity => json!({ "index": index, "velocity": (v * 127.0).clamp(1.0, 127.0) as u8 }),
            // fine pitch clicks to whole cents
            Lane::Fine => json!({ "index": index, "fine": ((v * 4.0 - 2.0) * 100.0).round() / 100.0 }),
            Lane::Bright => json!({ "index": index, "bright": v }),
        }
    }

    fn describe(self, n: &ravune_core::Note) -> String {
        match self {
            Lane::Velocity => format!("velocity {}", n.velocity),
            Lane::Fine => tr!("{} cents", format!("{:+.0}", n.fine * 100.0)),
            Lane::Bright => tr!("brightness {}%", format!("{:+.0}", (n.bright - 0.5) * 200.0)),
        }
    }
}

fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

impl PianoView {
    /// The grid notes snap to right now (Alt: none).
    fn grid(&self, ui: &Ui) -> u32 {
        match self.snap {
            _ if ui.input(|i| i.modifiers.alt) => 1,
            0 => auto_grid(self.zoom / PPQ as f32),
            g => g,
        }
    }
}

impl App {
    pub fn piano_roll(&mut self, ui: &mut Ui, now_tick: Option<f64>) {
        let t = self.theme.clone();
        if !self.played {
            self.zone_hint(ui, tr!("Click to draw notes · right button erases · Space plays · A–K play the channel"));
        }
        let Some(channel) = self.session.selected_channel() else {
            kit::padded(ui, &t, |ui| ui.label(RichText::new(tr!("Select a channel in the rack first.")).color(t.fg_muted)));
            return;
        };
        let (slot, name) = (channel.slot, channel.name.clone());
        let pattern_name = self.session.current_pattern().name.clone();
        kit::bar(ui, &t, |ui| {
            ui.label(RichText::new(&name).strong().color(t.accent));
            kit::vsep(ui, &t);
            tool_buttons(ui, &t, &mut self.piano.tool, &[Tool::Draw, Tool::Paint, Tool::Erase, Tool::Slice]);
            ui.label(RichText::new(format!("in {pattern_name}")).color(t.fg_muted));
            kit::vsep(ui, &t);
            let snap_name = match self.piano.snap {
                0 => format!("Auto {}", SNAPS.iter().find(|s| s.1 == auto_grid(self.piano.zoom / PPQ as f32)).map_or("", |s| s.0)),
                g => SNAPS.iter().find(|s| s.1 == g).map_or("1/16", |s| crate::i18n::t(s.0)).to_string(),
            };
            egui::ComboBox::from_id_salt("snap").selected_text(format!("{}  {snap_name}", icons::GRID_FOUR)).width(104.0).show_ui(ui, |ui| {
                for (label, ticks) in SNAPS {
                    ui.selectable_value(&mut self.piano.snap, ticks, crate::i18n::t(label));
                }
            });
            let (root, scale) = (self.session.project.root, self.session.project.scale);
            egui::ComboBox::from_id_salt("root").selected_text(NOTE_NAMES[root as usize]).width(44.0).show_ui(ui, |ui| {
                for (r, n) in NOTE_NAMES.iter().enumerate() {
                    if ui.selectable_label(r as u8 == root, *n).clicked() {
                        self.run("song.set_key", json!({ "root": r }));
                    }
                }
            });
            egui::ComboBox::from_id_salt("scale").selected_text(crate::i18n::t(scale.name())).width(120.0).show_ui(ui, |ui| {
                for s in Scale::ALL {
                    if ui.selectable_label(s == scale, crate::i18n::t(s.name())).clicked() {
                        self.run("song.set_key", json!({ "scale": s }));
                    }
                }
            });
            let r = kit::button(ui, &t, Some(icons::MAGNET), tr!("Fold to scale"), self.piano.fold);
            if r.clicked() {
                self.piano.fold = !self.piano.fold;
            }
            self.hint(&r, tr!("New and moved notes snap to the nearest note of the song's scale: no wrong notes."));
            let chord = self.piano.chord.map_or(tr!("Single notes"), |c| crate::i18n::t(CHORDS[c].0));
            let r = egui::ComboBox::from_id_salt("chord").selected_text(format!("{}  {chord}", icons::MUSIC_NOTES)).width(118.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.piano.chord, None, tr!("Single notes"));
                for (k, (name, _)) in CHORDS.iter().enumerate() {
                    ui.selectable_value(&mut self.piano.chord, Some(k), crate::i18n::t(name));
                }
            });
            self.hint(&r.response, tr!("Chord stamp: a click draws the whole chord from the key you click (Fold to scale keeps it in key)."));
            let r = kit::button(ui, &t, Some(icons::MAGIC_WAND), tr!("Tools"), false);
            kit::menu(&t, &r).show(|ui| {
                let grid = self.piano.grid(ui).max(STEP / 4);
                for (label, op, props, help) in [
                    (tr!("Quantize"), "notes.quantize", json!({ "grid": grid }), tr!("Every note start onto the grid")),
                    (tr!("Legato"), "notes.legato", json!({}), tr!("Each note up to the next one")),
                    (tr!("Strum"), "notes.strum", json!({ "ticks": STEP / 4 }), tr!("Chords spread out, low to high")),
                    (tr!("Humanize"), "notes.humanize", json!({ "amount": 0.3 }), tr!("A little off the grid, varied velocity")),
                    (tr!("Clear"), "notes.clear", json!({}), tr!("Every note of this channel in this pattern")),
                ] {
                    if ui.button(label).on_hover_text(help).clicked() {
                        self.run(op, props);
                        ui.close();
                    }
                }
                ui.separator();
                ui.menu_button(format!("{}  {}", icons::CODE, tr!("Scripts")), |ui| {
                    for script in crate::script::list() {
                        if ui.button(&script.name).on_hover_text(&script.about).clicked() {
                            self.run_script(&script);
                            ui.close();
                        }
                    }
                    ui.separator();
                    let r = ui.button(format!("{}  {}", icons::FOLDER_OPEN, tr!("Your scripts folder")));
                    if r.on_hover_text(tr!("Luau scripts (.luau) put here show up in this menu and the command palette")).clicked() {
                        let dir = crate::script::scripts_dir();
                        let _ = std::fs::create_dir_all(&dir);
                        ui.ctx().open_url(egui::OpenUrl::new_tab(format!("file:///{}", dir.display())));
                        ui.close();
                    }
                });
            });
            self.hint(&r, tr!("Tools on this channel's notes: quantize, legato, strum, humanize, clear, and scripts (arpeggios, Euclidean rhythms, basslines…)."));
            kit::right(ui, |ui| {
                for (icon, factor, label) in [(icons::MAGNIFYING_GLASS_PLUS, 1.25, tr!("Zoom in (Ctrl wheel)")), (icons::MAGNIFYING_GLASS_MINUS, 0.8, tr!("Zoom out (Ctrl wheel)"))] {
                    if kit::icon_button(ui, &t, icon, label, false).clicked() {
                        self.piano.zoom = (self.piano.zoom * factor).clamp(16.0, 800.0);
                    }
                }
                kit::vsep(ui, &t);
                self.inspector_toggle(ui, &t);
                for (icon, factor, label) in [(icons::PLUS, 1.2, tr!("Taller rows (Alt wheel)")), (icons::MINUS, 1.0 / 1.2, tr!("Shorter rows (Alt wheel)"))] {
                    if kit::icon_button(ui, &t, icon, label, false).clicked() {
                        self.piano.row = (self.piano.row * factor).clamp(6.0, 36.0);
                    }
                }
            });
        });

        self.inspector(ui, "piano");
        let ppt = self.piano.zoom / PPQ as f32;
        let length = self.session.current_pattern().ticks();
        let grid_h = (ui.available_height() - VEL_H).max(80.0);
        let mut area = egui::ScrollArea::both().id_salt("piano").max_height(grid_h).auto_shrink(false);
        // another pattern or channel: its notes in the middle (an empty one starts at C6)
        if self.piano.scrolled != Some((self.session.pattern, slot)) {
            self.piano.scrolled = Some((self.session.pattern, slot));
            let keys = self.session.current_pattern().notes(slot).iter().map(|n| n.key as f32);
            let offset = match keys.clone().reduce(f32::min).zip(keys.reduce(f32::max)) {
                Some((lo, hi)) => (127.0 - (lo + hi) / 2.0) * self.piano.row - grid_h / 2.0,
                None => (127 - 84) as f32 * self.piano.row,
            };
            area = area.vertical_scroll_offset(offset.max(0.0));
        }
        let out = area.show(ui, |ui| {
            let size = vec2(KEYS + length as f32 * ppt, 128.0 * self.piano.row);
            let (rect, resp) = ui.allocate_exact_size(size.max(vec2(ui.available_width(), 0.0)), Sense::click_and_drag());
            self.piano_body(ui, &t, rect, &resp, ppt, slot, length, now_tick);
        });
        self.velocity_lane(ui, &t, ppt, slot, out.state.clamped_offset().x, out.inner_rect.left());
    }

    #[allow(clippy::too_many_arguments)]
    fn piano_body(&mut self, ui: &mut Ui, t: &Theme, rect: Rect, resp: &egui::Response, ppt: f32, slot: u8, length: u32, now_tick: Option<f64>) {
        let visible = ui.clip_rect();
        let painter = ui.painter_at(rect);
        let x_of = |tick: f64| rect.left() + KEYS + tick as f32 * ppt;
        let tick_at = |x: f32| ((x - rect.left() - KEYS) / ppt).max(0.0) as f64;
        let row = self.piano.row;
        let key_at = |y: f32| (127.0 - ((y - rect.top()) / row).floor()).clamp(0.0, 127.0) as u8;
        let row_top = |key: u8| rect.top() + (127 - key) as f32 * row;
        let g = self.piano.grid(ui);
        let (root, scale) = (self.session.project.root, self.session.project.scale);
        let here = (self.session.pattern, slot);
        if self.piano.selected_for != Some(here) {
            (self.piano.selected_for, self.piano.boxing) = (Some(here), None);
            self.piano.selected.clear();
        }
        let count = self.session.current_pattern().notes(slot).len();
        self.piano.selected.retain(|i| *i < count);
        let pattern = self.session.current_pattern();

        // rows: black keys darker, out-of-scale rows dimmer, C rows ruled
        for key in 0..128u8 {
            let r = Rect::from_min_size(pos2(rect.left(), row_top(key)), vec2(rect.width(), row));
            if !r.intersects(visible) {
                continue;
            }
            let mut fill = if is_black(key) { Theme::mix(t.bg_base, t.bg_field, 0.6) } else { t.bg_base };
            if scale != Scale::Chromatic {
                fill = if scale.contains(root, key) { Theme::mix(fill, t.fg_text, 0.05) } else { Theme::mix(fill, t.bg_field, 0.7) };
            }
            painter.rect_filled(r, CornerRadius::ZERO, fill);
            if key % 12 == 0 {
                painter.line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, t.line));
            }
        }
        // every other group of columns is a shade lighter, so you know where you are at any
        // zoom: beats close up, bars further out, four bars far out
        let group = [PPQ, 4 * PPQ, 16 * PPQ].into_iter().find(|g| *g as f32 * ppt >= 40.0).unwrap_or(16 * PPQ);
        let first_group = ((visible.left() - rect.left() - KEYS) / (group as f32 * ppt)).floor().max(0.0) as u32;
        for k in (first_group..=length / group).filter(|k| k % 2 == 1) {
            let (x0, x1) = (x_of((k * group) as f64), x_of(((k + 1) * group).min(length) as f64));
            if x0 > visible.right() {
                break;
            }
            let band = Rect::from_x_y_ranges(x0..=x1, rect.y_range()).intersect(visible);
            painter.rect_filled(band, CornerRadius::ZERO, t.fg_text.gamma_multiply(0.035));
        }
        // grid lines at the snap, thinned out until they're 6 px apart
        let mut grid = g.max(STEP / 4);
        while (grid as f32 * ppt) < 6.0 {
            grid *= 2;
        }
        for k in 0..=length / grid {
            let tick = k * grid;
            let color = if tick.is_multiple_of(PPQ * 4) {
                t.fg_muted
            } else if tick.is_multiple_of(PPQ) {
                t.line
            } else {
                Theme::mix(t.bg_base, t.line, 0.4)
            };
            let x = x_of(tick as f64);
            painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, color));
        }

        // ghost notes (other channels), then this channel's notes
        for notes in pattern.notes.iter().filter(|(s, _)| **s != slot).map(|(_, n)| n) {
            for n in notes {
                let r = Rect::from_min_size(pos2(x_of(n.start as f64), row_top(n.key) + 1.0), vec2(n.length as f32 * ppt, row - 2.0));
                painter.rect_filled(r, CornerRadius::same(1), Theme::mix(t.bg_base, t.fg_muted, 0.25));
            }
        }
        let notes = pattern.notes(slot).to_vec();
        let note_rect = |n: &ravune_core::Note| Rect::from_min_size(pos2(x_of(n.start as f64), row_top(n.key) + 1.0), vec2((n.length as f32 * ppt).max(3.0), row - 2.0));
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        // notes light up while they sound, and so do their keys on the left
        let sounding = |n: &ravune_core::Note| now_tick.is_some_and(|tick| (n.start as f64) <= tick && tick < n.end() as f64);
        let mut lit = [false; 128];
        for (k, n) in notes.iter().enumerate() {
            let r = note_rect(n);
            let mut fill = Theme::mix(Theme::mix(t.bg_control, t.accent, 0.4), t.accent, n.velocity as f32 / 127.0);
            if self.piano.selected.contains(&k) {
                fill = Theme::mix(fill, t.fg_text, 0.35);
                painter.rect_stroke(r.expand(1.0), CornerRadius::same(3), Stroke::new(1.0, t.fg_text.gamma_multiply(0.7)), StrokeKind::Outside);
            }
            if sounding(n) {
                lit[n.key as usize] = true;
                painter.rect_filled(r.expand(2.0), CornerRadius::same(3), t.accent.gamma_multiply(0.25));
                painter.rect_filled(r, CornerRadius::same(2), Theme::mix(fill, t.fg_text, 0.45));
                continue;
            }
            painter.rect_filled(r, CornerRadius::same(2), fill);
            if pointer.is_some_and(|p| r.contains(p) && p.x > r.right() - EDGE) {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
        }
        if let Some(tick) = now_tick {
            let x = x_of(tick);
            painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.5, t.fg_text));
        }

        // sticky keyboard on the left
        let keys = Rect::from_min_size(pos2(visible.left(), rect.top()), vec2(KEYS, rect.height()));
        for key in 0..128u8 {
            let r = Rect::from_min_size(pos2(keys.left(), row_top(key)), vec2(KEYS - 2.0, row));
            if !r.intersects(visible) {
                continue;
            }
            let base = if is_black(key) { t.key_black } else { t.key_white };
            painter.rect_filled(r, CornerRadius::ZERO, if lit[key as usize] { Theme::mix(base, t.accent, 0.7) } else { base });
            if key % 12 == 0 {
                painter.text(r.right_center() - vec2(3.0, 0.0), Align2::RIGHT_CENTER, note_name(key), Theme::mono(8.0), t.key_label);
            }
        }

        // ---- interaction ----
        let hit = |p: Pos2| notes.iter().enumerate().rev().find_map(|(i, n)| {
            let r = note_rect(n);
            r.contains(p).then_some((i, p.x > r.right() - EDGE))
        });
        let fold_on = self.piano.fold;
        let fold = |key: u8| if fold_on { scale.fold(root, key) } else { key };
        let snap = |tick: f64, grid: u32| ((tick.max(0.0) / grid as f64).floor() as u32 * grid).min(length.saturating_sub(1));
        let mut preview_key = None;
        if let Some(p) = pointer {
            if resp.hovered() && ui.input(|i| i.pointer.primary_pressed()) {
                self.session.close_merge();
                if p.x < keys.right() {
                    preview_key = Some(key_at(p.y));
                } else if ui.input(|i| i.modifiers.command) {
                    // Ctrl selects: a note in or out, or a box over everything it touches
                    match hit(p) {
                        Some((i, _)) => {
                            if !self.piano.selected.remove(&i) {
                                self.piano.selected.insert(i);
                            }
                        }
                        None => self.piano.boxing = Some((p, self.piano.selected.clone())),
                    }
                } else if self.piano.tool == Tool::Erase {
                    // the eraser below does it while the button is down
                } else if self.piano.tool == Tool::Slice {
                    if let Some((i, _)) = hit(p) {
                        let n = notes[i];
                        let at = snap(tick_at(p.x), g.max(1));
                        if at > n.start && at < n.end() {
                            self.run_merge("note.set", json!({ "index": i, "length": at - n.start }), "slice");
                            self.run_merge("note.add", json!({ "key": n.key, "start": at, "length": n.end() - at, "velocity": n.velocity, "fine": n.fine, "bright": n.bright }), "slice");
                            self.session.close_merge();
                        }
                    }
                } else if let Some((i, edge)) = hit(p) {
                    let n = notes[i];
                    self.piano.last_len = n.length; // the next note drawn gets this length
                    let grab = if edge { Grab::Resize } else { Grab::Move(tick_at(p.x) - n.start as f64, key_at(p.y) as i32 - n.key as i32) };
                    // a selected note picked up brings the rest of the selection along
                    self.piano.group = if !edge && self.piano.selected.contains(&i) {
                        self.piano.selected.iter().filter_map(|&k| notes.get(k).map(|n| (k, n.start, n.key))).collect()
                    } else {
                        self.piano.selected.clear();
                        Vec::new()
                    };
                    self.piano.drag = Some((i, grab));
                    preview_key = Some(n.key);
                } else if let Some(c) = self.piano.chord {
                    self.piano.selected.clear();
                    let (key, start) = (key_at(p.y), snap(tick_at(p.x), g));
                    self.run("notes.chord", json!({ "key": key, "start": start, "length": self.piano.last_len, "chord": CHORDS[c].0, "fold": fold_on }));
                    self.spark(ui, t.accent);
                    preview_key = Some(fold(key));
                } else {
                    self.piano.selected.clear();
                    let (key, start) = (fold(key_at(p.y)), snap(tick_at(p.x), g));
                    let props = json!({ "key": key, "start": start, "length": self.piano.last_len });
                    self.run_merge("note.add", props, "note-drag");
                    self.spark(ui, t.accent);
                    if self.piano.tool == Tool::Paint {
                        // paint: more of the same note, one after the other, as the mouse goes right
                        self.piano.paint = Some((key, start + self.piano.last_len));
                    } else {
                        let i = self.session.current_pattern().notes(slot).len().saturating_sub(1);
                        self.piano.drag = Some((i, Grab::Move(tick_at(p.x) - start as f64, 0)));
                    }
                    preview_key = Some(key);
                }
            } else if resp.is_pointer_button_down_on()
                && let Some((key, mut next)) = self.piano.paint
            {
                while tick_at(p.x) >= next as f64 && next < length {
                    self.run_merge("note.add", json!({ "key": key, "start": next, "length": self.piano.last_len }), "note-drag");
                    next += self.piano.last_len.max(1);
                }
                self.piano.paint = Some((key, next));
            } else if resp.is_pointer_button_down_on()
                && let Some((i, grab)) = self.piano.drag
                && let Some(n) = notes.get(i)
            {
                match grab {
                    Grab::Move(dt, dk) if !self.piano.group.is_empty() => {
                        // the whole selection, by what the grabbed note moved; none goes past an edge
                        let group = self.piano.group.clone();
                        let &(_, s0, k0) = group.iter().find(|m| m.0 == i).unwrap_or(&group[0]);
                        let lowest = group.iter().map(|m| m.1).min().unwrap_or(0) as i64;
                        let latest = group.iter().map(|m| m.1).max().unwrap_or(0) as i64;
                        let (bottom, top) = (group.iter().map(|m| m.2).min().unwrap_or(0) as i64, group.iter().map(|m| m.2).max().unwrap_or(0) as i64);
                        let ds = (snap(tick_at(p.x) - dt, g) as i64 - s0 as i64).clamp(-lowest, (length as i64 - 1 - latest).max(-lowest));
                        let dkey = (key_at(p.y) as i64 - dk as i64 - k0 as i64).clamp(-bottom, 127 - top);
                        for (k, s, key) in group {
                            let (start, key) = ((s as i64 + ds) as u32, fold((key as i64 + dkey) as u8));
                            if notes.get(k).is_some_and(|n| (n.start, n.key) != (start, key)) {
                                self.run_merge("note.set", json!({ "index": k, "start": start, "key": key }), "note-drag");
                            }
                        }
                        let key = (k0 as i64 + dkey) as u8;
                        if key != n.key {
                            preview_key = Some(key);
                        }
                    }
                    Grab::Move(dt, dk) => {
                        let start = snap(tick_at(p.x) - dt, g);
                        let key = fold((key_at(p.y) as i32 - dk).clamp(0, 127) as u8);
                        if (start, key) != (n.start, n.key) {
                            self.run_merge("note.set", json!({ "index": i, "start": start, "key": key }), "note-drag");
                            if key != n.key {
                                preview_key = Some(key);
                            }
                        }
                    }
                    Grab::Resize => {
                        let end = (((tick_at(p.x) / g as f64).round() as u32) * g).max(n.start + g);
                        if end - n.start != n.length {
                            self.run_merge("note.set", json!({ "index": i, "length": end - n.start }), "note-drag");
                            self.piano.last_len = end - n.start;
                        }
                    }
                }
            }
            if resp.hovered() {
                let zoom = ui.input(|i| i.zoom_delta()); // Ctrl+wheel or pinch
                if zoom != 1.0 {
                    self.piano.zoom = (self.piano.zoom * zoom).clamp(16.0, 800.0);
                }
                // Alt+wheel: row height (and the scroll area doesn't scroll)
                let (alt, wheel) = ui.input(|i| (i.modifiers.alt, i.smooth_scroll_delta.y + i.smooth_scroll_delta.x));
                if alt && wheel != 0.0 {
                    self.piano.row = (self.piano.row * (1.0 + wheel * 0.004)).clamp(6.0, 36.0);
                    ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
                }
            }
        }
        // Ctrl-drag box: everything it touches joins the selection
        if let Some((from, keep)) = self.piano.boxing.clone()
            && let Some(p) = pointer
        {
            let area = Rect::from_two_pos(from, p);
            painter.rect_filled(area, CornerRadius::same(t.radius_sm), t.select.gamma_multiply(0.12));
            painter.rect_stroke(area, CornerRadius::same(t.radius_sm), Stroke::new(1.0, t.select), StrokeKind::Inside);
            let mut chosen = keep;
            chosen.extend(notes.iter().enumerate().filter(|(_, n)| note_rect(n).intersects(area)).map(|(k, _)| k));
            self.piano.selected = chosen;
        }
        // the right button on the selection: its menu (it doesn't erase)
        if ui.input(|i| i.pointer.secondary_pressed())
            && let Some((i, _)) = pointer.and_then(hit)
            && self.piano.selected.contains(&i)
        {
            self.piano.sel_menu = true;
        }
        if self.piano.sel_menu {
            let n = self.piano.selected.len();
            let (mut delete, mut all, mut none) = (false, false, false);
            let open = resp
                .context_menu(|ui| {
                    let what = if n == 1 { tr!("Delete 1 note").to_string() } else { tr!("Delete {} notes", n) };
                    if ui.add(egui::Button::new(format!("{}  {what}", icons::TRASH)).shortcut_text(tr!("Del"))).clicked() {
                        delete = true;
                        ui.close();
                    }
                    if ui.add(egui::Button::new(tr!("Select all")).shortcut_text("Ctrl+A")).clicked() {
                        all = true;
                        ui.close();
                    }
                    if ui.add(egui::Button::new(tr!("Deselect")).shortcut_text("Esc")).clicked() {
                        none = true;
                        ui.close();
                    }
                })
                .is_some();
            if delete {
                self.delete_selected_notes();
            }
            if all {
                self.piano.selected = (0..notes.len()).collect();
            }
            if none {
                self.piano.selected.clear();
            }
            if !open && !ui.input(|i| i.pointer.secondary_down() || i.pointer.secondary_released()) {
                self.piano.sel_menu = false;
            }
        }
        // keys over the piano roll: Delete, Ctrl+A, Esc, the clipboard, Ctrl+D, arrows
        if resp.contains_pointer() && !ui.ctx().egui_wants_keyboard_input() {
            if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
                self.piano.selected = (0..notes.len()).collect();
            }
            let (copy, cut, paste) = ui.input(|i| {
                let has = |f: fn(&egui::Event) -> bool| i.events.iter().any(f);
                (has(|e| matches!(e, egui::Event::Copy)), has(|e| matches!(e, egui::Event::Cut)), has(|e| matches!(e, egui::Event::Paste(_))))
            });
            let chosen: Vec<ravune_core::Note> = self.piano.selected.iter().filter_map(|&i| notes.get(i).copied()).collect();
            if (copy || cut) && !chosen.is_empty() {
                self.piano.clipboard = chosen.clone();
                if cut {
                    self.delete_selected_notes();
                }
            } else if paste && !self.piano.clipboard.is_empty() {
                // where the pointer is, on the grid
                let at = ui.input(|i| i.pointer.hover_pos()).map_or(0, |p| snap(tick_at(p.x), g));
                self.add_notes(self.piano.clipboard.clone(), at as i64, notes.len());
            } else if !chosen.is_empty() {
                let shift = ui.input(|i| i.modifiers.shift);
                if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace)) {
                    self.delete_selected_notes();
                } else if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                    self.piano.selected.clear();
                } else if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::D)) {
                    // the copy right after the selection, its span rounded up to the grid
                    let from = chosen.iter().map(|n| n.start).min().unwrap_or(0);
                    let span = chosen.iter().map(|n| n.end()).max().unwrap_or(0) - from;
                    self.add_notes(chosen, (from + span.div_ceil(g).max(1) * g) as i64, notes.len());
                } else {
                    // arrows: a grid step or a semitone (Shift: an octave, or ←/→ the length)
                    let moves = [(Key::ArrowLeft, -1i64, 0i64), (Key::ArrowRight, 1, 0), (Key::ArrowUp, 0, 1), (Key::ArrowDown, 0, -1)];
                    let pressed = moves.into_iter().find(|(key, ..)| ui.input_mut(|i| i.consume_key(if shift { Modifiers::SHIFT } else { Modifiers::NONE }, *key)));
                    if let Some((_, dx, dy)) = pressed {
                        self.nudge_notes(&notes, dx, dy, shift, g, length);
                    }
                }
            }
        }
        // right button held: the mouse is an eraser, whatever it passes over goes
        // the right button erases with any tool; the eraser tool erases with the left one too
        let erasing = ui.input(|i| i.pointer.secondary_down()) && !self.piano.sel_menu
            || self.piano.tool == Tool::Erase && resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
        if erasing && let Some(p) = ui.input(|i| i.pointer.hover_pos()).filter(|p| visible.contains(*p) && rect.contains(*p) && p.x > keys.right()) {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
            painter.circle_stroke(p, 6.0, Stroke::new(1.5, t.meter_hot));
            if let Some((i, _)) = hit(p) {
                self.run_merge("note.remove", json!({ "index": i }), "erase");
                self.piano.selected.clear(); // the indices moved
            }
        }
        if ui.input(|i| i.pointer.secondary_released()) || self.piano.tool == Tool::Erase && ui.input(|i| i.pointer.primary_released()) {
            self.session.close_merge();
        }
        if ui.input(|i| i.pointer.primary_released()) && self.piano.paint.take().is_some() {
            self.session.close_merge();
        }
        // the tool next to the pointer, and for the scissors where the cut would go
        if let Some(p) = ui.input(|i| i.pointer.hover_pos()).filter(|p| visible.contains(*p) && rect.contains(*p) && p.x > keys.right())
            && self.piano.tool != Tool::Draw
        {
            if self.piano.tool == Tool::Slice
                && let Some((i, _)) = hit(p)
            {
                let x = x_of(snap(tick_at(p.x), g.max(1)) as f64);
                let r = note_rect(&notes[i]);
                painter.line_segment([pos2(x, r.top() - 2.0), pos2(x, r.bottom() + 2.0)], Stroke::new(1.5, t.fg_text));
            }
            ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("piano-tool"))).text(p + vec2(12.0, 12.0), Align2::LEFT_TOP, self.piano.tool.icon(), egui::FontId::proportional(15.0), t.fg_text);
        }
        // audition: one note at a time, while the mouse is down
        if let Some(key) = preview_key {
            if let Some(old) = self.piano.preview.replace(key) {
                self.note_off(old);
            }
            self.note_on(key);
        }
        if ui.input(|i| i.pointer.primary_released()) {
            if let Some(old) = self.piano.preview.take() {
                self.note_off(old);
            }
            if self.piano.drag.take().is_some() {
                self.session.close_merge();
            }
            self.piano.group.clear();
            self.piano.boxing = None;
        }
        self.hint(resp, tr!("Piano roll: click to draw · drag to move · right edge to resize (snap follows the zoom, Alt = free) · hold the right button to erase · Ctrl+drag selects (then drag them together, right-click or Del to delete) · arrows move it (Shift: an octave, or the length) · Ctrl+C / X / V / D · Ctrl+wheel zoom · Alt+wheel row height."));
    }

    /// Per-note values under the grid (velocity, fine pitch, brightness), following its
    /// horizontal scroll. The names on the left pick which.
    fn velocity_lane(&mut self, ui: &mut Ui, t: &Theme, ppt: f32, slot: u8, scroll_x: f32, left: f32) {
        let (lane, resp) = ui.allocate_exact_size(vec2(ui.available_width(), VEL_H), Sense::click_and_drag());
        let painter = ui.painter_at(lane);
        painter.rect_filled(lane, CornerRadius::ZERO, t.bg_field);
        // the lane picker, in the keyboard's column: the notes' values, then the knobs
        // moving inside the pattern
        let row = |k: usize| egui::Rect::from_min_size(lane.left_top() + vec2(2.0, 2.0 + k as f32 * 15.0), vec2(KEYS - 4.0, 14.0));
        let pick_row = |ui: &mut Ui, k: usize, label: &str, on: bool| {
            let r = row(k);
            let pick = ui.interact(r, ui.scope_id().with(("lane", k)), Sense::click());
            if on || pick.hovered() {
                painter.rect_filled(r, CornerRadius::same(t.radius_sm), if on { Theme::mix(t.bg_field, t.accent, 0.25) } else { t.bg_control });
            }
            painter.text(r.center(), Align2::CENTER_CENTER, label, Theme::mono(8.0), if on { t.accent } else { t.fg_muted });
            pick
        };
        for (k, (which, label, help)) in Lane::ALL.into_iter().enumerate() {
            let pick = pick_row(ui, k, crate::i18n::t(label), self.piano.auto.is_none() && self.piano.lane == which);
            if pick.clicked() {
                (self.piano.lane, self.piano.auto) = (which, None);
            }
            self.hint(&pick, crate::i18n::t(help));
        }
        let pick = pick_row(ui, Lane::ALL.len(), "AUTO", self.piano.auto.is_some());
        let lines: Vec<(Target, String)> = self.session.current_pattern().automation.iter().map(|a| (a.target, crate::mixer::target_name(&self.session.project, a.target))).collect();
        kit::menu(t, &pick).show(|ui| {
            ui.label(RichText::new(tr!("Knobs moving in this pattern")).small());
            for (target, name) in &lines {
                if ui.selectable_label(self.piano.auto == Some(*target), name).clicked() {
                    self.piano.auto = Some(*target);
                    ui.close();
                }
            }
            if lines.is_empty() {
                ui.label(RichText::new(tr!("None yet: right-click a knob → Automate in this pattern")).small().color(t.fg_muted));
            }
        });
        self.hint(&pick, tr!("Knobs moving inside this pattern, wherever it plays: pick one to draw it here. Right-click any knob → Automate in this pattern."));
        if let Some(target) = self.piano.auto {
            self.pattern_line(ui, t, lane, &resp, ppt, scroll_x, left, target);
            return;
        }
        let which = self.piano.lane;
        let x_of = |tick: u32| left + KEYS + tick as f32 * ppt - scroll_x;
        let y_of = |v: f32| lane.bottom() - 3.0 - (lane.height() - 6.0) * v;
        let notes = self.session.current_pattern().notes(slot).to_vec();
        let (_, rest) = notes.first().map_or((0.0, 0.0), |n| which.get(n));
        if rest > 0.0 {
            painter.line_segment([pos2(lane.left() + KEYS, y_of(rest)), pos2(lane.right(), y_of(rest))], Stroke::new(1.0, t.fg_muted.gamma_multiply(0.4)));
        }
        for n in &notes {
            let x = x_of(n.start);
            if x < lane.left() + KEYS {
                continue;
            }
            let (v, rest) = which.get(n);
            painter.line_segment([pos2(x, y_of(rest)), pos2(x, y_of(v))], Stroke::new(2.0, t.accent));
            painter.circle_filled(pos2(x, y_of(v)), 2.5, t.accent);
        }
        if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.is_pointer_button_down_on()) {
            let nearest = notes.iter().enumerate().map(|(i, n)| (i, (x_of(n.start) - p.x).abs())).filter(|(_, d)| *d < 6.0).min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((i, _)) = nearest {
                let v = ((lane.bottom() - 3.0 - p.y) / (lane.height() - 6.0)).clamp(0.0, 1.0);
                self.run_merge("note.set", which.set(i, v), "lane");
            }
        }
        if resp.drag_stopped() || resp.clicked() {
            self.session.close_merge();
        }
        if let Some(n) = resp.hover_pos().and_then(|p| notes.iter().min_by(|a, b| (x_of(a.start) - p.x).abs().total_cmp(&(x_of(b.start) - p.x).abs())).filter(|n| (x_of(n.start) - p.x).abs() < 6.0)) {
            painter.text(lane.right_top() + vec2(-6.0, 3.0), Align2::RIGHT_TOP, which.describe(n), Theme::mono(9.0), t.fg_text);
        }
        self.hint(&resp, tr!("Note lane: drag a bar up or down to change that note. VEL / PITCH / BRIGHT / AUTO on the left pick what it edits."));
    }

    /// Deletes the selected notes, as one undo step.
    /// Adds `new` (as copied, starts kept apart) with the first one at `at`, as one undo step,
    /// and selects them. `count` is how many notes the channel has now.
    fn add_notes(&mut self, new: Vec<ravune_core::Note>, at: i64, count: usize) {
        let from = new.iter().map(|n| n.start).min().unwrap_or(0) as i64;
        self.session.close_merge();
        for n in &new {
            let start = (n.start as i64 - from + at).max(0);
            self.run_merge("note.add", json!({ "key": n.key, "start": start, "length": n.length, "velocity": n.velocity, "fine": n.fine, "bright": n.bright }), "paste-notes");
        }
        self.session.close_merge();
        self.piano.selected = (count..count + new.len()).collect();
    }

    /// The selection a grid step left / right (`dx`) or a semitone up / down (`dy`); with
    /// `shift`, an octave, or ←/→ shorter / longer by a step. Nothing moves past an edge.
    fn nudge_notes(&mut self, notes: &[ravune_core::Note], dx: i64, dy: i64, shift: bool, g: u32, length: u32) {
        let chosen: Vec<(usize, ravune_core::Note)> = self.piano.selected.iter().filter_map(|&i| notes.get(i).map(|n| (i, *n))).collect();
        let (g, octave) = (g as i64, if shift { 12 } else { 1 });
        let lowest = chosen.iter().map(|(_, n)| n.start as i64).min().unwrap_or(0);
        let latest = chosen.iter().map(|(_, n)| n.start as i64).max().unwrap_or(0);
        let (bottom, top) = (chosen.iter().map(|(_, n)| n.key as i64).min().unwrap_or(0), chosen.iter().map(|(_, n)| n.key as i64).max().unwrap_or(0));
        let dt = (dx * g).clamp(-lowest, (length as i64 - 1 - latest).max(-lowest));
        let dk = (dy * octave).clamp(-bottom, 127 - top);
        self.session.close_merge();
        for (i, n) in chosen {
            let props = if shift && dx != 0 {
                json!({ "index": i, "length": (n.length as i64 + dx * g).max(g) })
            } else {
                json!({ "index": i, "start": n.start as i64 + dt, "key": n.key as i64 + dk })
            };
            self.run_merge("note.set", props, "nudge-notes");
        }
        self.session.close_merge();
    }

    fn delete_selected_notes(&mut self) {
        let mut chosen: Vec<usize> = std::mem::take(&mut self.piano.selected).into_iter().collect();
        chosen.sort_unstable_by(|a, b| b.cmp(a)); // from the end, so the others keep their place
        self.session.close_merge();
        for i in chosen {
            self.run_merge("note.remove", json!({ "index": i }), "delete-notes");
        }
        self.session.close_merge();
    }

    /// A knob's line inside the current pattern, under the grid: click to add a point, drag
    /// it (Alt: off the grid), right-click to delete it.
    #[allow(clippy::too_many_arguments)]
    fn pattern_line(&mut self, ui: &mut Ui, t: &Theme, lane: Rect, resp: &Response, ppt: f32, scroll_x: f32, left: f32, target: Target) {
        let pattern = self.session.current_pattern();
        let length = pattern.ticks();
        let (points, curves) = pattern.automation.iter().find(|a| a.target == target).map_or((Vec::new(), Vec::new()), |a| (a.points.clone(), a.curves.clone()));
        let name = crate::mixer::target_name(&self.session.project, target);
        let x_of = |tick: f64| left + KEYS + tick as f32 * ppt - scroll_x;
        let tick_at = |x: f32| ((x - left - KEYS + scroll_x) / ppt).max(0.0) as f64;
        let y_of = |v: f32| lane.bottom() - 3.0 - (lane.height() - 6.0) * v;
        let v_at = |y: f32| ((lane.bottom() - 3.0 - y) / (lane.height() - 6.0)).clamp(0.0, 1.0);
        let painter = ui.painter_at(Rect::from_min_max(pos2(lane.left() + KEYS, lane.top()), lane.max));
        // the grid: beats down, quarters across; the pattern's end
        let beat = PPQ as f64;
        for b in 0..=(length as f64 / beat) as u32 {
            let x = x_of(b as f64 * beat);
            painter.line_segment([pos2(x, lane.top()), pos2(x, lane.bottom())], Stroke::new(1.0, if b % 4 == 0 { t.line } else { Theme::mix(t.bg_field, t.line, 0.4) }));
        }
        kit::quarter_lines(&painter, egui::Rangef::new(x_of(0.0), x_of(length as f64)), y_of, t.line);
        painter.line_segment([pos2(x_of(length as f64), lane.top()), pos2(x_of(length as f64), lane.bottom())], Stroke::new(1.0, t.line));
        if points.is_empty() {
            painter.text(pos2(lane.left() + KEYS + 8.0, lane.center().y), Align2::LEFT_CENTER, tr!("Click to draw {} in this pattern", name), egui::FontId::proportional(11.0), t.fg_muted);
        } else {
            let line: Vec<Pos2> = curve_samples(&points, &curves, 0.0, length as f64).into_iter().map(|(tk, v)| pos2(x_of(tk), y_of(v))).collect();
            painter.add(Shape::line(line, Stroke::new(1.5, t.accent)));
        }
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        let near = |p: Pos2| points.iter().position(|(tk, v)| pos2(x_of(*tk as f64), y_of(*v)).distance(p) < 7.0);
        let spots: Vec<(usize, Pos2)> = bend_spots(&points, &curves).into_iter().map(|(k, mid, v)| (k, pos2(x_of(mid), y_of(v)))).collect();
        let near_bend = |p: Pos2| spots.iter().find(|(_, at)| at.distance(p) < 7.0).map(|(k, _)| *k);
        let hovered = pointer.and_then(near);
        let hovered_bend = pointer.filter(|_| hovered.is_none()).and_then(near_bend);
        for (k, spot) in &spots {
            kit::bend_handle(&painter, *spot, t.accent, hovered_bend == Some(*k) || self.piano.auto_bend.is_some_and(|b| b.0 == *k));
        }
        for (k, (tk, v)) in points.iter().enumerate() {
            let big = hovered == Some(k) || self.piano.auto_drag == Some(k);
            painter.circle_filled(pos2(x_of(*tk as f64), y_of(*v)), if big { 4.5 } else { 3.0 }, t.accent);
        }
        let grid = self.piano.grid(ui).max(1);
        let snap = |tick: f64| ((tick / grid as f64).round() as u32 * grid).min(length);
        let set = |points: Vec<(u32, f32)>, curves: Vec<f32>| json!({ "target": target, "points": points, "curves": curves });
        // a new point where the mouse is, in its place among the others
        let with_point = |p: Pos2| {
            let (mut pts, mut bends) = (points.clone(), curves.clone());
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
                self.piano.auto_drag = Some(k);
            } else if let Some(k) = near_bend(p) {
                self.piano.auto_bend = Some((k, curves.get(k).copied().unwrap_or(0.0), p.y));
            } else {
                let (k, pts, bends) = with_point(p);
                self.run_merge("pattern.automation", set(pts, bends), "pattern-line");
                self.piano.auto_drag = Some(k);
            }
        } else if resp.dragged()
            && let (Some((k, bent, y0)), Some(p)) = (self.piano.auto_bend, pointer)
            && k + 1 < points.len()
        {
            // up bows the stretch up, down bows it down
            let mut bends = curves.clone();
            bends.resize(points.len() - 1, 0.0);
            bends[k] = (bent + (y0 - p.y) / 60.0).clamp(-1.0, 1.0);
            self.run_merge("pattern.automation", set(points.clone(), bends), "pattern-bend");
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        } else if resp.double_clicked()
            && let Some(k) = pointer.and_then(near_bend)
            && k < curves.len()
        {
            let mut bends = curves.clone();
            bends[k] = 0.0;
            self.run("pattern.automation", set(points.clone(), bends));
        } else if resp.dragged()
            && let (Some(k), Some(p)) = (self.piano.auto_drag, pointer)
            && k < points.len()
        {
            // a point stays between its neighbours
            let lo = if k > 0 { points[k - 1].0 + 1 } else { 0 };
            let hi = points.get(k + 1).map_or(length, |n| n.0.saturating_sub(1)).max(lo);
            let mut pts = points.clone();
            pts[k] = (snap(tick_at(p.x)).clamp(lo, hi), v_at(p.y));
            if pts[k] != points[k] {
                self.run_merge("pattern.automation", set(pts, curves.clone()), "pattern-line");
            }
        } else if resp.clicked()
            && let Some(p) = pointer
            && near(p).is_none()
            && near_bend(p).is_none()
        {
            let (_, pts, bends) = with_point(p);
            self.run("pattern.automation", set(pts, bends));
        }
        if resp.secondary_clicked()
            && let Some(k) = pointer.and_then(near)
        {
            if points.len() > 1 {
                let (mut pts, mut bends) = (points.clone(), curves.clone());
                remove_point(&mut pts, &mut bends, k);
                self.run("pattern.automation", set(pts, bends));
            } else {
                // the last point: the line goes
                self.run("pattern.automation", json!({ "target": target, "remove": true }));
            }
        }
        if hovered_bend.is_some() && self.piano.auto_drag.is_none() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
        if resp.drag_stopped() {
            (self.piano.auto_drag, self.piano.auto_bend) = (None, None);
            self.session.close_merge();
        }
        if let Some(p) = pointer.filter(|p| lane.contains(*p) && p.x > lane.left() + KEYS) {
            let value = hovered.map_or_else(|| curve_at(&points, &curves, tick_at(p.x)).unwrap_or(0.5), |k| points[k].1);
            painter.text(lane.right_top() + vec2(-6.0, 3.0), Align2::RIGHT_TOP, format!("{name} · {}", crate::mixer::describe_target(&self.session.project, target, value)), Theme::mono(9.0), t.fg_text);
        }
        self.hint(resp, tr!("{}, inside this pattern: click to add a point, drag it (Alt: off the grid), right-click to delete it, drag a ring to bend the line. It plays wherever the pattern does.", name));
    }
}
