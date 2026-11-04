//! The specimen board (ported from Velo): every widget in its states, moved by a fake 128 BPM
//! loop, on top of the real UI, with what the interface costs a frame and a stress mode of 200
//! knobs. Opened from the command palette ("Specimen board"), or `RAVUNE_SPECIMEN=1` at start
//! (`RAVUNE_SPECIMEN=stress` with the stress test on). For checking a theme or a change to the
//! kit at a glance, and for catching a widget that got slow.

use std::collections::VecDeque;
use std::f32::consts::TAU;

use eframe::egui::{self, Id, RichText, Sense, Ui, vec2};
use ravune_core::{Curve, ParamInfo};

use crate::icons;
use crate::kit::{self, Variant};
use crate::orb::{self, State};
use crate::shell::Tone;
use crate::theme::Theme;
use crate::usage::{self, Load};
use crate::widgets;

const CUTOFF: ParamInfo = ParamInfo { name: "Cutoff", min: 20.0, max: 20_000.0, default: 2_000.0, curve: Curve::Log, unit: "Hz", help: "" };
const DRIVE: ParamInfo = ParamInfo { name: "Drive", min: 0.0, max: 1.0, default: 0.3, curve: Curve::Linear, unit: "%", help: "" };
const PAN: ParamInfo = ParamInfo { name: "Pan", min: -1.0, max: 1.0, default: 0.0, curve: Curve::Linear, unit: "", help: "" };
const SYNC: ParamInfo = ParamInfo { name: "Sync", min: 0.0, max: 1.0, default: 1.0, curve: Curve::Linear, unit: "onoff", help: "" };

/// A fake four-on-the-floor loop at 128 BPM: what moves the meters.
struct Signals {
    beat: f64,
    level: f32,
    kick: f32,
    lfo: f32,
}

impl Signals {
    fn at(time: f64) -> Self {
        let beat = time * 128.0 / 60.0;
        let kick = (-((beat.rem_euclid(1.0)) * 60.0 / 128.0) as f32 / 0.12).exp();
        let lfo = (time as f32 * TAU / (4.0 * 60.0 / 128.0)).sin() * 0.5 + 0.5;
        Self { beat, level: 0.45 + 0.45 * kick, kick, lfo }
    }
}

pub struct Specimen {
    cutoff: f32,
    drive: f32,
    pan: f32,
    sync: f32,
    wave: f32,
    power: bool,
    switch: bool,
    tab: usize,
    mode: usize,
    gain: f32,
    stress: bool,
    /// The interface's frame costs of the last two seconds (when, seconds).
    costs: VecDeque<(f64, f32)>,
}

impl Default for Specimen {
    fn default() -> Self {
        Self { cutoff: 2_000.0, drive: 0.3, pan: 0.0, sync: 1.0, wave: 0.0, power: true, switch: true, tab: 0, mode: 1, gain: 0.8, stress: false, costs: VecDeque::new() }
    }
}

impl Specimen {
    /// `RAVUNE_SPECIMEN=1` opens the board at start, `RAVUNE_SPECIMEN=stress` with the stress
    /// test on.
    pub fn from_env() -> Option<Self> {
        let v = std::env::var("RAVUNE_SPECIMEN").ok()?;
        Some(Self { stress: v == "stress", ..Default::default() })
    }

    /// Draws the board; `cost` is what the last frame took (eframe's count). Returns whether
    /// it's still open, and a toast it asked for.
    pub fn show(&mut self, ctx: &egui::Context, t: &Theme, cost: Option<f32>) -> (bool, Option<Tone>) {
        let now = ctx.input(|i| i.time);
        if let Some(cost) = cost {
            self.costs.push_back((now, cost));
        }
        while self.costs.front().is_some_and(|(at, _)| now - at > 2.0) {
            self.costs.pop_front();
        }
        let (mut open, mut toast) = (true, None);
        let s = Signals::at(now);
        egui::Window::new("specimen").title_bar(false).frame(kit::window_frame(ctx, t)).default_size([760.0, 720.0]).resizable(true).show(ctx, |ui| {
            kit::region(ui);
            kit::bar(ui, t, |ui| {
                ui.label(RichText::new(tr!("Specimen board")).strong().color(t.fg_text));
                ui.label(RichText::new(tr!("every widget, moved by a fake 128 BPM loop")).color(t.fg_muted));
                kit::right(ui, |ui| {
                    if kit::icon_button(ui, t, icons::X, tr!("Close (Esc)"), false).clicked() {
                        open = false;
                    }
                });
            });
            kit::scroll(ui, t, "specimen", true, t.bg_base, |ui| toast = self.board(ui, t, &s));
        });
        // the fake loop moves every frame
        ctx.request_repaint();
        (open && !ctx.input(|i| i.key_pressed(egui::Key::Escape)), toast)
    }

