//! Ravune core: the project model and the operator system.
//!
//! Pure data: no audio, no UI. Every edit goes through an [`Operator`], so the
//! UI, keymaps, scripts and (later) MCP agents all share one undoable API.

mod devices;
mod model;
mod ops;
#[cfg(test)]
mod tests;

pub use devices::*;
pub use model::*;
pub use ops::{OPERATORS, Operator, VARIATIONS, find};

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Synth parameters
// ---------------------------------------------------------------------------

/// VA-1 synth parameters. The discriminant indexes [`PARAMS`] and synth param arrays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Param {
    Osc1Wave,
    Osc2Wave,
    Osc2Pitch,
    Osc2Detune,
    OscMix,
    Cutoff,
    Resonance,
    EnvAmount,
    Attack,
    Decay,
    Sustain,
    Release,
    Gain,
}

pub const PARAM_COUNT: usize = 13;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    Linear,
    /// Knob travel is logarithmic (frequencies, times).
    Log,
    /// Integer steps (waveforms, semitones).
    Steps,
}

#[derive(Debug)]
pub struct ParamInfo {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub curve: Curve,
    pub unit: &'static str,
    pub help: &'static str,
}

pub const WAVES: [&str; 4] = ["Sine", "Triangle", "Saw", "Square"];
/// LFO shapes: the synth's waves, then a new random level every cycle (sample and hold).
pub const LFO_SHAPES: [&str; 5] = ["Sine", "Triangle", "Saw", "Square", "Random"];

const fn p(
    _param: Param, // only keeps the table below readable; order must match `Param`
    name: &'static str,
    (min, max, default): (f32, f32, f32),
    curve: Curve,
    unit: &'static str,
    help: &'static str,
) -> ParamInfo {
    ParamInfo { name, min, max, default, curve, unit, help }
}

use Curve::*;
use Param::*;
pub const PARAMS: [ParamInfo; PARAM_COUNT] = [
    p(Osc1Wave, "Wave 1", (0.0, 3.0, 2.0), Steps, "wave", "Shape of oscillator 1: sine is soft, saw is bright and buzzy, square is hollow."),
    p(Osc2Wave, "Wave 2", (0.0, 3.0, 3.0), Steps, "wave", "Shape of oscillator 2."),
    p(Osc2Pitch, "Pitch 2", (-24.0, 24.0, 0.0), Steps, "st", "Transposes oscillator 2 in semitones. Try +7 or +12 for big chords."),
    p(Osc2Detune, "Detune", (0.0, 50.0, 8.0), Linear, "ct", "Slightly detunes oscillator 2 for a fat, chorus-like sound."),
    p(OscMix, "Mix", (0.0, 1.0, 0.4), Linear, "%", "Balance between oscillator 1 and oscillator 2."),
    p(Cutoff, "Cutoff", (40.0, 18000.0, 2400.0), Log, "Hz", "Filter brightness: low values sound dark and muffled, high values bright."),
    p(Resonance, "Reso", (0.0, 1.0, 0.25), Linear, "%", "Boosts the filter's cutoff point for a squelchy, vocal sound."),
    p(EnvAmount, "Env Amt", (0.0, 1.0, 0.35), Linear, "%", "How much the envelope opens the filter on each note."),
    p(Attack, "Attack", (0.001, 4.0, 0.005), Log, "s", "Time for a note to fade in. Short = punchy, long = swelling pad."),
    p(Decay, "Decay", (0.005, 4.0, 0.35), Log, "s", "Time to fall from the peak to the sustain level."),
    p(Sustain, "Sustain", (0.0, 1.0, 0.6), Linear, "%", "Level held while the key stays down."),
    p(Release, "Release", (0.005, 8.0, 0.25), Log, "s", "Time for a note to fade out after you let go."),
    p(Gain, "Volume", (0.0, 1.0, 0.7), Linear, "%", "Output volume of the synth."),
];

impl Param {
    pub const ALL: [Param; PARAM_COUNT] = [
        Osc1Wave, Osc2Wave, Osc2Pitch, Osc2Detune, OscMix, Cutoff, Resonance, EnvAmount, Attack,
        Decay, Sustain, Release, Gain,
    ];

    pub fn info(self) -> &'static ParamInfo {
        &PARAMS[self as usize]
    }
}

impl ParamInfo {
    pub fn clamp(&self, v: f32) -> f32 {
        let v = v.clamp(self.min, self.max);
        if self.curve == Steps { v.round() } else { v }
    }

