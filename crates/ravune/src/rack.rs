//! Channel rack (FL-style step sequencer) and the device view of the selected channel.

use eframe::egui::{self, Align2, CornerRadius, FontId, Frame, Id, Key, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use ravune_core::{Curve, DEFAULT_VELOCITY, INSTRUMENT_EXTENSIONS, Instrument, MAX_STEPS, Note, PATTERN_COLORS, Param, ParamInfo, STEP, Target};
use serde_json::json;

use crate::browser::{AUDIO_EXTENSIONS, PluginDrag, SampleDrag};
use crate::clipprops::Cut;
use crate::shell::{Rename, Renaming};
use crate::theme::Theme;
use crate::{App, icons, kit, widgets};

const ROW: f32 = 24.0;
const NAME_W: f32 = 110.0;
/// Mute, name and volume: the part of a row that stays put.
const HEAD_W: f32 = 40.0 + NAME_W + 6.0 + 40.0 + 4.0;
/// One step, its gap included.
const STEP_W: f32 = 22.0;

// Sampler controls reuse the knob widget, so they get ParamInfo descriptions like synth params.
const PITCH: ParamInfo = ParamInfo {
    name: "Pitch",
    min: -24.0,
    max: 24.0,
    default: 0.0,
    curve: Curve::Linear,
    unit: "semi",
    help: "Transpose in semitones and cents (a sample also gets shorter or longer). Ctrl: whole semitones, Shift: finer.",
};
pub const VOLUME: ParamInfo = ParamInfo {
    name: "Volume",
    min: 0.0,
    max: 1.0,
    default: 0.8,
    curve: Curve::Linear,
    unit: "%",
    help: "Channel volume.",
};
const CHOKE: ParamInfo = ParamInfo {
    name: "Choke",
    min: 0.0,
    max: 8.0,
    default: 0.0,
    curve: Curve::Steps,
    unit: "grp",
    help: "Channels in the same choke group cut each other off, like open and closed hi-hats. 0 = off.",
};

const ATTACK: ParamInfo = ParamInfo {
    name: "Attack",
    min: 0.0,
    max: 1.0,
    default: 0.0,
    curve: Curve::Linear,
    unit: "s",
    help: "Fade-in at each hit: softens the click, or swells it in.",
};
const RELEASE: ParamInfo = ParamInfo {
    name: "Release",
    min: 0.0,
    max: 2.0,
    default: 0.0,
    curve: Curve::Linear,
    unit: "s",
    help: "0: every hit plays to its end. Above: a hit lasts as long as its note, then fades over this time.",
};

/// Small caps section label.
pub fn label(ui: &mut Ui, t: &Theme, text: &str) {
    ui.label(RichText::new(text).font(Theme::mono(10.0)).extra_letter_spacing(1.0).color(t.fg_muted));
}

impl App {
    pub fn channel_rack(&mut self, ui: &mut Ui, now_step: Option<usize>) {
        let t = self.theme.clone();
        // first steps, in the status bar while the mouse is over the rack, until something has played
        if !self.played {
            self.zone_hint(ui, tr!("Click steps to make a beat · Space plays · A–K play the selected channel"));
        }
        kit::bar(ui, &t, |ui| {
            let current = self.session.pattern;
            let list: Vec<(String, Option<[u8; 3]>)> = self.session.project.patterns.iter().map(|p| (p.name.clone(), p.color)).collect();
            let combo = self.pattern_picker(ui, &t, "pattern");
            self.hint(&combo, tr!("The pattern shown here and in the piano roll (all of them are in the browser's Patterns tab)."));
            for (icon, op, help) in [
                (icons::PLUS, "pattern.add", tr!("New empty pattern (F4)")),
                (icons::COPY, "pattern.duplicate", tr!("Clone this pattern")),
            ] {
                if kit::icon_button(ui, &t, icon, help, false).clicked() {
                    self.run(op, json!({}));
                }
            }
            if kit::icon_button(ui, &t, icons::PENCIL_SIMPLE, tr!("Rename this pattern"), false).clicked() {
                self.rename = Some(Renaming::new(Rename::Pattern(current), list[current].0.clone()));
            }
            let can_delete = list.len() > 1;
            if ui.add_enabled_ui(can_delete, |ui| kit::icon_button(ui, &t, icons::TRASH, tr!("Delete this pattern"), false)).inner.clicked() {
                self.run("pattern.remove", json!({}));
            }
            kit::vsep(ui, &t);
            let mut length = self.session.current_pattern().length;
            let r = ui.add(egui::DragValue::new(&mut length).range(1..=MAX_STEPS).speed(0.25).suffix(tr!(" steps")));
            if r.changed() {
                self.run_merge("pattern.set_length", json!({ "steps": length }), "length");
            }
            if r.drag_stopped() || r.lost_focus() {
                self.session.close_merge();
            }
            r.context_menu(|ui| {
                for (n, label) in [(16, tr!("1 bar")), (32, tr!("2 bars")), (64, tr!("4 bars")), (128, tr!("8 bars"))] {
                    if ui.selectable_label(length == n, tr!("{} steps · {}", n, label)).clicked() {
                        self.run("pattern.set_length", json!({ "steps": n }));
                        ui.close();
                    }
                }
            });
            self.hint(&r, tr!("Pattern length in 16th steps (16 = one bar): drag, type any length, or right-click for bars. Clips one pattern long follow it."));
            let mut swing = self.session.project.swing * 100.0;
            let r = ui.add(egui::DragValue::new(&mut swing).range(0.0..=100.0).speed(0.5).fixed_decimals(0).suffix("% swing"));
            if r.changed() {
                self.run_merge("transport.set_swing", json!({ "amount": swing / 100.0 }), "swing");
            }
            if r.drag_stopped() || r.lost_focus() {
                self.session.close_merge();
            }
            self.hint(&r, tr!("Swing pushes every second 16th late for a shuffled groove."));
            kit::right(ui, |ui| {
                self.inspector_toggle(ui, &t);
                if kit::icon_button(ui, &t, icons::ARROW_COUNTER_CLOCKWISE, tr!("Clear every step and note (Ctrl Z brings them back)"), false).clicked() {
                    self.run("pattern.clear", json!({}));
                }
                if kit::button(ui, &t, Some(icons::PLUS), "VA-1", false).clicked() {
                    self.run("channel.add", json!({}));
                }
            });
        });

        self.inspector(ui, "rack");
        // the channels and their steps, no card around them: the whole area scrolls both ways,
        // so a long pattern or a narrow panel never pushes it under the sound's panel
        let n = self.session.project.channels.len();
        let area = egui::ScrollArea::both().id_salt("rack").auto_shrink(false).show(ui, |ui| {
            if n == 0 {
                kit::padded(ui, &t, |ui| ui.label(RichText::new(tr!("Drop sounds here from the browser, or add a VA-1.")).color(t.fg_muted)));
                return;
            }
            const GAP: f32 = 4.0;
            let rows = ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing = vec2(t.gap, GAP);
                ui.vertical(|ui| (0..n).for_each(|i| self.rack_head(ui, &t, i)));
                ui.vertical(|ui| (0..n).for_each(|i| self.rack_steps(ui, &t, i, now_step)));
            });
            // a divider under each channel, across the names and the steps
            let (rect, x) = (rows.response.rect, rows.response.rect.left()..=ui.max_rect().right().max(rows.response.rect.right()));
            for i in 1..=n {
                ui.painter().hline(x.clone(), rect.top() + i as f32 * (ROW + GAP) - GAP / 2.0, Stroke::new(1.0, t.line));
            }
        });
        // a sound dropped on a channel's row replaces its instrument (the row takes it
        // first); anywhere else in the rack it becomes a new channel
        let zone = ui.interact(area.inner_rect, ui.scope_id().with("rack-drop"), Sense::hover());
        if kit::drag_over::<SampleDrag>(&zone).is_some() {
            ui.painter().rect_stroke(zone.rect, CornerRadius::same(t.radius_md), Stroke::new(1.0, t.select.gamma_multiply(0.5)), StrokeKind::Inside);
        }
        if let Some(drag) = kit::dropped::<SampleDrag>(&zone) {
            self.run("channel.add", json!({ "sample": drag.0 }));
        }
        // plugins too: instruments become channels, effects go on the selected channel's track
        if kit::drag_over::<PluginDrag>(&zone).is_some() {
            ui.painter().rect_stroke(zone.rect, CornerRadius::same(t.radius_md), Stroke::new(2.0, t.select), StrokeKind::Inside);
        }
        if let Some(drag) = kit::dropped::<PluginDrag>(&zone) {
            self.drop_plugin_on_rack(&drag.0);
        }
    }

    /// A plugin or a sound held over a channel's row: it lights up, and a drop replaces that
    /// channel's instrument (its notes stay).
    fn drop_on_row(&mut self, ui: &Ui, t: &Theme, over: &egui::Response, i: usize) {
        let plugin = kit::drag_over::<PluginDrag>(over);
        if plugin.is_some() || kit::drag_over::<SampleDrag>(over).is_some() {
            let effect = plugin.is_some_and(|p| !p.0.instrument);
            ui.painter().rect_stroke(over.rect, CornerRadius::same(t.radius_sm), Stroke::new(2.0, if effect { t.accent } else { t.select }), StrokeKind::Inside);
        }
        if let Some(p) = kit::dropped::<PluginDrag>(over) {
            self.replace_instrument(i, Some(&p.0), None);
        } else if let Some(s) = kit::dropped::<SampleDrag>(over) {
            self.replace_instrument(i, None, Some(&s.0));
        }
    }

    /// The current pattern: its color chip (a menu to change it) and a list to pick another,
    /// in the rack's bar and the playlist's. `salt` keeps the two apart.
    pub(crate) fn pattern_picker(&mut self, ui: &mut Ui, t: &Theme, salt: &str) -> egui::Response {
        let current = self.session.pattern;
        let list: Vec<(String, Option<[u8; 3]>)> = self.session.project.patterns.iter().map(|p| (p.name.clone(), p.color)).collect();
        let r = kit::swatch(ui, t, Theme::pattern(current, list[current].1), 18.0, tr!("Pattern color"));
        kit::menu(t, &r).show(|ui| self.pattern_color_menu(ui, current));
        egui::ComboBox::from_id_salt(salt)
            .selected_text(format!("{}  {}", icons::SQUARES_FOUR, list[current].0))
            .width(150.0)
            .show_ui(ui, |ui| {
                for (i, (n, color)) in list.iter().enumerate() {
                    let text = RichText::new(format!("{}  {n}", icons::SQUARES_FOUR)).color(Theme::pattern(i, *color));
                    if ui.selectable_label(i == current, text).clicked() {
                        self.run("pattern.select", json!({ "pattern": i }));
                    }
                }
            })
            .response
    }

    /// Palette, custom color and automatic, for a pattern's color chip.
    pub fn pattern_color_menu(&mut self, ui: &mut Ui, pattern: usize) {
        let t = self.theme.clone();
        let Some(color) = self.session.project.patterns.get(pattern).map(|p| p.color) else { return };
        for row in PATTERN_COLORS.chunks(6) {
            ui.horizontal(|ui| {
                for c in row {
                    if kit::swatch(ui, &t, Theme::pattern(0, Some(*c)), 20.0, tr!("Use this color")).clicked() {
                        self.run("pattern.set_color", json!({ "pattern": pattern, "color": c }));
                        ui.close();
                    }
                }
            });
        }
        ui.horizontal(|ui| {
            let mut rgb = color.unwrap_or(PATTERN_COLORS[pattern % PATTERN_COLORS.len()]);
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                self.run_merge("pattern.set_color", json!({ "pattern": pattern, "color": rgb }), "pattern-color");
            }
            ui.label(tr!("Custom"));
        });
        if ui.button(tr!("Automatic")).clicked() {
            self.run("pattern.set_color", json!({ "pattern": pattern, "color": null }));
            ui.close();
        }
    }

    /// Where a playing pattern is, in ticks (none when stopped, in song mode or with reduce
    /// motion): what the flashes below follow.
    fn pattern_tick(&self) -> Option<f64> {
        let beat = self.beat.filter(|_| !self.session.song_mode && !self.settings.reduce_motion)?;
        Some((beat * ravune_core::PPQ as f64).rem_euclid(self.session.current_pattern().length as f64 * STEP as f64))
    }

    /// How hard channel `i` hits right now: its velocity the moment a note starts, fading out
    /// over a 16th and a half.
    fn channel_hit(&self, i: usize) -> f32 {
        let (Some(tick), Some(slot)) = (self.pattern_tick(), self.session.project.channels.get(i).map(|c| c.slot)) else { return 0.0 };
        self.session.current_pattern().notes(slot).iter().map(|n| flash(tick, n.start) * n.velocity as f32 / 127.0).fold(0.0, f32::max)
    }

    /// Mute, name and volume of a channel: the fixed left part of its row.
    pub(crate) fn rack_head(&mut self, ui: &mut Ui, t: &Theme, i: usize) {
        // an earlier row's menu may have removed a channel this frame
        let Some(ch) = self.session.project.channels.get(i) else { return };
        // read now: this row's own menu can remove the channel before the knob below is drawn
        let (name, mute, gain, choke, slot) = (ch.name.clone(), ch.mute, ch.gain, ch.choke, ch.slot);
        let selected = self.session.selected == i;
        let (row, over) = ui.allocate_exact_size(vec2(HEAD_W, ROW), Sense::hover());
        let painter = ui.painter_at(row.expand(1.0));
        self.drop_on_row(ui, t, &over, i);
        if selected {
            painter.rect_filled(row, CornerRadius::same(t.radius_sm), Theme::mix(t.bg_panel, t.fg_text, 0.06));
            painter.rect_filled(Rect::from_min_size(row.min, vec2(2.0, ROW)), CornerRadius::ZERO, t.accent);
        }

        // M and S, like the playlist's lanes (solo: every other channel goes quiet)
        let channels = &self.session.project.channels;
        let soloed = channels.len() > 1 && channels.iter().enumerate().all(|(k, c)| c.mute == (k != i));
        let r = kit::chip(ui, &painter, t, pos2(row.left() + 11.0, row.center().y), "M", mute, t.meter_warn, ui.scope_id().with(("mute", i)));
        if r.clicked() {
            self.run("channel.set", json!({ "channel": i, "mute": !mute }));
        }
        self.hint(&r, if mute { tr!("Muted: click to hear it again") } else { tr!("Mute this channel") });
        let r = kit::chip(ui, &painter, t, pos2(row.left() + 27.0, row.center().y), "S", soloed, t.select, ui.scope_id().with(("solo", i)));
        if r.clicked() {
            self.run("channel.solo", json!({ "channel": i }));
        }
        self.hint(&r, if soloed { tr!("Soloed: click to bring the others back") } else { tr!("Solo this channel: the others go quiet") });

        // name: click selects (keys + device follow), right-click for more
        let name_rect = Rect::from_min_size(pos2(row.left() + 40.0, row.top() + 2.0), vec2(NAME_W, ROW - 4.0));
        let r = ui.interact(name_rect, ui.scope_id().with(("name", i)), Sense::click());
        let fill = if r.hovered() { Theme::mix(t.bg_control, t.fg_text, 0.12) } else { t.bg_control };
        // it lights up as it plays
        let hit = self.channel_hit(i);
        if hit > 0.0 {
            painter.rect_filled(name_rect.expand(1.0 + 3.0 * hit), CornerRadius::same(t.radius_sm + 2), t.accent.gamma_multiply(0.3 * hit));
        }
        painter.rect_filled(name_rect, CornerRadius::same(t.radius_sm), Theme::mix(fill, t.accent, 0.6 * hit));
        let text_color = if selected { t.accent } else { t.fg_text };
        let cut = kit::fit_text(&painter, name_rect.left_center() + vec2(6.0, 0.0), Align2::LEFT_CENTER, name.as_str(), FontId::proportional(12.0), text_color, NAME_W - 10.0).1;
        if cut {
            r.clone().on_hover_text(name.as_str());
        }
        if r.clicked() {
            self.run("channel.select", json!({ "channel": i }));
        }
        if r.double_clicked() {
            // a plugin opens its own window; anything else, the piano roll
            match self.session.project.channels.get(i).map(|c| (c.slot, c.instrument.clone())) {
                Some((slot, Instrument::Plugin { plugin })) => self.toggle_plugin_editor(ravune_engine::PluginTarget::Channel(slot), &plugin.name),
                _ => self.tab = crate::Tab::PianoRoll,
            }
        }
        r.context_menu(|ui| {
            if ui.button(tr!("{}  Edit in the piano roll", icons::PIANO_KEYS)).clicked() {
                self.run("channel.select", json!({ "channel": i }));
                self.tab = crate::Tab::PianoRoll;
                ui.close();
            }
            if ui.button(tr!("{}  Instrument and effects", icons::FADERS)).clicked() {
                self.run("channel.select", json!({ "channel": i }));
                self.settings.inspector = true;
                ui.close();
            }
            ui.separator();
            ui.menu_button(tr!("{}  Fill each", icons::GRID_FOUR), |ui| {
                for (every, label) in [(1, "step"), (2, tr!("2 steps")), (4, tr!("4 steps (each beat)")), (8, tr!("8 steps")), (16, tr!("16 steps (each bar)"))] {
                    if ui.button(tr!("Each {}", label)).clicked() {
                        self.run("steps.fill", json!({ "channel": i, "every": every }));
                        ui.close();
                    }
                }
            });
            ui.menu_button(tr!("{}  Rotate", icons::ARROWS_LEFT_RIGHT), |ui| {
                for (by, label) in [(-1, tr!("One step left")), (1, tr!("One step right")), (-4, tr!("One beat left")), (4, tr!("One beat right"))] {
                    if ui.button(label).clicked() {
                        self.run("steps.rotate", json!({ "channel": i, "by": by }));
                        ui.close();
                    }
                }
            });
            if ui.button(tr!("{}  Clear steps and notes", icons::ARROW_COUNTER_CLOCKWISE)).clicked() {
                self.run("notes.clear", json!({ "channel": i }));
                ui.close();
            }
            ui.separator();
            let soloed = self.session.project.channels.iter().enumerate().all(|(k, c)| c.mute == (k != i));
            if ui.button(format!("{}  {}", icons::HEADPHONES, if soloed { tr!("Unsolo") } else { "Solo" })).clicked() {
                self.run("channel.solo", json!({ "channel": i }));
                ui.close();
            }
            let tracks: Vec<String> = self.session.project.mixer.iter().map(|m| m.name.clone()).collect();
            let track = self.session.project.channels.get(i).map_or(0, |c| c.track);
            ui.menu_button(tr!("{}  Plays into", icons::FADERS_HORIZONTAL), |ui| {
                for (k, name) in tracks.iter().enumerate() {
                    if ui.selectable_label(track == k, name).clicked() {
                        self.run("channel.set", json!({ "channel": i, "track": k }));
                        ui.close();
                    }
                }
            });
            ui.menu_button(tr!("Choke group"), |ui| {
                for g in 0..=4u8 {
                    let text = if g == 0 { tr!("None").to_string() } else { tr!("Group {}", g) };
                    if ui.selectable_label(choke == g, text).clicked() {
                        self.run("channel.set", json!({ "channel": i, "choke": g }));
                        ui.close();
                    }
                }
            });
            let (own, song) = (self.session.project.channels.get(i).and_then(|c| c.swing), self.session.project.swing);
            ui.menu_button(tr!("Swing"), |ui| {
                // a channel can swing more or less than the song: straight hats over a shuffled beat
                let song_label = tr!("The song's ({}%)", (song * 100.0).round());
                if ui.selectable_label(own.is_none(), song_label).clicked() {
                    self.run("channel.set", json!({ "channel": i, "swing": -1.0 }));
                    ui.close();
                }
                for amount in [0.0f32, 0.2, 0.4, 0.6, 0.8, 1.0] {
                    let text = if amount == 0.0 { tr!("Straight").to_string() } else { format!("{}%", (amount * 100.0).round()) };
                    if ui.selectable_label(own == Some(amount), text).clicked() {
                        self.run("channel.set", json!({ "channel": i, "swing": amount }));
                        ui.close();
                    }
                }
            });
            ui.separator();
            if ui.button(tr!("{}  Rename…", icons::PENCIL_SIMPLE)).clicked() {
                self.rename = Some(Renaming::new(Rename::Channel(i), name.clone()));
                ui.close();
            }
            if ui.button(tr!("{}  Clone", icons::COPY)).clicked() {
                self.run("channel.clone", json!({ "channel": i }));
                ui.close();
            }
            for (by, label, icon) in [(-1, tr!("Move up"), icons::CARET_UP), (1, tr!("Move down"), icons::CARET_DOWN)] {
                if ui.button(format!("{icon}  {label}")).clicked() {
                    self.run("channel.move", json!({ "channel": i, "by": by }));
                    ui.close();
                }
            }
            if ui.button(tr!("{}  Remove channel", icons::TRASH)).clicked() {
                self.run("channel.remove", json!({ "channel": i }));
                ui.close();
            }
        });
        self.hint(&r, tr!("{}: click to select (the keyboard plays it) · double-click opens the piano roll · right-click for options", name));

        // volume: a little knob where the bar was
        let (r, v) = widgets::mini_knob(ui, t, pos2(name_rect.right() + 26.0, row.center().y), gain, VOLUME.default, ui.scope_id().with(("gain", i)));
        self.mod_menu(&r, Target::ChannelGain { slot });
        self.apply_knob(&r, v, "channel.set", |v| json!({ "channel": i, "gain": v }), &VOLUME);
        self.hint(&r, tr!("Volume {}% · drag up or down (Shift: finer), wheel, double-click: 80% · right-click: automate, modulate, MIDI learn", format!("{:.0}", v.unwrap_or(gain) * 100.0)));
    }

    /// A channel's steps in the current pattern, or its notes as a small piano roll when
    /// they are more than steps (a melody, longer notes).
    pub(crate) fn rack_steps(&mut self, ui: &mut Ui, t: &Theme, i: usize, now_step: Option<usize>) {
        let Some(ch) = self.session.project.channels.get(i) else { return };
        let (name, slot) = (ch.name.clone(), ch.slot);
        let pattern = self.session.current_pattern();
        let length = pattern.length;
        let (row, over) = ui.allocate_exact_size(vec2(length as f32 * STEP_W, ROW), Sense::hover());
        let painter = ui.painter_at(row.expand(1.0));
        self.drop_on_row(ui, t, &over, i);
        let pattern = self.session.current_pattern();
        if self.session.selected == i {
            painter.rect_filled(row, CornerRadius::same(t.radius_sm), Theme::mix(t.bg_panel, t.fg_text, 0.06));
        }
        let area = row.shrink2(vec2(0.0, 2.0));
        if !pattern.is_steps(slot) {
            let notes = pattern.notes(slot).to_vec();
            self.mini_roll(ui, t, &painter, area, &notes, i, &name, now_step);
            return;
        }
        let steps: Vec<u8> = (0..length).map(|s| pattern.step(slot, s)).collect();
        let tick = self.pattern_tick();

        // steps: press to toggle, drag along the row to paint, right-drag to erase
        let step_at = |p: Pos2| {
            let s = ((p.x - area.left()) / STEP_W).floor();
            (s >= 0.0 && (s as usize) < length).then_some(s as usize)
        };
        let r = ui.interact(area, ui.scope_id().with(("steps", i)), Sense::click_and_drag());
        let pointer = r.interact_pointer_pos().or(r.hover_pos());
        if r.hovered() && ui.input(|inp| inp.pointer.primary_pressed()) {
            if let Some(s) = pointer.and_then(step_at) {
                let on = steps[s] == 0;
                self.paint = Some((i, on));
                if on {
                    self.spark(ui, t.accent);
                }
                self.session.close_merge(); // a new gesture is a new undo step
                let velocity = if on { DEFAULT_VELOCITY } else { 0 };
                self.run_merge("step.set", json!({ "channel": i, "step": s, "velocity": velocity }), "paint");
            }
        } else if r.is_pointer_button_down_on()
            && let (Some((row_i, on)), Some(s)) = (self.paint, pointer.and_then(step_at))
            && row_i == i
            && (steps[s] > 0) != on
        {
            let velocity = if on { DEFAULT_VELOCITY } else { 0 };
            self.run_merge("step.set", json!({ "channel": i, "step": s, "velocity": velocity }), "paint");
        }
        // right button held: an eraser across the row
        if r.contains_pointer()
            && ui.input(|inp| inp.pointer.secondary_down())
            && let Some(s) = pointer.and_then(step_at)
            && steps[s] > 0
        {
            self.run_merge("step.set", json!({ "channel": i, "step": s, "velocity": 0 }), "erase");
        }
        if ui.input(|inp| inp.pointer.secondary_released()) {
            self.session.close_merge();
        }
        if ui.input(|inp| inp.pointer.primary_released()) && self.paint.is_some_and(|(row_i, _)| row_i == i) {
            self.paint = None;
            self.session.close_merge();
        }

        let visible = ui.clip_rect();
        for (s, &velocity) in steps.iter().enumerate() {
            let cell = Rect::from_min_size(pos2(area.left() + s as f32 * STEP_W, area.top()), vec2(STEP_W - 2.0, area.height()));
            if !cell.intersects(visible) {
                continue;
            }
            // alternate beats so the bar reads at a glance
            let off = if (s / 4) % 2 == 0 { t.bg_control } else { Theme::mix(t.bg_control, t.fg_text, 0.08) };
            let fill = if velocity > 0 { Theme::mix(off, t.accent, 0.45 + 0.55 * velocity as f32 / 127.0) } else { off };
            // a step that lights up pops for a moment
            let pop = ui.ctx().animate_bool_with_time(Id::unique(("pop", slot, s)), velocity > 0, t.motion * 1.5);
            let bump = if velocity > 0 && pop < 1.0 { (pop * std::f32::consts::PI).sin() * 0.18 } else { 0.0 };
            let shown = cell.expand2(cell.size() * bump * 0.5);
            if now_step == Some(s) {
                painter.rect_filled(cell.expand(2.0), CornerRadius::same(t.radius_md), t.fg_text.gamma_multiply(0.10));
            }
            painter.rect_filled(shown, CornerRadius::same(t.radius_sm), fill);
            let hit = tick.filter(|_| velocity > 0).map_or(0.0, |tick| flash(tick, s as u32 * STEP));
            if hit > 0.0 {
                // the step that just played flashes, a halo around it
                painter.rect_filled(shown.expand(1.0 + 4.0 * hit), CornerRadius::same(t.radius_md), t.accent.gamma_multiply(0.35 * hit));
                painter.rect_filled(shown, CornerRadius::same(t.radius_sm), t.fg_text.gamma_multiply(0.6 * hit));
            }
            if now_step == Some(s) {
                painter.rect_stroke(cell, CornerRadius::same(t.radius_sm), Stroke::new(1.0, t.fg_text), StrokeKind::Inside);
            }
            // keyboard: arrows walk the grid, Enter toggles, Delete clears (the mouse uses the row)
            let key = ui.interact(cell, Id::unique(("step", slot, s)), Sense::focusable_noninteractive());
            key.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, velocity > 0, format!("{name} step {}", s + 1)));
            if key.has_focus() {
                let (enter, delete) = ui.input(|inp| (inp.key_pressed(Key::Enter), inp.key_pressed(Key::Delete) || inp.key_pressed(Key::Backspace)));
                if enter || delete {
                    let velocity = if enter && velocity == 0 { DEFAULT_VELOCITY } else { 0 };
                    self.run("step.set", json!({ "channel": i, "step": s, "velocity": velocity }));
                }
                kit::focus_ring(ui, t, &key, t.radius_sm);
                self.info_next = tr!("{}, step {} of {}: Enter toggles · Delete clears · arrows move", name, s + 1, length);
            }
        }
        self.hint(&r, tr!("Steps: click to toggle, drag to paint, hold the right button to erase. Each step is a 16th note, and a note in the piano roll."));
    }

    /// A channel's notes squeezed into its row; click to edit them in the piano roll.
    #[allow(clippy::too_many_arguments)]
    fn mini_roll(&mut self, ui: &mut Ui, t: &Theme, painter: &egui::Painter, area: Rect, notes: &[Note], i: usize, name: &str, now_step: Option<usize>) {
        let r = ui.interact(area, ui.scope_id().with(("roll", i)), Sense::click());
        let h = kit::hover(ui, t, &r);
        painter.rect_filled(area, CornerRadius::same(t.radius_sm), Theme::mix(t.bg_field, t.bg_control, 0.4 + 0.4 * h));
        for bar in (0..(area.width() / (STEP_W * 16.0)).ceil() as usize).filter(|b| b % 2 == 1) {
            let x = area.left() + bar as f32 * STEP_W * 16.0;
            painter.rect_filled(Rect::from_x_y_ranges(x..=(x + STEP_W * 16.0).min(area.right()), area.y_range()), CornerRadius::ZERO, t.fg_text.gamma_multiply(0.03));
        }
        let lo = notes.iter().map(|n| n.key).min().unwrap_or(60);
        let hi = notes.iter().map(|n| n.key).max().unwrap_or(60);
        let span = (hi - lo).max(11) as f32 + 1.0;
        let lo = lo.saturating_sub(((span - 1.0 - (hi - lo) as f32) / 2.0) as u8);
        let px = STEP_W / STEP as f32;
        let bar_h = (area.height() / span).clamp(2.0, 4.0);
        for n in notes {
            let y = area.bottom() - ((n.key.saturating_sub(lo)) as f32 + 0.5) / span * area.height();
            let note = Rect::from_min_size(pos2(area.left() + n.start as f32 * px, y - bar_h / 2.0), vec2((n.length as f32 * px - 1.0).max(2.0), bar_h));
            painter.rect_filled(note, CornerRadius::same(1), Theme::mix(t.bg_control, t.accent, 0.4 + 0.6 * n.velocity as f32 / 127.0));
        }
        if let Some(s) = now_step {
            let x = area.left() + s as f32 * STEP_W;
            painter.line_segment([pos2(x, area.top()), pos2(x, area.bottom())], Stroke::new(1.0, t.fg_text));
        }
        if r.clicked() {
            self.run("channel.select", json!({ "channel": i }));
            self.tab = crate::Tab::PianoRoll;
        }
        self.hint(&r, tr!("{} plays notes here that steps can't show: click to edit them in the piano roll.", name));
    }

    // ---- channel settings (the inspector) ------------------------------------

    /// The selected channel's settings on the right of the rack and the piano roll: its
    /// instrument and the effects of the track it plays into. F8 shows or hides it.
    pub fn inspector(&mut self, ui: &mut Ui, id: &str) {
        if !self.settings.inspector || self.session.selected_channel().is_none() {
            return;
        }
        let t = self.theme.clone();
        if self.settings.inspector_window {
            // once a frame: the rack and the piano roll side by side both ask for it
            let (key, pass) = (Id::unique("channel-settings-pass"), ui.ctx().cumulative_pass_nr());
            if ui.data(|d| d.get_temp::<u64>(key)) == Some(pass) {
                return;
            }
            ui.data_mut(|d| d.insert_temp(key, pass));
            let ctx = ui.ctx().clone();
            egui::Window::new("Channel settings")
                .id(Id::unique("channel-settings-window"))
                .title_bar(false)
                .frame(kit::window_frame(&ctx, &t))
                .resizable(true)
                .default_size([460.0, 560.0])
                .min_size([380.0, 280.0])
                .default_pos(ctx.content_rect().center() - vec2(230.0, 280.0))
                .show(&ctx, |ui| {
                    kit::region(ui);
                    self.device_view(ui);
                });
            return;
        }
        let mut open = true;
        egui::Panel::right(("channel-settings", id))
            .frame(Frame::new().fill(t.bg_panel))
            .resizable(true)
            .default_size(420.0)
            .size_range(340.0..=760.0)
            .show_collapsible(ui, &mut open, |ui| {
                kit::region(ui);
                self.device_view(ui);
            });
        if !open {
            self.settings.inspector = false;
        }
    }

    /// The bar button that shows or hides the channel settings.
    pub fn inspector_toggle(&mut self, ui: &mut Ui, t: &Theme) {
        let tip = self.tip(tr!("The selected channel's settings on the right"), "editor.device");
        if kit::icon_button(ui, t, icons::SIDEBAR_SIMPLE, &tip, self.settings.inspector).clicked() {
            self.settings.inspector = !self.settings.inspector;
        }
    }

    pub fn device_view(&mut self, ui: &mut Ui) {
        let t = self.theme.clone();
        let Some(ch) = self.session.selected_channel() else {
            kit::padded(ui, &t, |ui| ui.label(RichText::new(tr!("No channel selected.")).color(t.fg_muted)));
            return;
        };
        let (track, synth, name, mute) = (ch.track, ch.synth().is_some(), ch.name.clone(), ch.mute);
        // a sampler's panel has its track in its MIX section, like a clip's
        let sampler = matches!(ch.instrument, Instrument::Sampler { .. });
        let names: Vec<String> = self.session.project.mixer.iter().map(|t| t.name.clone()).collect();
        // Ableton-style: the instrument, then the effects of the track it plays into
        kit::bar(ui, &t, |ui| {
            let kind = if synth { "VA-1" } else { "Sampler" };
            ui.label(RichText::new(format!("{}  {kind}", if synth { icons::WAVE_SAWTOOTH } else { icons::WAVEFORM })).strong());
            ui.label(RichText::new(&name).color(t.accent));
            if !sampler {
                kit::vsep(ui, &t);
                kit::caps(ui, &t, tr!("PLAYS INTO"));
                let current = names.get(track).map_or("Master", String::as_str);
                let r = egui::ComboBox::from_id_salt("channel-track").selected_text(format!("{}  {current}", icons::FADERS)).width(130.0).show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        if ui.selectable_label(i == track, n).clicked() {
                            self.run("channel.set", json!({ "track": i }));
                        }
                    }
                });
                self.hint(&r.response, tr!("The mixer track this channel plays into. Its effects are below; faders are on the Mixer (F9)."));
            }
            kit::right(ui, |ui| {
                if kit::icon_button(ui, &t, icons::X, tr!("Hide (F8)"), false).clicked() {
                    self.settings.inspector = false;
                }
                let window = self.settings.inspector_window;
                let (place, help) = if window { (icons::SIDEBAR_SIMPLE, tr!("Dock it on the right")) } else { (icons::ARROW_SQUARE_OUT, tr!("Float it in a window")) };
                if kit::icon_button(ui, &t, place, help, false).clicked() {
                    self.settings.inspector_window = !window;
                }
                kit::vsep(ui, &t);
                if sampler {
                    let r = kit::icon_button(ui, &t, icons::FOLDER_OPEN, tr!("Replace the sound…"), false);
                    if r.clicked()
                        && let Some(file) = rfd::FileDialog::new().add_filter("Audio", &AUDIO_EXTENSIONS).pick_file()
                    {
                        self.run("channel.set", json!({ "sample": file.to_string_lossy() }));
                    }
                    self.hint(&r, tr!("Pick another sample for this channel (or drop one on the waveform)."));
                    let r = kit::icon_button(ui, &t, icons::MAGNET, tr!("Snap the cuts to 16ths"), self.settings.snap_cuts);
                    if r.clicked() {
                        self.settings.snap_cuts = !self.settings.snap_cuts;
                    }
                    self.hint(&r, tr!("On: the handles and envelope points land on 16ths (Alt: the other way round). Off: anywhere."));
                }
                let r = kit::icon_button(ui, &t, if mute { icons::SPEAKER_SLASH } else { icons::SPEAKER_HIGH }, if mute { tr!("Muted") } else { tr!("Mute") }, mute);
                if r.clicked() {
                    self.run("channel.set", json!({ "mute": !mute }));
                }
                self.hint(&r, tr!("A muted channel stays in the rack but doesn't play (M on its row)."));
                if synth {
                    if kit::icon_button(ui, &t, icons::SHUFFLE, tr!("Randomize the patch within musical ranges (Ctrl Z to go back)"), false).clicked() {
                        self.run("synth.surprise", json!({}));
                    }
                    if kit::icon_button(ui, &t, icons::ARROW_COUNTER_CLOCKWISE, tr!("Initialize: every parameter back to its default"), false).clicked() {
                        self.run("synth.reset", json!({}));
                    }
                }
            });
        });
        let instrument = self.session.selected_channel().map(|c| c.instrument.clone());
        kit::scroll(ui, &t, "device", false, t.bg_base, |ui| {
            ui.spacing_mut().item_spacing = vec2(t.gap, t.gap);
            let card = Frame::new().fill(t.bg_device).inner_margin(t.pad);
            let shown = match instrument {
                Some(Instrument::Synth { .. }) => card.show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    self.synth_device(ui, &t)
                }).response.rect,
                Some(Instrument::Sampler { sample }) => card.show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    self.sampler_device(ui, &t, &sample)
                }).response.rect,
                Some(Instrument::Multi { file }) => card.show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    self.multi_device(ui, &t, &file)
                }).response.rect,
                Some(Instrument::Plugin { plugin }) => {
                    let slot = self.session.selected_channel().map_or(0, |c| c.slot);
                    card.show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(RichText::new(&plugin.name).strong().color(t.fg_text));
                        self.plugin_card(ui, &t, ravune_engine::PluginTarget::Channel(slot), &plugin);
                    })
                    .response
                    .rect
                }
                None => return,
            };
            ui.painter().hline(ui.max_rect().x_range(), shown.bottom() + 0.5, Stroke::new(1.0, t.line));
            kit::spotlight(ui, &t, shown, 0);
            // a sampler shows its track's effects once that track is its own (its MIX makes it)
            if !sampler || self.owns_track(crate::clipprops::Owner::Channel, track) {
                self.chain(ui, &t, track);
            }
        });
    }

    fn synth_device(&mut self, ui: &mut Ui, t: &Theme) {
        ui.label(RichText::new(tr!("Virtual analog · 2 oscillators → ZDF low-pass → ADSR · 16 voices")).small().color(t.fg_muted));
        use Param::*;
        let sections: [(&str, &[Param]); 4] = [
            (tr!("OSCILLATORS"), &[Osc1Wave, Osc2Wave, Osc2Pitch, Osc2Detune, OscMix]),
            (tr!("FILTER"), &[Cutoff, Resonance, EnvAmount]),
            (tr!("AMP ENVELOPE"), &[Attack, Decay, Sustain, Release]),
            (tr!("OUTPUT"), &[Gain]),
        ];
        // egui can't wrap nested frames, so pick the rows ourselves: one when wide, two otherwise.
        let per_row = match ui.available_width() {
            w if w > 1040.0 => 4,
            w if w > 640.0 => 2,
            _ => 1,
        };
        for row in sections.chunks(per_row) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (k, &(title, params)) in row.iter().enumerate() {
                    let rect = self.synth_section(ui, t, title, params);
                    if k + 1 < row.len() {
                        ui.painter().vline(rect.right() - 0.5, rect.y_range(), Stroke::new(1.0, t.line));
                    }
                }
            });
        }
    }

    fn synth_value(&self, p: Param) -> f32 {
        self.session.selected_channel().and_then(|c| c.synth()).map_or(p.info().default, |s| s[p as usize])
    }

    fn synth_section(&mut self, ui: &mut Ui, t: &Theme, title: &str, params: &[Param]) -> Rect {
        Frame::new().inner_margin(egui::Margin::symmetric(t.pad as i8, t.gap as i8)).show(ui, |ui| {
            ui.vertical(|ui| {
                label(ui, t, title);
                ui.horizontal(|ui| {
                    for &p in params {
                        let info = p.info();
                        let value = self.synth_value(p);
                        let (resp, new) = if info.unit == "wave" {
                            widgets::wave_picker(ui, t, info.name, value, 4)
                        } else {
                            widgets::param(ui, t, info, value)
                        };
                        self.apply_knob(&resp, new, "synth.set_param", |v| json!({ "param": p, "value": v }), info);
                        if let Some(slot) = self.session.selected_channel().map(|c| c.slot) {
                            self.mod_menu(&resp, Target::Synth { slot, param: p });
                        }
                    }
                    if params.contains(&Param::Attack) {
                        let adsr = [Param::Attack, Param::Decay, Param::Sustain, Param::Release].map(|p| self.synth_value(p));
                        let r = widgets::adsr_preview(ui, t, adsr);
                        self.hint(&r, tr!("Amplitude envelope: attack, decay, sustain level, release."));
                    }
                });
            });
        })
        .response
        .rect
    }

    /// Runs a knob's operator: drags and scrolls merge into one undo step.
    pub fn apply_knob(
        &mut self,
        resp: &egui::Response,
        new: Option<f32>,
        op: &str,
        props: impl Fn(f32) -> serde_json::Value,
        info: &ParamInfo,
    ) {
        if let Some(v) = new {
            if resp.dragged() || resp.hovered() && !resp.clicked() && !resp.double_clicked() {
                self.run_merge(op, props(v), info.name);
            } else {
                self.run(op, props(v));
            }
        }
        if resp.drag_stopped() {
            self.session.close_merge();
        }
        self.hint(resp, tr!("{}: {}   (drag · Shift = fine · double-click = reset)", info.name, crate::i18n::t(info.help)));
    }

    fn multi_device(&mut self, ui: &mut Ui, t: &Theme, file: &str) {
        let info = self.library.instrument(file);
        let detail = match &info {
            Ok(i) => format!("{} · {file}", i.summary()),
            Err(e) => tr!("can't play it: {}", e),
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(detail).small().color(if info.is_ok() { t.fg_muted } else { t.meter_hot }));
            let r = kit::button(ui, t, Some(icons::FOLDER_OPEN), tr!("Replace…"), false);
            if r.clicked()
                && let Some(file) = rfd::FileDialog::new().add_filter(tr!("SFZ or Decent Sampler"), &INSTRUMENT_EXTENSIONS).pick_file()
            {
                self.run("channel.set", json!({ "sample": file.to_string_lossy() }));
            }
            self.hint(&r, tr!("Pick another .sfz or .dspreset for this channel."));
        });
        let Some(ch) = self.session.selected_channel() else { return };
        let (pitch, gain, ch_slot) = (ch.pitch, ch.gain, ch.slot);
        Frame::new().fill(t.bg_panel).corner_radius(t.radius_sm).inner_margin(8).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, v) = widgets::knob(ui, t, &PITCH, pitch);
                self.apply_knob(&r, v, "channel.set", |v| json!({ "pitch": v }), &PITCH);
                let (r, v) = widgets::knob(ui, t, &VOLUME, gain);
                self.apply_knob(&r, v, "channel.set", |v| json!({ "gain": v }), &VOLUME);
                self.mod_menu(&r, Target::ChannelGain { slot: ch_slot });
                ui.label(RichText::new(tr!("Play it from the keys or a MIDI controller; the piano roll writes its notes.")).small().color(t.fg_muted));
            });
        });
    }

    fn sampler_device(&mut self, ui: &mut Ui, t: &Theme, path: &str) {
        // its name, Replace and the switches are in the panel's header (as on a clip's)
        let sample = self.library.get(path);
        let Some(ch) = self.session.selected_channel() else { return };
        let (pitch, gain, choke, ch_slot, attack, release, start, end) = (ch.pitch, ch.gain, ch.choke as f32, ch.slot, ch.attack, ch.release, ch.start, ch.end);
        let (looped, track, name) = (ch.looped, ch.track, ch.name.clone());
        // the sound on top, drawn as on a clip's panel: press to hear the channel, drag a
        // handle to cut its start or end, drop a sample on it to replace it
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(200.0), crate::clipprops::SOUND_H), Sense::click());
        let sound = sample.as_ref().ok().cloned();
        let ticks = sound.as_ref().map_or(1.0, |s| s.seconds() as f64 / crate::playlist::tick_seconds(self.session.project.tempo));
        let info = crate::clipprops::sound_info(sound.as_deref(), path);
        if let Some((cut, tick)) = self.sound_view(ui, t, rect, (0.0, ticks), sound.as_deref().map(|s| (s, ticks)), (start as f64 * ticks, end as f64 * ticks), 0.0, &info) {
            // on the grid with the magnet on; a hundredth of the sound at least stays
            let grid = self.cut_grid(ui) as f64;
            let at = ((tick / grid).round() * grid / ticks).clamp(0.0, 1.0) as f32;
            let props = match cut {
                Cut::Start => json!({ "start": at.min(end - 0.01) }),
                Cut::End => json!({ "end": at.max(start + 0.01) }),
            };
            self.run_merge("channel.set", props, "sample-cut");
        }
        if kit::drag_over::<SampleDrag>(&resp).is_some() {
            ui.painter().rect_stroke(rect, CornerRadius::same(t.radius_sm), Stroke::new(2.0, t.select), StrokeKind::Inside);
        }
        // held down, the channel plays as from its key; let go, it stops (if it has a release)
        let held = Id::unique("sampler-audition");
        let was = ui.data(|d| d.get_temp::<bool>(held).unwrap_or(false));
        if resp.is_pointer_button_down_on() != was {
            ui.data_mut(|d| d.insert_temp(held, !was));
            if was { self.note_off(60) } else { self.note_on(60) }
        }
        self.hint(&resp, tr!("Press to hear it · drag a handle to cut its start or end · drop a sample here to replace it"));
        if let Some(drag) = kit::dropped::<SampleDrag>(&resp) {
            self.run("channel.set", json!({ "sample": drag.0 }));
        }
        // the same sections as a clip's panel: the sample, its envelope, the mix
        let whole = sound.as_ref().map_or(0.0, |s| s.seconds());
        Self::props_section(ui, t, tr!("SAMPLE"), |ui| {
            ui.horizontal_wrapped(|ui| {
                let at = |v: f32| (v / whole.max(0.01)).clamp(0.0, 1.0);
                let info = crate::clipprops::cut_info("Start", whole, 0.0);
                let (r, v) = widgets::knob(ui, t, &info, start * whole);
                self.apply_knob(&r, v, "channel.set", |v| json!({ "start": at(v).min(end - 0.01) }), &info);
                let info = crate::clipprops::cut_info("End", whole, whole);
                let (r, v) = widgets::knob(ui, t, &info, end * whole);
                self.apply_knob(&r, v, "channel.set", |v| json!({ "end": at(v).max(start + 0.01) }), &info);
                let (r, v) = widgets::knob(ui, t, &PITCH, pitch);
                self.apply_knob(&r, v, "channel.set", |v| json!({ "pitch": v }), &PITCH);
                let (r, v) = widgets::knob(ui, t, &VOLUME, gain);
                self.apply_knob(&r, v, "channel.set", |v| json!({ "gain": v }), &VOLUME);
                self.mod_menu(&r, Target::ChannelGain { slot: ch_slot });
            });
            ui.horizontal_wrapped(|ui| {
                kit::caps(ui, t, tr!("PLAYS"));
                if let Some(k) = kit::segmented(ui, t, looped as usize, &[tr!("One-shot"), tr!("Loop")]) {
                    self.run("channel.set", json!({ "looped": k == 1 }));
                }
                let what = if looped { tr!("Start to end, round and round while the note is held (FL's loop).") } else { tr!("Start to end once, however long the note (Release stops it with the note).") };
                ui.label(RichText::new(what).small().color(t.fg_muted));
            });
        });
        Self::props_section(ui, t, tr!("ENVELOPE"), |ui| {
            ui.horizontal_wrapped(|ui| {
                let (r, v) = widgets::knob(ui, t, &ATTACK, attack);
                self.apply_knob(&r, v, "channel.set", |v| json!({ "attack": v }), &ATTACK);
                let (r, v) = widgets::knob(ui, t, &RELEASE, release);
                self.apply_knob(&r, v, "channel.set", |v| json!({ "release": v }), &RELEASE);
                let (r, v) = widgets::knob(ui, t, &CHOKE, choke);
                self.apply_knob(&r, v, "channel.set", |v| json!({ "choke": v as u8 }), &CHOKE);
            });
        });
        self.sound_mix(ui, t, crate::clipprops::Owner::Channel, track, &name);
    }
}