    fn board(&mut self, ui: &mut Ui, t: &Theme, s: &Signals) -> Option<Tone> {
        ui.spacing_mut().item_spacing = vec2(t.gap * 2.0, t.gap * 2.0);
        let mut toast = None;
        kit::section(ui, t, tr!("BUTTONS"), true, |ui| {
            ui.horizontal(|ui| {
                kit::action(ui, t, Variant::Secondary, Some(icons::PLUS), tr!("Add channel"));
                kit::action(ui, t, Variant::Outline, Some(icons::METRONOME), tr!("Tap"));
                kit::action(ui, t, Variant::Ghost, None, tr!("Cancel"));
                kit::button(ui, t, Some(icons::LIGHTNING), tr!("Learn MIDI"), true);
                kit::button(ui, t, None, tr!("Off"), false);
            });
            ui.horizontal(|ui| {
                kit::icon_button(ui, t, icons::LINK, tr!("On"), true);
                kit::icon_button(ui, t, icons::SLIDERS, tr!("Off"), false);
                kit::tinted_icon_button(ui, t, icons::RECORD, tr!("Record"), true, t.meter_hot);
                let items = [(0, icons::SQUARES_FOUR, tr!("Rack")), (1, icons::PIANO_KEYS, tr!("Piano Roll")), (2, icons::FADERS, tr!("Mixer"))];
                if let Some(v) = kit::tabs(ui, t, Id::unique("specimen-tabs"), self.tab, &items, false) {
                    self.tab = v;
                }
                if let Some(m) = kit::segmented(ui, t, self.mode, &["PAT", "SONG"]) {
                    self.mode = m;
                }
                kit::switch(ui, t, &mut self.switch, tr!("Switch"));
            });
            ui.horizontal(|ui| {
                ui.set_max_width(260.0);
                kit::hold_button(ui, t, Some(icons::TRASH), tr!("Hold to delete all"), tr!("Deleted"), 1.2);
            });
        });

        kit::section(ui, t, tr!("CONTROLS"), false, |ui| {
            ui.horizontal(|ui| {
                for (info, value) in [(&CUTOFF, &mut self.cutoff), (&DRIVE, &mut self.drive), (&PAN, &mut self.pan), (&SYNC, &mut self.sync)] {
                    if let (_, Some(v)) = widgets::param(ui, t, info, *value) {
                        *value = v;
                    }
                }
                if let (_, Some(v)) = widgets::wave_picker(ui, t, "Wave", self.wave, 4) {
                    self.wave = v;
                }
                widgets::adsr_preview(ui, t, [0.05, 0.3, 0.6, 0.4]);
                if widgets::power(ui, t, self.power).clicked() {
                    self.power = !self.power;
                }
            });
            ui.horizontal(|ui| {
                widgets::meter(ui, t, s.level);
                if let (_, Some(g)) = widgets::fader(ui, t, self.gain, s.level * self.gain, s.level.max(0.8) * self.gain, 120.0) {
                    self.gain = g;
                }
                ui.vertical(|ui| {
                    widgets::beat_counter(ui, t, Some(s.beat), false);
                    widgets::position(ui, t, s.beat);
                    let (rect, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                    widgets::progress_ring(ui.painter(), t, rect.center(), 9.0, s.lfo);
                });
            });
        });

        kit::section(ui, t, tr!("STATUS"), false, |ui| {
            ui.horizontal(|ui| {
                // the AI's orb in each of its states
                for state in [State::Breathing, State::Working, State::Searching, State::Listening, State::Composing] {
                    let (rect, r) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                    orb::paint(ui.painter(), rect.center(), 28.0, state, ui.input(|i| i.time), t.accent);
                    r.on_hover_text(format!("{state:?}"));
                }
            });
            ui.horizontal(|ui| {
                // a load at ease, near its limit, over it
                let trail = |k: f32| (0..usage::TRAIL).map(|i| (k + 0.15 * (i as f32 * 0.6 + s.lfo * TAU).sin()).clamp(0.0, 1.0)).collect();
                for (name, k) in [("CPU", 0.2), ("RAM", 0.6), ("GPU", 0.95)] {
                    usage::chip(ui, t, &Load { name, value: k * 100.0, format: |v| format!("{v:.0}%"), room: "100%", strain: k, warn: 0.5, high: 0.85, trail: trail(k) });
                }
            });
        });

        kit::section(ui, t, tr!("TOASTS"), false, |ui| {
            ui.horizontal(|ui| {
                for (tone, label) in [(Tone::Info, tr!("Info")), (Tone::Done, tr!("Done")), (Tone::Action, tr!("Your turn")), (Tone::Error, tr!("Error"))] {
                    if kit::action(ui, t, Variant::Outline, None, label).clicked() {
                        toast = Some(tone);
                    }
                }
            });
        });

        kit::section(ui, t, tr!("TYPE"), false, |ui| {
            ui.label(RichText::new(tr!("Ravune makes the beat")).size(Theme::TEXT_LG).strong().color(t.fg_text));
            ui.label(RichText::new(tr!("Body text: pick a sound, draw a pattern, place it on the playlist.")).color(t.fg_text));
            ui.label(RichText::new(tr!("Muted: hints, empty states, secondary labels.")).small().color(t.fg_muted));
            ui.label(RichText::new("128.00 BPM · 48.0 kHz · 5.3 ms · −12.4 dB").font(Theme::mono(12.0)).color(t.fg_text));
        });

        kit::section(ui, t, tr!("PERFORMANCE"), false, |ui| {
            ui.horizontal(|ui| {
                let n = self.costs.len().max(1) as f32;
                let avg = self.costs.iter().map(|c| c.1).sum::<f32>() / n;
                let worst = self.costs.iter().map(|c| c.1).fold(0.0, f32::max);
                let ms = |s: f32| format!("{:.2}", s * 1000.0);
                ui.label(RichText::new(tr!("Frame {} ms avg / {} worst", ms(avg), ms(worst))).font(Theme::mono(12.0)).color(t.fg_text));
                if kit::button(ui, t, Some(icons::SLIDERS), tr!("Stress: 200 knobs"), self.stress).clicked() {
                    self.stress = !self.stress;
                }
            });
            if self.stress {
                ui.horizontal_wrapped(|ui| {
                    for i in 0..200 {
                        let phase = i as f32 * 0.37;
                        let value = ((s.lfo * TAU + phase).sin() * 0.5 + 0.5) * s.kick.max(0.3);
                        ui.push_id(i, |ui| widgets::knob(ui, t, &DRIVE, value));
                    }
                });
            }
        });
        toast
    }
}