    /// A value typed for this parameter, in the unit it shows ("1.2k" or "1200" Hz, "250 ms",
    /// "40%", "L30"; a bare number for a time is in the unit showing for `now`). `None` for
    /// what isn't a number (a waveform, a filter type) or text that isn't one.
    pub fn parse(&self, text: &str, now: f32) -> Option<f32> {
        let text = text.trim().to_lowercase();
        if self.unit == "pan" && text == "c" {
            return Some(0.0);
        }
        let number: String = text.chars().filter(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | ',')).collect();
        let n: f32 = number.replace(',', ".").parse().ok()?;
        let bare = !text.chars().any(|c| c.is_alphabetic());
        let v = match self.unit {
            "wave" | "ftype" | "onoff" | "beats" | "grp" => return None,
            "Hz" if text.contains('k') => n * 1000.0,
            "s" if text.contains("ms") || bare && self.format(now).ends_with("ms") => n / 1000.0,
            "%" => n / 100.0,
            "pan" if text.starts_with('l') => -n / 100.0,
            "pan" => n / 100.0,
            _ => n,
        };
        Some(self.clamp(v))
    }

    /// Value → 0..1 knob position.
    pub fn to_norm(&self, v: f32) -> f32 {
        match self.curve {
            Log => (v / self.min).ln() / (self.max / self.min).ln(),
            _ => (v - self.min) / (self.max - self.min),
        }
        .clamp(0.0, 1.0)
    }

    /// 0..1 knob position → value.
    pub fn from_norm(&self, n: f32) -> f32 {
        let n = n.clamp(0.0, 1.0);
        self.clamp(match self.curve {
            Log => self.min * (self.max / self.min).powf(n),
            _ => self.min + n * (self.max - self.min),
        })
    }

    pub fn format(&self, v: f32) -> String {
        match self.unit {
            "wave" => WAVES[v as usize % WAVES.len()].to_string(),
            "Hz" if v >= 1000.0 => format!("{:.1} kHz", v / 1000.0),
            "Hz" => format!("{v:.0} Hz"),
            "s" if v < 0.01 => format!("{:.1} ms", v * 1000.0),
            "s" if v < 1.0 => format!("{:.0} ms", v * 1000.0),
            "s" => format!("{v:.2} s"),
            "%" => format!("{:.0}%", v * 100.0),
            "st" => format!("{v:+.0} st"),
            // semitones and cents
            "semi" => format!("{v:+.2} st"),
            "grp" if v == 0.0 => "Off".into(),
            "grp" => format!("Group {v:.0}"),
            "dB" => format!("{v:+.1} dB"),
            ":1" => format!("{v:.1}:1"),
            "16th" => format!("{v:.0}/16"),
            "onoff" => if v >= 0.5 { "On" } else { "Off" }.into(),
            "ftype" => ["Low-pass", "High-pass", "Band-pass"][(v as usize).min(2)].into(),
            "pan" if v.abs() < 0.005 => "C".into(),
            "pan" => format!("{}{:.0}", if v < 0.0 { "L" } else { "R" }, v.abs() * 100.0),
            // a fade of nothing: no fade (not "1/inf")
            "beats" if v <= 0.0 => "Off".into(),
            "beats" if v < 1.0 => format!("1/{:.0} beat", 1.0 / v),
            "beats" if v == 1.0 => "1 beat".into(),
            "beats" => format!("{v:.0} beats"),
            unit => format!("{v:.0} {unit}"),
        }
    }
}

pub fn default_synth() -> [f32; PARAM_COUNT] {
    PARAMS.map(|p| p.default)
}