/// 1 the moment the playhead reaches `start`, fading to 0 over a 16th and a half.
fn flash(tick: f64, start: u32) -> f32 {
    let since = tick - start as f64;
    if (0.0..1.5 * STEP as f64).contains(&since) { 1.0 - (since / (1.5 * STEP as f64)) as f32 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drags a sound from a source widget onto the middle of what `row` draws for channel 1,
    /// over a few frames: what that channel plays afterwards.
    fn drag_sound_onto(app: &mut App, row: fn(&mut App, &mut Ui, &Theme)) -> String {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let t = app.theme.clone();
        // one frame of input; where the rows were drawn
        let frame = |app: &mut App, events: Vec<egui::Event>| -> Rect {
            let mut rows = Rect::NOTHING;
            let input = egui::RawInput { events, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 600.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                let (_, src) = ui.allocate_exact_size(vec2(100.0, 20.0), Sense::drag());
                src.dnd_set_drag_payload(SampleDrag("builtin:Kicks/kick-house.wav".into()));
                let shown = ui.vertical(|ui| row(app, ui, &t)).response;
                rows = shown.rect;
            });
            out.textures_delta.clear();
            rows
        };
        let at = |p: Pos2| egui::Event::PointerMoved(p);
        let button = |p: Pos2, pressed: bool| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let start = pos2(10.0, 10.0);
        let rows = frame(app, vec![]);
        frame(app, vec![at(start), button(start, true)]);
        frame(app, vec![at(start + vec2(30.0, 0.0))]);
        let end = rows.center();
        frame(app, vec![at(end)]);
        frame(app, vec![at(end), button(end, false)]);
        frame(app, vec![]);
        match &app.session.project.channels[1].instrument {
            Instrument::Sampler { sample } => sample.clone(),
            _ => String::new(),
        }
    }

    /// A sound dropped on a channel's row (its name or its steps, over the buttons and
    /// knobs on it too) replaces what it plays.
    #[test]
    fn a_sound_dropped_on_a_channel_replaces_it() {
        let mut app = App::for_tests();
        assert_eq!(drag_sound_onto(&mut app, |app, ui, t| app.rack_head(ui, t, 1)), "builtin:Kicks/kick-house.wav");
        let mut app = App::for_tests();
        assert_eq!(drag_sound_onto(&mut app, |app, ui, t| app.rack_steps(ui, t, 1, None)), "builtin:Kicks/kick-house.wav");
    }

    /// A channel lights up the moment its note starts and fades out over the 16th.
    #[test]
    fn a_channel_flashes_as_it_hits() {
        let mut app = App::for_tests();
        app.session.run("step.set", json!({ "channel": 0, "step": 8, "velocity": 127 })).unwrap();
        app.beat = Some(2.0 + 0.25 * 0.375); // three eighths of a 16th after step 8 (beat 3)
        let hit = app.channel_hit(0);
        assert!((hit - 0.75).abs() < 0.01, "{hit}");
        app.beat = Some(2.4);
        assert_eq!(app.channel_hit(0), 0.0, "a 16th and a half later: dark again");
        app.session.song_mode = true;
        app.beat = Some(2.0);
        assert_eq!(app.channel_hit(0), 0.0, "only while a pattern plays");
    }
}
#[cfg(test)]
mod narrow {
    use super::*;

    /// The channel settings fit the side panel beside the rack and the piano roll: a sampler
    /// and a VA-1 laid out in 420 px take no more (a panel grows to fit what's in it).
    #[test]
    fn channel_settings_fit_a_narrow_panel() {
        for channel in [0, 5] {
            let mut app = App::for_tests();
            app.session.selected = channel;
            let ctx = egui::Context::default();
            crate::theme::install_fonts(&ctx);
            let mut width = 0.0;
            for _ in 0..2 {
                let screen = Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 800.0));
                let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                    ui.scope(|ui| {
                        ui.set_max_width(420.0);
                        app.device_view(ui);
                        width = ui.min_rect().width();
                    });
                });
                out.textures_delta.clear();
            }
            assert!(width <= 420.5, "channel {channel}: {width} px");
        }
    }
}