/// VA-1 starting points (the browser's Instruments tab): changes from the defaults.
pub const SYNTH_PRESETS: [(&str, &[(Param, f32)]); 7] = [
    ("Init", &[]),
    ("Sub Bass", &[(Osc1Wave, 0.0), (Osc2Wave, 0.0), (Osc2Pitch, -12.0), (OscMix, 0.3), (Cutoff, 400.0), (Resonance, 0.1), (EnvAmount, 0.1), (Attack, 0.002), (Decay, 0.3), (Sustain, 0.9), (Release, 0.12), (Gain, 0.8)]),
    ("Acid Bass", &[(Osc1Wave, 2.0), (Osc2Wave, 3.0), (Osc2Detune, 5.0), (OscMix, 0.2), (Cutoff, 300.0), (Resonance, 0.8), (EnvAmount, 0.8), (Attack, 0.001), (Decay, 0.2), (Sustain, 0.0), (Release, 0.08), (Gain, 0.65)]),
    ("Pluck", &[(Osc1Wave, 2.0), (Osc2Wave, 2.0), (Osc2Detune, 12.0), (OscMix, 0.5), (Cutoff, 800.0), (Resonance, 0.3), (EnvAmount, 0.7), (Attack, 0.001), (Decay, 0.25), (Sustain, 0.0), (Release, 0.3), (Gain, 0.6)]),
    ("Soft Pad", &[(Osc1Wave, 2.0), (Osc2Wave, 2.0), (Osc2Detune, 18.0), (OscMix, 0.5), (Cutoff, 1500.0), (Resonance, 0.15), (EnvAmount, 0.2), (Attack, 0.6), (Decay, 1.0), (Sustain, 0.8), (Release, 1.5), (Gain, 0.45)]),
    ("Saw Lead", &[(Osc1Wave, 2.0), (Osc2Wave, 2.0), (Osc2Pitch, 7.0), (Osc2Detune, 10.0), (OscMix, 0.4), (Cutoff, 3500.0), (Resonance, 0.35), (EnvAmount, 0.4), (Attack, 0.005), (Decay, 0.3), (Sustain, 0.7), (Release, 0.2), (Gain, 0.55)]),
    ("Bell", &[(Osc1Wave, 0.0), (Osc2Wave, 1.0), (Osc2Pitch, 19.0), (Osc2Detune, 0.0), (OscMix, 0.4), (Cutoff, 6000.0), (Resonance, 0.1), (EnvAmount, 0.3), (Attack, 0.001), (Decay, 1.2), (Sustain, 0.0), (Release, 1.5), (Gain, 0.55)]),
];

/// A preset by name (any case): its proper name and full parameter set.
pub fn synth_preset(name: &str) -> Option<(&'static str, [f32; PARAM_COUNT])> {
    let (found, changes) = SYNTH_PRESETS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name))?;
    let mut params = default_synth();
    changes.iter().for_each(|&(p, v)| params[p as usize] = p.info().clamp(v));
    Some((found, params))
}

// ---------------------------------------------------------------------------
// Session: project + transient state + undo
// ---------------------------------------------------------------------------

/// The project plus transient state (transport, selection) and undo history.
#[derive(Default)]
pub struct Session {
    pub project: Project,
    pub playing: bool,
    /// Play the playlist instead of looping the current pattern (FL's SONG/PAT switch).
    pub song_mode: bool,
    /// Where song playback starts, in ticks.
    pub position: u32,
    /// Bumped by `transport.jump`: a playing song moves to `position` right away.
    pub jumps: u32,
    /// What song mode plays over and over (ticks, from..to): the loop region while it's on.
    pub loop_range: Option<(u32, u32)>,
    /// The loop region, kept while it's off.
    pub loop_region: Option<(u32, u32)>,
    pub loop_on: bool,
    /// Channel the keyboard plays and the device / piano roll show.
    pub selected: usize,
    /// Pattern shown in the channel rack and piano roll.
    pub pattern: usize,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    /// The top undo entry may still absorb edits with the same merge key (drags).
    merge_open: bool,
    /// An open undo group: every edit lands in one step until [`Self::end_group`].
    group: Option<&'static str>,
}

struct Entry {
    label: &'static str,
    merge_key: Option<String>,
    project: Project,
}

const MAX_UNDO: usize = 500;

impl Session {
    /// Starts fresh from a loaded project (history cleared).
    pub fn with_project(project: Project) -> Self {
        Self { project, ..Default::default() }
    }

    /// Runs an operator by id with JSON props.
    pub fn run(&mut self, id: &str, props: Value) -> Result<(), String> {
        self.exec(id, props, None)
    }

    /// Like [`Self::run`], but consecutive calls with the same `key` form one undo step,
    /// until [`Self::close_merge`]; used for continuous gestures like knob drags.
    pub fn run_merge(&mut self, id: &str, props: Value, key: &str) -> Result<(), String> {
        self.exec(id, props, Some(key.to_string()))
    }

    /// Runs an operator as part of the last undo step: a change that rides along with the
    /// one just made (the same edit, copied to other clips).
    pub fn run_joined(&mut self, id: &str, props: Value) -> Result<(), String> {
        let op = find(id).ok_or_else(|| format!("unknown operator '{id}'"))?;
        let before = self.project.clone();
        if let Err(e) = (op.exec)(self, props) {
            self.project = before;
            return Err(e);
        }
        self.clamp_selection();
        if self.undo.is_empty() && before != self.project && self.group.is_none() {
            self.undo.push(Entry { label: op.label, merge_key: None, project: before });
        }
        self.redo.clear();
        Ok(())
    }

    pub fn close_merge(&mut self) {
        self.merge_open = false;
    }

    /// Starts an undo group: whatever runs until [`Self::end_group`] undoes as one step called
    /// `label` (a script run, an agent's session). Nested calls join the open group.
    pub fn begin_group(&mut self, label: &'static str) {
        if self.group.is_some() {
            return;
        }
        self.merge_open = false;
        self.undo.push(Entry { label, merge_key: None, project: self.project.clone() });
        self.group = Some(label);
    }

    /// Closes the open group; one that changed nothing leaves no undo step.
    pub fn end_group(&mut self) {
        if self.group.take().is_some() && self.undo.last().is_some_and(|e| e.project == self.project) {
            self.undo.pop();
        }
    }

    pub fn in_group(&self) -> bool {
        self.group.is_some()
    }

    pub fn selected_channel(&self) -> Option<&Channel> {
        self.project.channels.get(self.selected)
    }

    pub fn current_pattern(&self) -> &Pattern {
        &self.project.patterns[self.pattern.min(self.project.patterns.len() - 1)]
    }

    fn exec(&mut self, id: &str, props: Value, merge_key: Option<String>) -> Result<(), String> {
        let op = find(id).ok_or_else(|| format!("unknown operator '{id}'"))?;
        if !(op.poll)(self) {
            return Err(format!("{} isn't possible right now", op.label));
        }
        let before = op.undo.then(|| self.project.clone());
        if let Err(e) = (op.exec)(self, props) {
            if let Some(b) = before {
                self.project = b;
            }
            return Err(e);
        }
        self.clamp_selection();
        let Some(before) = before else { return Ok(()) };
        if before == self.project {
            return Ok(()); // no-op edits don't pollute history
        }
        if self.group.is_some() {
            self.redo.clear(); // the group's step already holds the state from before
            return Ok(());
        }
        let merging = merge_key.is_some()
            && self.merge_open
            && self.undo.last().is_some_and(|e| e.merge_key == merge_key);
        self.merge_open = merge_key.is_some();
        if !merging {
            self.undo.push(Entry { label: op.label, merge_key, project: before });
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        Ok(())
    }

    pub fn undo_label(&self) -> Option<&'static str> {
        self.undo.last().map(|e| e.label)
    }

    pub fn redo_label(&self) -> Option<&'static str> {
        self.redo.last().map(|e| e.label)
    }

    fn step(&mut self, back: bool) {
        self.end_group(); // undoing in the middle of a group takes back the whole group
        let (from, to) = if back { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
        if let Some(mut e) = from.pop() {
            std::mem::swap(&mut e.project, &mut self.project);
            e.merge_key = None;
            to.push(e);
        }
        self.merge_open = false;
    }

    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.project.channels.len().saturating_sub(1));
        self.pattern = self.pattern.min(self.project.patterns.len().saturating_sub(1));
    }

    /// The channel named in props, or the selected one (Blender-style "context").
    fn channel_index(&self, index: Option<usize>) -> Result<usize, String> {
        let i = index.unwrap_or(self.selected);
        if i < self.project.channels.len() { Ok(i) } else { Err(format!("no channel {i}")) }
    }

    fn channel_mut(&mut self, index: Option<usize>) -> Result<&mut Channel, String> {
        let i = self.channel_index(index)?;
        Ok(&mut self.project.channels[i])
    }

    fn synth_mut(&mut self, index: Option<usize>) -> Result<&mut [f32; PARAM_COUNT], String> {
        match &mut self.channel_mut(index)?.instrument {
            Instrument::Synth { params } => Ok(params),
            _ => Err("that channel is a sampler, not a synth".into()),
        }
    }

    fn pattern_mut(&mut self, index: Option<usize>) -> Result<&mut Pattern, String> {
        let i = index.unwrap_or(self.pattern);
        self.project.patterns.get_mut(i).ok_or_else(|| format!("no pattern {i}"))
    }
}
