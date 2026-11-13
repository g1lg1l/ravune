//! Operators: the single entry point for every edit (UI, keymaps, scripts, agents).

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

use crate::*;

/// A named, documented action.
pub struct Operator {
    /// Stable id, e.g. `step.toggle`. Used by keymaps, scripts and agents.
    pub id: &'static str,
    pub label: &'static str,
    /// Shown in Info View, tooltips and agent tool docs.
    pub description: &'static str,
    /// Example props as JSON; `{}` means it runs without input (command palette).
    pub example: &'static str,
    /// Whether it changes the project (and gets an undo step).
    pub undo: bool,
    pub poll: fn(&Session) -> bool,
    pub exec: fn(&mut Session, Value) -> Result<(), String>,
}

pub fn find(id: &str) -> Option<&'static Operator> {
    OPERATORS.iter().find(|o| o.id == id)
}

fn props<T: DeserializeOwned>(v: Value) -> Result<T, String> {
    serde_json::from_value(v).map_err(|e| format!("bad props: {e}"))
}

fn finite(v: f32, what: &str) -> Result<f32, String> {
    if v.is_finite() { Ok(v) } else { Err(format!("{what} must be a number")) }
}

fn always(_: &Session) -> bool {
    true
}

fn has_channels(s: &Session) -> bool {
    !s.project.channels.is_empty()
}

fn has_clips(s: &Session) -> bool {
    !s.project.clips.is_empty()
}

fn selected_is_synth(s: &Session) -> bool {
    s.selected_channel().is_some_and(|c| c.synth().is_some())
}

/// Resolves (pattern, channel slot) for note/step operators.
fn target(s: &mut Session, pattern: Option<usize>, channel: Option<usize>) -> Result<(&mut Pattern, u8), String> {
    let slot = s.project.channels[s.channel_index(channel)?].slot;
    Ok((s.pattern_mut(pattern)?, slot))
}

const MAX_TICK: u32 = MAX_STEPS as u32 * STEP;

fn check_note(n: &Note) -> Result<(), String> {
    if n.key > 127 {
        return Err("key must be 0–127".into());
    }
    if n.length == 0 || n.start >= MAX_TICK {
        return Err(format!("notes need a length and must start before tick {MAX_TICK}"));
    }
    Ok(())
}

#[derive(Deserialize)]
struct SetTempo {
    bpm: f32,
}

#[derive(Deserialize)]
struct Amount {
    amount: f32,
}

#[derive(Deserialize)]
struct Position {
    tick: u32,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LoopRange {
    from: Option<u32>,
    to: Option<u32>,
    on: Option<bool>,
    clear: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SetKey {
    root: Option<u8>,
    scale: Option<Scale>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PatternRef {
    pattern: Option<usize>,
    name: Option<String>,
    steps: Option<usize>,
}

#[derive(Deserialize)]
struct SetColor {
    #[serde(default)]
    pattern: Option<usize>,
    color: Option<[u8; 3]>,
}

#[derive(Deserialize)]
struct StepRef {
    channel: usize,
    step: usize,
    #[serde(default)]
    pattern: Option<usize>,
    #[serde(default)]
    velocity: Option<u8>,
}

#[derive(Deserialize)]
struct AddNote {
    #[serde(default)]
    channel: Option<usize>,
    #[serde(default)]
    pattern: Option<usize>,
    key: u8,
    start: u32,
    length: u32,
    #[serde(default)]
    velocity: Option<u8>,
    #[serde(default)]
    fine: Option<f32>,
    #[serde(default)]
    bright: Option<f32>,
}

#[derive(Deserialize)]
struct SetNote {
    #[serde(default)]
    channel: Option<usize>,
    #[serde(default)]
    pattern: Option<usize>,
    index: usize,
    #[serde(default)]
    key: Option<u8>,
    #[serde(default)]
    start: Option<u32>,
    #[serde(default)]
    length: Option<u32>,
    #[serde(default)]
    velocity: Option<u8>,
    #[serde(default)]
    fine: Option<f32>,
    #[serde(default)]
    bright: Option<f32>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AddNotes {
    channel: Option<usize>,
    pattern: Option<usize>,
    /// Each note: key, start, length, and optionally velocity (1–127).
    notes: Vec<NoteIn>,
}

#[derive(Deserialize)]
struct NoteIn {
    key: u8,
    start: u32,
    length: u32,
    velocity: Option<u8>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Quantize {
    channel: Option<usize>,
    pattern: Option<usize>,
    grid: Option<u32>,
    strength: Option<f32>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AddClip {
    lane: u32,
    start: u32,
    length: Option<u32>,
    offset: u32,
    pattern: Option<usize>,
    /// Makes an audio clip; then `length` is required.
    sample: Option<String>,
    /// Gain, pitch, fades, envelopes (a copy keeps the original's).
    fx: Option<ClipFx>,
    /// Makes an automation clip moving this knob, along `points`.
    automation: Option<Target>,
    points: Vec<(u32, f32)>,
    curves: Vec<f32>,
}

#[derive(Deserialize)]
struct SetPatternAuto {
    #[serde(default)]
    pattern: Option<usize>,
    target: Target,
    #[serde(default)]
    points: Option<Vec<(u32, f32)>>,
    #[serde(default)]
    curves: Option<Vec<f32>>,
    #[serde(default)]
    remove: bool,
}

#[derive(Deserialize)]
struct SetPoints {
    index: usize,
    points: Vec<(u32, f32)>,
    #[serde(default)]
    curves: Option<Vec<f32>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReplaceChannel {
    channel: Option<usize>,
    sample: Option<String>,
    preset: Option<String>,
    plugin: Option<PluginRef>,
}

/// The instrument (and the name it brings) a channel.add / channel.replace asks for: a
/// plugin, else a sample or instrument file, else a VA-1 (from a preset).
fn instrument(sample: Option<String>, preset: Option<String>, plugin: Option<PluginRef>) -> Result<(String, Instrument), String> {
    Ok(match (sample, preset, plugin) {
        (_, _, Some(plugin)) => (plugin.name.clone(), Instrument::Plugin { plugin }),
        (Some(path), _, _) => (sample_name(&path).to_string(), Instrument::from_file(path)),
        (None, Some(preset), None) => {
            let (found, params) = synth_preset(&preset).ok_or_else(|| format!("no VA-1 preset '{preset}'"))?;
            (found.into(), Instrument::Synth { params })
        }
        (None, None, None) => ("VA-1".into(), Instrument::Synth { params: default_synth() }),
    })
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SetLane {
    lane: u32,
    name: Option<String>,
    color: Option<[u8; 3]>,
    /// Back to no color.
    clear_color: bool,
    mute: Option<bool>,
    solo: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct StepTool {
    channel: Option<usize>,
    pattern: Option<usize>,
    /// fill: every how many steps; rotate: by how many (negative = left).
    every: Option<usize>,
    by: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NoteTool {
    channel: Option<usize>,
    pattern: Option<usize>,
    /// humanize: 0..1; strum: ticks between the notes of a chord.
    amount: Option<f32>,
    ticks: Option<u32>,
    seed: Option<u64>,
}

#[derive(Deserialize)]
struct Chord {
    #[serde(default)]
    channel: Option<usize>,
    #[serde(default)]
    pattern: Option<usize>,
    key: u8,
    start: u32,
    length: u32,
    chord: String,
    /// Keep every note in the song's scale.
    #[serde(default)]
    fold: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ChannelMove {
    channel: Option<usize>,
    by: i64,
}

#[derive(Deserialize)]
struct SetFx {
    index: usize,
    #[serde(default)]
    gain: Option<f32>,
    #[serde(default)]
    pitch: Option<f32>,
    #[serde(default)]
    fade_in: Option<u32>,
    #[serde(default)]
    fade_out: Option<u32>,
    #[serde(default)]
    mute: Option<bool>,
    #[serde(default)]
    track: Option<usize>,
    #[serde(default)]
    stretch: Option<Stretch>,
    #[serde(default)]
    bpm: Option<f32>,
    #[serde(default)]
    looped: Option<bool>,
}

#[derive(Deserialize)]
struct SetEnvelope {
    index: usize,
    target: EnvTarget,
    #[serde(default)]
    points: Option<Vec<(u32, f32)>>,
    #[serde(default)]
    curves: Option<Vec<f32>>,
    #[serde(default)]
    shown: Option<bool>,
    #[serde(default)]
    remove: bool,
}

#[derive(Deserialize)]
struct SetClip {
    index: usize,
    #[serde(default)]
    lane: Option<u32>,
    #[serde(default)]
    start: Option<u32>,
    #[serde(default)]
    length: Option<u32>,
    #[serde(default)]
    offset: Option<u32>,
    /// Audio clips: another sound to play.
    #[serde(default)]
    sample: Option<String>,
}

#[derive(Deserialize)]
struct ClipAt {
    index: usize,
    #[serde(default)]
    at: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ChannelRef {
    channel: Option<usize>,
}

#[derive(Deserialize)]
struct MidiLearn {
    #[serde(default)]
    channel: u8,
    cc: u8,
    target: Target,
}

#[derive(Deserialize)]
struct Forget {
    target: Target,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct StoreState {
    channel: Option<usize>,
    id: Option<u32>,
    state: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AddChannel {
    /// Sample path or `builtin:<name>` (an .sfz / .dspreset makes a multi-sample
    /// instrument); omitted = a VA-1 synth.
    sample: Option<String>,
    name: Option<String>,
    /// VA-1 preset name (see `SYNTH_PRESETS`).
    preset: Option<String>,
    /// A CLAP / VST3 instrument instead.
    plugin: Option<PluginRef>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TrackProps {
    track: usize,
    name: Option<String>,
    gain: Option<f32>,
    pan: Option<f32>,
    /// The strip's EQ bands, in dB.
    low: Option<f32>,
    mid: Option<f32>,
    high: Option<f32>,
    mute: Option<bool>,
    solo: Option<bool>,
    route: Option<usize>,
    to: Option<usize>,
    amount: Option<f32>,
    /// Sidechain source track.
    source: Option<usize>,
    color: Option<[u8; 3]>,
    /// Back to the palette's color.
    auto_color: bool,
    /// An icon name ("" = automatic).
    icon: Option<String>,
    /// A ghost track, out of the mixer's sight unless its ghosts are shown.
    hidden: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DeviceStyle {
    id: u32,
    /// "" = its kind's name.
    name: Option<String>,
    color: Option<[u8; 3]>,
    auto_color: bool,
    /// "" = its kind's icon.
    icon: Option<String>,
}

/// An icon name as kept: empty means automatic.
fn icon_name(icon: String) -> Result<Option<String>, String> {
    match icon.trim() {
        "" => Ok(None),
        name if name.len() <= 32 => Ok(Some(name.to_string())),
        _ => Err("that's not an icon name".into()),
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DeviceProps {
    id: u32,
    track: usize,
    kind: Option<DeviceKind>,
    index: Option<usize>,
    param: Option<usize>,
    value: Option<f32>,
    /// Sidechain source track (absent = off).
    source: Option<usize>,
    /// For kind "plugin": the CLAP / VST3 effect.
    plugin: Option<PluginRef>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MacroTool {
    index: Option<usize>,
    seed: Option<u64>,
}

/// Variations a project keeps at most (macro.store says the number).
pub const VARIATIONS: usize = 8;

#[derive(Deserialize, Default)]
#[serde(default)]
struct ModProps {
    index: usize,
    value: Option<f32>,
    name: Option<String>,
    shape: Option<u8>,
    hz: Option<f32>,
    sync: Option<bool>,
    beats: Option<f32>,
    source: Option<Source>,
    target: Option<Target>,
    amount: Option<f32>,
}

fn track_index(s: &Session, track: usize) -> Result<usize, String> {
    if track < s.project.mixer.len() { Ok(track) } else { Err(format!("no mixer track {track}")) }
}

fn check_target(p: &Project, t: Target) -> Result<(), String> {
    let ok = match t {
        Target::Synth { slot, .. } => p.channels.iter().any(|c| c.slot == slot && c.synth().is_some()),
        Target::Device { id, param } => p.device(id).is_some_and(|(_, _, d)| param < d.params.len()),
        Target::TrackGain { track } | Target::TrackPan { track } => track < p.mixer.len(),
        Target::ChannelGain { slot } => p.channels.iter().any(|c| c.slot == slot),
        Target::Macro { index } => (index as usize) < MACROS,
    };
    if ok { Ok(()) } else { Err("that knob doesn't exist".into()) }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SetChannel {
    channel: Option<usize>,
    track: Option<usize>,
    name: Option<String>,
    gain: Option<f32>,
    mute: Option<bool>,
    pitch: Option<f32>,
    choke: Option<u8>,
    sample: Option<String>,
    attack: Option<f32>,
    release: Option<f32>,
    start: Option<f32>,
    end: Option<f32>,
    looped: Option<bool>,
    swing: Option<f32>,
}

#[derive(Deserialize)]
struct SetParam {
    #[serde(default)]
    channel: Option<usize>,
    param: Param,
    value: f32,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Surprise {
    channel: Option<usize>,
    seed: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MarkerProps {
    index: Option<usize>,
    tick: Option<u32>,
    name: Option<String>,
}

/// What a pattern does in a song, from what plays in it: the kick, other drums, a bass
/// (a synth playing low), or the music (the other instruments).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Role {
    Kick,
    Drums,
    Bass,
    Music,
}

fn role(project: &Project, pattern: &Pattern) -> Option<Role> {
    let roles = pattern.notes.iter().filter(|(_, n)| !n.is_empty()).filter_map(|(slot, notes)| {
        let c = project.channels.iter().find(|c| c.slot == *slot)?;
        let named = |w: &str| c.name.to_lowercase().contains(w) || matches!(&c.instrument, Instrument::Sampler { sample } if sample.to_lowercase().contains(w));
        Some(match &c.instrument {
            Instrument::Sampler { .. } if named("kick") || named("bd") => Role::Kick,
            Instrument::Sampler { .. } => Role::Drums,
            _ if notes.iter().map(|n| n.key as u32).sum::<u32>() < 48 * notes.len() as u32 => Role::Bass,
            _ => Role::Music,
        })
    });
    // mixed: the kick makes it the foundation, then the most melodic part decides
    roles.max_by_key(|r| match r {
        Role::Kick => 3,
        Role::Music => 2,
        Role::Bass => 1,
        Role::Drums => 0,
    })
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct StructureProps {
    /// Bars per section, scaled (1 = intro 16, build 16, drop 32, break 16, drop 32, outro 16).
    scale: Option<f32>,
}

fn marker_index(s: &Session, index: Option<usize>) -> Result<usize, String> {
    let i = index.ok_or("which marker? (index)")?;
    if i < s.project.markers.len() { Ok(i) } else { Err(format!("no marker {i}")) }
}

pub static OPERATORS: &[Operator] = &[
    // ---- transport ----
    Operator {
        id: "transport.play_toggle",
        label: "Play / Stop",
        description: "Start or stop playback (the pattern in PAT mode, the playlist in SONG mode).",
        example: "{}",
        undo: false,
        poll: always,
        exec: |s, _| {
            s.playing = !s.playing;
            Ok(())
        },
    },
    Operator {
        id: "transport.mode_toggle",
        label: "Pattern / Song Mode",
        description: "Switch between looping the current pattern and playing the playlist.",
        example: "{}",
        undo: false,
        poll: always,
        exec: |s, _| {
            s.song_mode = !s.song_mode;
            Ok(())
        },
    },
    Operator {
        id: "transport.set_position",
        label: "Set Song Position",
        description: "Where song playback starts, in ticks (96 per beat).",
        example: r#"{"tick": 384}"#,
        undo: false,
        poll: always,
        exec: |s, v| {
            let Position { tick } = props(v)?;
            s.position = tick;
            Ok(())
        },
    },
    Operator {
        id: "transport.loop",
        label: "Loop Region",
        description: "Play only part of the song, over and over: from and to in ticks (96 per beat) set the region and turn it on; on switches it on or off (the region stays); clear drops it.",
        example: r#"{"from": 0, "to": 1536}"#,
        undo: false,
        poll: always,
        exec: |s, v| {
            let LoopRange { from, to, on, clear } = props(v)?;
            if clear {
                (s.loop_region, s.loop_on) = (None, false);
            }
            if let (Some(from), Some(to)) = (from, to) {
                if to <= from {
                    return Err("a loop ends after it starts".into());
                }
                (s.loop_region, s.loop_on) = (Some((from, to)), true);
            }
            if let Some(on) = on {
                s.loop_on = on && s.loop_region.is_some();
            }
            s.loop_range = s.loop_region.filter(|_| s.loop_on);
            Ok(())
        },
    },
    Operator {
        id: "transport.jump",
        label: "Jump Here",
        description: "Move the song position (ticks) and, if the song is playing, the playhead with it.",
        example: r#"{"tick": 384}"#,
        undo: false,
        poll: always,
        exec: |s, v| {
            let Position { tick } = props(v)?;
            s.position = tick;
            s.jumps = s.jumps.wrapping_add(1);
            Ok(())
        },
    },
    Operator {
        id: "transport.set_tempo",
        label: "Set Tempo",
        description: "Set the song tempo in beats per minute (20–300).",
        example: r#"{"bpm": 120}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let SetTempo { bpm } = props(v)?;
            s.project.tempo = finite(bpm, "tempo")?.clamp(20.0, 300.0);
            Ok(())
        },
    },
    Operator {
        id: "transport.metronome_toggle",
        label: "Toggle Metronome",
        description: "Turn the click track on or off while playing.",
        example: "{}",
        undo: true,
        poll: always,
        exec: |s, _| {
            s.project.metronome = !s.project.metronome;
            Ok(())
        },
    },
    Operator {
        id: "transport.set_swing",
        label: "Set Swing",
        description: "Delay every second 16th step for a shuffled groove (0–100%).",
        example: r#"{"amount": 0.3}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let Amount { amount } = props(v)?;
            s.project.swing = finite(amount, "swing")?.clamp(0.0, 1.0);
            Ok(())
        },
    },
    Operator {
        id: "song.set_key",
        label: "Set Key",
        description: "Song key: root 0–11 (0 = C) and scale (major, minor, dorian, …).",
        example: r#"{"root": 9, "scale": "minor"}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let SetKey { root, scale } = props(v)?;
            if let Some(r) = root {
                s.project.root = r % 12;
            }
            if let Some(sc) = scale {
                s.project.scale = sc;
            }
            Ok(())
        },
    },
    // ---- patterns ----
    Operator {
        id: "pattern.add",
        label: "New Pattern",
        description: "Add an empty pattern and select it.",
        example: "{}",
        undo: true,
        poll: always,
        exec: |s, v| {
            let PatternRef { name, .. } = props(v)?;
            let n = s.project.patterns.len();
            let mut p = Pattern::new(name.unwrap_or_else(|| format!("Pattern {}", n + 1)));
            p.color = Some(PATTERN_COLORS[n % PATTERN_COLORS.len()]);
            s.project.patterns.push(p);
            s.pattern = n;
            Ok(())
        },
    },
    Operator {
        id: "pattern.duplicate",
        label: "Clone Pattern",
        description: "Copy the current pattern (its notes and steps) and select the copy.",
        example: "{}",
        undo: true,
        poll: always,
        exec: |s, v| {
            let PatternRef { pattern, .. } = props(v)?;
            let mut copy = s.pattern_mut(pattern)?.clone();
            copy.name.push_str(" copy");
            s.project.patterns.push(copy);
            s.pattern = s.project.patterns.len() - 1;
            Ok(())
        },
    },
    Operator {
        id: "pattern.select",
        label: "Select Pattern",
        description: "Show a pattern in the channel rack and piano roll.",
        example: r#"{"pattern": 1}"#,
        undo: false,
        poll: always,
        exec: |s, v| {
            let PatternRef { pattern, .. } = props(v)?;
            let i = pattern.ok_or("pattern is required")?;
            if i >= s.project.patterns.len() {
                return Err(format!("no pattern {i}"));
            }
            s.pattern = i;
            Ok(())
        },
    },
    Operator {
        id: "pattern.remove",
        label: "Delete Pattern",
        description: "Delete a pattern and its clips from the playlist (there is always one left).",
        example: "{}",
        undo: true,
        poll: |s| s.project.patterns.len() > 1,
        exec: |s, v| {
            let PatternRef { pattern, .. } = props(v)?;
            let i = pattern.unwrap_or(s.pattern);
            if i >= s.project.patterns.len() {
                return Err(format!("no pattern {i}"));
            }
            s.project.patterns.remove(i);
            s.project.clips.retain(|c| c.kind != ClipKind::Pattern { pattern: i });
            for c in &mut s.project.clips {
                if let ClipKind::Pattern { pattern } = &mut c.kind
                    && *pattern > i
                {
                    *pattern -= 1;
                }
            }
            Ok(())
        },
    },
    Operator {
        id: "pattern.rename",
        label: "Rename Pattern",
        description: "Give a pattern a name, e.g. \"Chorus drums\".",
        example: r#"{"name": "Intro"}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let PatternRef { pattern, name, .. } = props(v)?;
            s.pattern_mut(pattern)?.name = name.ok_or("name is required")?;
            Ok(())
        },
    },
    Operator {
        id: "pattern.set_color",
        label: "Set Pattern Color",
        description: "Color a pattern (RGB, 0–255 each); null goes back to its palette color.",
        example: r#"{"color": [232, 106, 60]}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let SetColor { pattern, color } = props(v)?;
            s.pattern_mut(pattern)?.color = color;
            Ok(())
        },
    },
    Operator {
        id: "pattern.set_length",
        label: "Set Pattern Length",
        description: "Pattern length in 16th steps (1–512). Its clips on the playlist that were exactly one pattern long follow.",
        example: r#"{"steps": 32}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let PatternRef { pattern, steps, .. } = props(v)?;
            let steps = steps.ok_or("steps is required")?.clamp(1, MAX_STEPS);
            let index = pattern.unwrap_or(s.pattern);
            let p = s.pattern_mut(pattern)?;
            let old = p.ticks();
            p.length = steps;
            let new = p.ticks();
            for c in s.project.clips.iter_mut().filter(|c| c.kind == ClipKind::Pattern { pattern: index } && c.length == old && c.offset == 0) {
                c.length = new;
            }
            Ok(())
        },
    },
    Operator {
        id: "pattern.automation",
        label: "Pattern Automation",
        description: "A knob's line inside a pattern (default: the current one); it plays wherever the pattern does. Points are [tick from the pattern's start, 0–1 along the knob]; curves bend each stretch between two points (-1..1, 0 straight). remove: true drops the line.",
        example: r#"{"target": {"type": "track_gain", "track": 1}, "points": [[0, 0.2], [384, 0.8]]}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let p: SetPatternAuto = props(v)?;
            check_target(&s.project, p.target)?;
            let pat = s.pattern_mut(p.pattern)?;
            if p.remove {
                pat.automation.retain(|a| a.target != p.target);
                return Ok(());
            }
            match pat.automation.iter_mut().find(|a| a.target == p.target) {
                Some(lane) => {
                    lane.points = p.points.map_or(std::mem::take(&mut lane.points), clean_points);
                    lane.curves = clean_curves(p.curves.unwrap_or(std::mem::take(&mut lane.curves)), lane.points.len());
                }
                None => {
                    let points = clean_points(p.points.unwrap_or_else(|| vec![(0, 0.5)]));
                    let curves = clean_curves(p.curves.unwrap_or_default(), points.len());
                    pat.automation.push(PatternAuto { target: p.target, points, curves });
                }
            }
            Ok(())
        },
    },
    Operator {
        id: "pattern.clear",
        label: "Clear Pattern",
        description: "Remove every step and note from the current pattern.",
        example: "{}",
        undo: true,
        poll: always,
        exec: |s, v| {
            let PatternRef { pattern, .. } = props(v)?;
            s.pattern_mut(pattern)?.notes.clear();
            Ok(())
        },
    },
    // ---- steps & notes ----
    Operator {
        id: "step.toggle",
        label: "Toggle Step",
        description: "Turn a step on or off. Channel and step are 0-based; the current pattern unless given.",
        example: r#"{"channel": 0, "step": 4}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let StepRef { channel, step, pattern, velocity } = props(v)?;
            let (p, slot) = target(s, pattern, Some(channel))?;
            if step >= MAX_STEPS {
                return Err(format!("no step {step}"));
            }
            let on = p.step(slot, step) == 0;
            p.set_step(slot, step, if on { velocity.unwrap_or(DEFAULT_VELOCITY).clamp(1, 127) } else { 0 });
            Ok(())
        },
    },
    Operator {
        id: "step.set",
        label: "Set Step",
        description: "Set a step's velocity (0 = off, 1–127 = on).",
        example: r#"{"channel": 0, "step": 4, "velocity": 100}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let StepRef { channel, step, pattern, velocity } = props(v)?;
            let velocity = velocity.ok_or("velocity is required")?.min(127);
            let (p, slot) = target(s, pattern, Some(channel))?;
            if step >= MAX_STEPS {
                return Err(format!("no step {step}"));
            }
            p.set_step(slot, step, velocity);
            Ok(())
        },
    },
    Operator {
        id: "note.add",
        label: "Add Note",
        description: "Draw a note in the piano roll: key (60 = C4), start and length in ticks (96 per beat).",
        example: r#"{"key": 60, "start": 0, "length": 24}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let AddNote { channel, pattern, key, start, length, velocity, fine, bright } = props(v)?;
            let note = Note {
                key,
                start,
                length,
                velocity: velocity.unwrap_or(DEFAULT_VELOCITY).clamp(1, 127),
                fine: fine.unwrap_or(0.0).clamp(-2.0, 2.0),
                bright: bright.unwrap_or(0.5).clamp(0.0, 1.0),
            };
            check_note(&note)?;
            let (p, slot) = target(s, pattern, channel)?;
            p.notes_mut(slot).push(note);
            Ok(())
        },
    },
    Operator {
        id: "note.set",
        label: "Edit Note",
        description: "Move, resize or change a note (by its index in the channel's notes): key, start, length, velocity, fine pitch (-2..2 semitones), brightness (0..1).",
        example: r#"{"index": 0, "start": 48, "key": 62}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let p: SetNote = props(v)?;
            let (pat, slot) = target(s, p.pattern, p.channel)?;
            let note = pat.notes_mut(slot).get_mut(p.index).ok_or_else(|| format!("no note {}", p.index))?;
            let mut n = *note;
            n.key = p.key.unwrap_or(n.key);
            n.start = p.start.unwrap_or(n.start);
            n.length = p.length.unwrap_or(n.length);
            n.velocity = p.velocity.map_or(n.velocity, |v| v.clamp(1, 127));
            n.fine = p.fine.map_or(n.fine, |f| f.clamp(-2.0, 2.0));
            n.bright = p.bright.map_or(n.bright, |b| b.clamp(0.0, 1.0));
            check_note(&n)?;
            *note = n;
            Ok(())
        },
    },
    Operator {
        id: "note.remove",
        label: "Delete Note",
        description: "Delete a note by its index in the channel's notes.",
        example: r#"{"index": 0}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let p: SetNote = props(v)?;
            let (pat, slot) = target(s, p.pattern, p.channel)?;
            let notes = pat.notes_mut(slot);
            if p.index >= notes.len() {
                return Err(format!("no note {}", p.index));
            }
            notes.remove(p.index);
            Ok(())
        },
    },
    Operator {
        id: "notes.quantize",
        label: "Quantize Notes",
        description: "Snap note starts to a grid in ticks (default 24 = 1/16); strength 0..1 moves them only that far (default 1, all the way).",
        example: "{}",
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let Quantize { channel, pattern, grid, strength } = props(v)?;
            let grid = grid.unwrap_or(STEP).max(1);
            let strength = strength.unwrap_or(1.0).clamp(0.0, 1.0) as f64;
            let (p, slot) = target(s, pattern, channel)?;
            for n in p.notes_mut(slot) {
                let snapped = ((n.start + grid / 2) / grid * grid).min(MAX_TICK - grid) as f64;
                n.start = (n.start as f64 + (snapped - n.start as f64) * strength).round() as u32;
            }
            Ok(())
        },
    },
    // ---- playlist ----
    Operator {
        id: "clip.add",
        label: "Add Clip",
        description: "Place a pattern (default: the current one), an audio sample or an automation clip (a knob's target and its points) on the playlist.",
        example: r#"{"lane": 0, "start": 0}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let AddClip { lane, start, length, offset, pattern, sample, fx, automation, points, curves } = props(v)?;
            let (kind, default_len) = match (sample, automation) {
                (Some(sample), _) => (ClipKind::Audio { sample }, None),
                (None, Some(target)) => {
                    let points = clean_points(points);
                    let curves = clean_curves(curves, points.len());
                    (ClipKind::Automation { target, points, curves }, Some(4 * BAR))
                }
                (None, None) => {
                    let i = pattern.unwrap_or(s.pattern);
                    let p = s.project.patterns.get(i).ok_or_else(|| format!("no pattern {i}"))?;
                    (ClipKind::Pattern { pattern: i }, Some(p.ticks()))
                }
            };
            let length = length.or(default_len).ok_or("audio clips need a length")?;
            if length == 0 {
                return Err("clips need a length".into());
            }
            s.project.clips.push(Clip { lane, start, length, offset, kind, fx: fx.unwrap_or_default() });
            Ok(())
        },
    },
    Operator {
        id: "clip.set",
        label: "Move Clip",
        description: "Move, resize or trim a playlist clip (by index), or give an audio clip another sound (sample: a file).",
        example: r#"{"index": 0, "start": 384}"#,
        undo: true,
        poll: has_clips,
        exec: |s, v| {
            let p: SetClip = props(v)?;
            let c = s.project.clips.get_mut(p.index).ok_or_else(|| format!("no clip {}", p.index))?;
            c.lane = p.lane.unwrap_or(c.lane);
            c.start = p.start.unwrap_or(c.start);
            c.length = p.length.unwrap_or(c.length).max(1);
            c.offset = p.offset.unwrap_or(c.offset);
            if let Some(path) = p.sample {
                let ClipKind::Audio { sample } = &mut c.kind else { return Err("only an audio clip plays a sound".into()) };
                *sample = path;
            }
            Ok(())
        },
    },
    Operator {
        id: "clip.remove",
        label: "Delete Clip",
        description: "Remove a clip from the playlist.",
        example: r#"{"index": 0}"#,
        undo: true,
        poll: has_clips,
        exec: |s, v| {
            let ClipAt { index, .. } = props(v)?;
            if index >= s.project.clips.len() {
                return Err(format!("no clip {index}"));
            }
            s.project.clips.remove(index);
            Ok(())
        },
    },
    Operator {
        id: "clip.duplicate",
        label: "Duplicate Clip",
        description: "Put a copy of a clip right after it on the same lane.",
        example: r#"{"index": 0}"#,
        undo: true,
        poll: has_clips,
        exec: |s, v| {
            let ClipAt { index, .. } = props(v)?;
            let mut copy = s.project.clips.get(index).ok_or_else(|| format!("no clip {index}"))?.clone();
            copy.start = copy.end();
            s.project.clips.push(copy);
            Ok(())
        },
    },
    Operator {
        id: "clip.split",
        label: "Split Clip",
        description: "Cut a clip in two at a song position (in ticks).",
        example: r#"{"index": 0, "at": 384}"#,
        undo: true,
        poll: has_clips,
        exec: |s, v| {
            let ClipAt { index, at } = props(v)?;
            let at = at.ok_or("at is required")?;
            let c = s.project.clips.get_mut(index).ok_or_else(|| format!("no clip {index}"))?;
            if at <= c.start || at >= c.end() {
                return Err("split point must be inside the clip".into());
            }
            let mut right = c.clone();
            c.length = at - c.start;
            right.offset += at - right.start;
            right.length = right.end() - at;
            right.start = at;
            s.project.clips.push(right);
            Ok(())
        },
    },
    Operator {
        id: "clip.set_fx",
        label: "Clip Properties",
        description: "A clip's gain (dB, -60 to +12), pitch (semitones, -24 to 24), fade in / out (ticks), mute, the mixer track it plays into, and how it follows the tempo: stretch (off, resample, stretch) from the sound's own bpm (20 to 400; 0 = not known), and looped (the sound repeats under the clip).",
        example: r#"{"index": 0, "gain": -3, "fade_in": 96}"#,
        undo: true,
        poll: has_clips,
        exec: |s, v| {
            let p: SetFx = props(v)?;
            if let Some(t) = p.track {
                track_index(s, t)?;
            }
            let c = s.project.clips.get_mut(p.index).ok_or_else(|| format!("no clip {}", p.index))?;
            c.fx.track = p.track.unwrap_or(c.fx.track);
            let fx = &mut c.fx;
            fx.gain = p.gain.map_or(fx.gain, |g| g.clamp(-60.0, 12.0));
            fx.pitch = p.pitch.map_or(fx.pitch, |x| x.clamp(-24.0, 24.0));
            fx.fade_in = p.fade_in.map_or(fx.fade_in, |f| f.min(c.length));
            fx.fade_out = p.fade_out.map_or(fx.fade_out, |f| f.min(c.length));
            fx.mute = p.mute.unwrap_or(fx.mute);
            fx.stretch = p.stretch.unwrap_or(fx.stretch);
            fx.bpm = p.bpm.map_or(fx.bpm, |b| if b > 0.0 { b.clamp(20.0, 400.0) } else { 0.0 });
            fx.looped = p.looped.unwrap_or(fx.looped);
            Ok(())
        },
    },
    Operator {
        id: "clip.envelope",
        label: "Clip Envelope",
        description: "Add, change or remove a clip envelope (volume, pan, filter). Points are [tick from where the clip's content starts, value 0–1]; curves bend each stretch between two points (-1..1, 0 straight).",
        example: r#"{"index": 0, "target": "filter", "points": [[0, 0.1], [384, 0.5]]}"#,
        undo: true,
        poll: has_clips,
        exec: |s, v| {
            let p: SetEnvelope = props(v)?;
            let c = s.project.clips.get_mut(p.index).ok_or_else(|| format!("no clip {}", p.index))?;
            if p.remove {
                c.fx.envelopes.retain(|e| e.target != p.target);
                return Ok(());
            }
            let (from, to) = (c.offset, c.offset + c.length);
            let i = match c.fx.envelopes.iter().position(|e| e.target == p.target) {
                Some(i) => i,
                None => {
                    let n = p.target.neutral();
                    c.fx.envelopes.push(Envelope { target: p.target, points: vec![(from, n), (to, n)], curves: Vec::new(), shown: true });
                    c.fx.envelopes.len() - 1
                }
            };
            let env = &mut c.fx.envelopes[i];
            if let Some(points) = p.points {
                env.points = clean_points(points);
            }
            env.curves = clean_curves(p.curves.unwrap_or(std::mem::take(&mut env.curves)), env.points.len());
            env.shown = p.shown.unwrap_or(env.shown);
            Ok(())
        },
    },
    Operator {
        id: "clip.automation",
        label: "Automation Points",
        description: "An automation clip's points: [tick from where the clip's content starts, 0–1 along the knob's travel]; curves bend each stretch between two points (-1..1, 0 straight).",
        example: r#"{"index": 0, "points": [[0, 0.2], [1536, 0.9]]}"#,
        undo: true,
        poll: has_clips,
        exec: |s, v| {
            let p: SetPoints = props(v)?;
            match s.project.clips.get_mut(p.index).map(|c| &mut c.kind) {
                Some(ClipKind::Automation { points, curves, .. }) => {
                    *points = clean_points(p.points);
                    *curves = clean_curves(p.curves.unwrap_or(std::mem::take(curves)), points.len());
                }
                Some(_) => return Err("not an automation clip".into()),
                None => return Err(format!("no clip {}", p.index)),
            }
            Ok(())
        },
    },
    Operator {
        id: "clip.make_unique",
        label: "Make Unique",
        description: "Give a pattern clip its own copy of the pattern, so editing it changes only this clip.",
        example: r#"{"index": 0}"#,
        undo: true,
        poll: has_clips,
        exec: |s, v| {
            let ClipAt { index, .. } = props(v)?;
            let ClipKind::Pattern { pattern } = s.project.clips.get(index).ok_or_else(|| format!("no clip {index}"))?.kind else {
                return Err("only pattern clips share a pattern".into());
            };
            let mut copy = s.project.patterns[pattern].clone();
            copy.name.push_str(" (unique)");
            s.project.patterns.push(copy);
            s.pattern = s.project.patterns.len() - 1;
            s.project.clips[index].kind = ClipKind::Pattern { pattern: s.pattern };
            Ok(())
        },
    },
    // ---- playlist lanes ----
    Operator {
        id: "lane.set",
        label: "Lane Settings",
        description: "Name, color, mute or solo a playlist lane (0-based).",
        example: r#"{"lane": 0, "name": "Drums", "solo": true}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let p: SetLane = props(v)?;
            let i = p.lane as usize;
            if s.project.lanes.len() <= i {
                s.project.lanes.resize(i + 1, LaneInfo::default());
            }
            let l = &mut s.project.lanes[i];
            l.name = p.name.unwrap_or(std::mem::take(&mut l.name));
            if p.clear_color {
                l.color = None;
            }
            l.color = p.color.or(l.color);
            l.mute = p.mute.unwrap_or(l.mute);
            l.solo = p.solo.unwrap_or(l.solo);
            // nothing left to remember: trim the plain lanes at the end
            while s.project.lanes.last().is_some_and(|l| *l == LaneInfo::default()) {
                s.project.lanes.pop();
            }
            Ok(())
        },
    },
    Operator {
        id: "lane.insert",
        label: "Insert Lane",
        description: "Open an empty lane: clips on it and below move down one.",
        example: r#"{"lane": 2}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let SetLane { lane, .. } = props(v)?;
            s.project.clips.iter_mut().filter(|c| c.lane >= lane).for_each(|c| c.lane += 1);
            if (lane as usize) < s.project.lanes.len() {
                s.project.lanes.insert(lane as usize, LaneInfo::default());
            }
            Ok(())
        },
    },
    Operator {
        id: "lane.remove",
        label: "Delete Lane",
        description: "Delete a lane with its clips; the lanes below move up one.",
        example: r#"{"lane": 2}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let SetLane { lane, .. } = props(v)?;
            s.project.clips.retain(|c| c.lane != lane);
            s.project.clips.iter_mut().filter(|c| c.lane > lane).for_each(|c| c.lane -= 1);
            if (lane as usize) < s.project.lanes.len() {
                s.project.lanes.remove(lane as usize);
            }
            Ok(())
        },
    },
    // ---- rack tools ----
    Operator {
        id: "marker.add",
        label: "Add Marker",
        description: "Put a named marker on the timeline (tick, default: the song position; name, default: Marker N). The ruler shows it; a click there jumps to it.",
        example: "{}",
        undo: true,
        poll: always,
        exec: |s, v| {
            let p: MarkerProps = props(v)?;
            let tick = p.tick.unwrap_or(s.position);
            let name = p.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("Marker {}", s.project.markers.len() + 1));
            s.project.markers.push(Marker { tick, name });
            s.project.markers.sort_by_key(|m| m.tick);
            Ok(())
        },
    },
    Operator {
        id: "marker.set",
        label: "Edit Marker",
        description: "Move (tick) or rename (name) a marker, by its index in time order.",
        example: r#"{"index": 0, "name": "Drop"}"#,
        undo: true,
        poll: |s| !s.project.markers.is_empty(),
        exec: |s, v| {
            let p: MarkerProps = props(v)?;
            let i = marker_index(s, p.index)?;
            let m = &mut s.project.markers[i];
            m.tick = p.tick.unwrap_or(m.tick);
            if let Some(name) = p.name.filter(|n| !n.trim().is_empty()) {
                m.name = name;
            }
            s.project.markers.sort_by_key(|m| m.tick);
            Ok(())
        },
    },
    Operator {
        id: "marker.remove",
        label: "Delete Marker",
        description: "Take a marker off the timeline, by its index in time order.",
        example: r#"{"index": 0}"#,
        undo: true,
        poll: |s| !s.project.markers.is_empty(),
        exec: |s, v| {
            let p: MarkerProps = props(v)?;
            let i = marker_index(s, p.index)?;
            s.project.markers.remove(i);
            Ok(())
        },
    },
    Operator {
        id: "song.structure",
        label: "Build a Song From the Patterns",
        description: "Lays the patterns out as a song after the end of the playlist: intro, build, drop, break, drop 2, outro, in 8-bar phrases, each with a marker. Kick and drums start, the bass joins, the break drops the kick, the outro thins out.",
        example: "{}",
        undo: true,
        poll: |s| s.project.patterns.iter().any(|p| p.notes.values().any(|n| !n.is_empty())),
        exec: |s, v| {
            let p: StructureProps = props(v)?;
            let k = p.scale.unwrap_or(1.0).clamp(0.25, 4.0);
            let bars = |n: f32| ((n * k / 8.0).round().max(1.0) * 8.0) as u32;
            let roles: Vec<(usize, Role)> = s.project.patterns.iter().enumerate().filter_map(|(i, p)| role(&s.project, p).map(|r| (i, r))).collect();
            if roles.is_empty() {
                return Err("the patterns are empty: put some notes in first".into());
            }
            let has = |r: Role| roles.iter().any(|(_, x)| *x == r);
            // without a kick, the drums are the foundation
            let base = if has(Role::Kick) { Role::Kick } else { Role::Drums };
            let sections: [(&str, f32, &[Role]); 6] = [
                ("Intro", 16.0, &[base, Role::Drums]),
                ("Build", 16.0, &[base, Role::Drums, Role::Bass]),
                ("Drop", 32.0, &[Role::Kick, Role::Drums, Role::Bass, Role::Music]),
                ("Break", 16.0, if has(Role::Music) { &[Role::Music, Role::Drums] } else { &[Role::Drums, Role::Bass] }),
                ("Drop 2", 32.0, &[Role::Kick, Role::Drums, Role::Bass, Role::Music]),
                ("Outro", 16.0, &[base, Role::Drums]),
            ];
            let mut at = if s.project.clips.is_empty() { 0 } else { s.project.song_end().div_ceil(BAR) * BAR };
            for (name, length, playing) in sections {
                let length = bars(length) * BAR;
                s.project.markers.push(Marker { tick: at, name: name.into() });
                for (lane, (pattern, r)) in roles.iter().enumerate() {
                    if playing.contains(r) {
                        s.project.clips.push(Clip { lane: lane as u32, start: at, length, offset: 0, kind: ClipKind::Pattern { pattern: *pattern }, fx: ClipFx::default() });
                    }
                }
                at += length;
            }
            s.project.markers.sort_by_key(|m| m.tick);
            s.song_mode = true;
            Ok(())
        },
    },
    Operator {
        id: "steps.fill",
        label: "Fill Each N Steps",
        description: "Replace a channel's steps with one every N steps (4 = each beat).",
        example: r#"{"channel": 0, "every": 4}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let StepTool { channel, pattern, every, .. } = props(v)?;
            let every = every.ok_or("every is required")?.max(1);
            let (p, slot) = target(s, pattern, channel)?;
            p.notes_mut(slot).retain(|n| !n.is_step());
            for step in (0..p.length).step_by(every) {
                p.set_step(slot, step, DEFAULT_VELOCITY);
            }
            Ok(())
        },
    },
    Operator {
        id: "steps.rotate",
        label: "Rotate",
        description: "Shift a channel's notes by some steps, wrapping around the pattern (negative = left).",
        example: r#"{"channel": 0, "by": 1}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let StepTool { channel, pattern, by, .. } = props(v)?;
            let (p, slot) = target(s, pattern, channel)?;
            let length = p.ticks() as i64;
            let shift = by.unwrap_or(1) * STEP as i64;
            for n in p.notes_mut(slot) {
                n.start = (n.start as i64 + shift).rem_euclid(length) as u32;
            }
            Ok(())
        },
    },
    Operator {
        id: "channel.solo",
        label: "Solo Channel",
        description: "Mute every other channel; again to unmute them all.",
        example: r#"{"channel": 0}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let ChannelRef { channel } = props(v)?;
            let i = s.channel_index(channel)?;
            let soloed = s.project.channels.iter().enumerate().all(|(k, c)| c.mute == (k != i));
            for (k, c) in s.project.channels.iter_mut().enumerate() {
                c.mute = !soloed && k != i;
            }
            Ok(())
        },
    },
    Operator {
        id: "channel.clone",
        label: "Clone Channel",
        description: "A copy of a channel (instrument, settings and its notes in every pattern) right below it.",
        example: r#"{"channel": 0}"#,
        undo: true,
        poll: |s| has_channels(s) && s.project.channels.len() < MAX_CHANNELS,
        exec: |s, v| {
            let ChannelRef { channel } = props(v)?;
            let i = s.channel_index(channel)?;
            let slot = s.project.free_slot().ok_or("all 64 channels are in use")?;
            let mut copy = s.project.channels[i].clone();
            let from = copy.slot;
            copy.slot = slot;
            copy.name.push_str(" 2");
            for p in &mut s.project.patterns {
                let notes = p.notes(from).to_vec();
                if !notes.is_empty() {
                    p.notes.insert(slot, notes);
                }
            }
            s.project.channels.insert(i + 1, copy);
            s.selected = i + 1;
            Ok(())
        },
    },
    Operator {
        id: "channel.move",
        label: "Move Channel",
        description: "Move a channel up (negative) or down in the rack.",
        example: r#"{"channel": 2, "by": -1}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let ChannelMove { channel, by } = props(v)?;
            let i = s.channel_index(channel)?;
            let to = (i as i64 + by).clamp(0, s.project.channels.len() as i64 - 1) as usize;
            let c = s.project.channels.remove(i);
            s.project.channels.insert(to, c);
            s.selected = to;
            Ok(())
        },
    },
    // ---- piano roll tools ----
    Operator {
        id: "notes.chord",
        label: "Stamp Chord",
        description: "Draw a chord (Major, Minor, 7, Maj7, Min7, Sus2, Sus4, Dim, Aug, Power) from a root key.",
        example: r#"{"key": 60, "start": 0, "length": 96, "chord": "Minor"}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let c: Chord = props(v)?;
            let intervals = CHORDS.iter().find(|(n, _)| n.eq_ignore_ascii_case(&c.chord)).ok_or_else(|| format!("no chord '{}'", c.chord))?.1;
            let (root, scale) = (s.project.root, s.project.scale);
            let (p, slot) = target(s, c.pattern, c.channel)?;
            let mut keys: Vec<u8> = intervals.iter().map(|i| c.key.saturating_add(*i).min(127)).map(|k| if c.fold { scale.fold(root, k) } else { k }).collect();
            keys.dedup();
            for key in keys {
                let note = Note { key, start: c.start, length: c.length.max(1), ..Default::default() };
                check_note(&note)?;
                p.notes_mut(slot).push(note);
            }
            Ok(())
        },
    },
    Operator {
        id: "notes.add",
        label: "Add Notes",
        description: "Draw many notes at once on a channel in a pattern: a list of key (60 = C4), start and length in ticks (96 per beat), and velocity (1–127, optional).",
        example: r#"{"notes": [{"key": 60, "start": 0, "length": 96}, {"key": 64, "start": 96, "length": 96, "velocity": 90}]}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let AddNotes { channel, pattern, notes } = props(v)?;
            let new: Vec<Note> = notes
                .into_iter()
                .map(|n| Note { key: n.key, start: n.start, length: n.length, velocity: n.velocity.unwrap_or(DEFAULT_VELOCITY).clamp(1, 127), fine: 0.0, bright: 0.5 })
                .collect();
            new.iter().try_for_each(check_note)?;
            let (p, slot) = target(s, pattern, channel)?;
            p.notes_mut(slot).extend(new);
            Ok(())
        },
    },
    Operator {
        id: "notes.clear",
        label: "Clear Notes",
        description: "Remove a channel's steps and notes from a pattern.",
        example: r#"{"channel": 0}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let NoteTool { channel, pattern, .. } = props(v)?;
            let (p, slot) = target(s, pattern, channel)?;
            p.notes.remove(&slot);
            Ok(())
        },
    },
    Operator {
        id: "notes.legato",
        label: "Legato",
        description: "Stretch each note of a channel up to the next one (the last to the pattern's end).",
        example: "{}",
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let NoteTool { channel, pattern, .. } = props(v)?;
            let (p, slot) = target(s, pattern, channel)?;
            let end = p.ticks();
            let notes = p.notes_mut(slot);
            let mut starts: Vec<u32> = notes.iter().map(|n| n.start).collect();
            starts.sort_unstable();
            starts.dedup();
            for n in notes.iter_mut() {
                let next = starts.iter().find(|s| **s > n.start).copied().unwrap_or(end.max(n.start + 1));
                n.length = next - n.start;
            }
            Ok(())
        },
    },
    Operator {
        id: "notes.humanize",
        label: "Humanize",
        description: "Nudge a channel's notes a little off the grid and vary their velocity (amount 0–1).",
        example: r#"{"amount": 0.3}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let NoteTool { channel, pattern, amount, seed, .. } = props(v)?;
            let amount = amount.unwrap_or(0.3).clamp(0.0, 1.0);
            let (p, slot) = target(s, pattern, channel)?;
            let mut x = seed.unwrap_or_else(random_seed) | 1;
            let mut rnd = || {
                x ^= x >> 12;
                x ^= x << 25;
                x ^= x >> 27;
                ((x.wrapping_mul(0x2545F4914F6CDD1D) >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
            };
            for n in p.notes_mut(slot) {
                n.start = (n.start as f32 + rnd() * amount * STEP as f32 / 4.0).max(0.0) as u32;
                n.velocity = (n.velocity as f32 + rnd() * amount * 30.0).clamp(1.0, 127.0) as u8;
            }
            Ok(())
        },
    },
    Operator {
        id: "notes.strum",
        label: "Strum",
        description: "Spread the notes of each chord out in time, low to high, like a strummed guitar.",
        example: r#"{"ticks": 6}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let NoteTool { channel, pattern, ticks, .. } = props(v)?;
            let ticks = ticks.unwrap_or(STEP / 4);
            let (p, slot) = target(s, pattern, channel)?;
            let notes = p.notes_mut(slot);
            let mut order: Vec<usize> = (0..notes.len()).collect();
            order.sort_by_key(|&i| (notes[i].start, notes[i].key));
            let mut place = (u32::MAX, 0);
            for i in order {
                let n = &mut notes[i];
                place = if n.start == place.0 { (place.0, place.1 + 1) } else { (n.start, 0) };
                n.start += place.1 * ticks;
                n.length = n.length.saturating_sub(place.1 * ticks).max(1);
            }
            Ok(())
        },
    },
    // ---- mixer ----
    Operator {
        id: "mixer.add_track",
        label: "Add Mixer Track",
        description: "Add a mixer track (routed to the master); hidden: true makes it a ghost, out of the mixer unless ghosts are shown.",
        example: "{}",
        undo: true,
        poll: |s| s.project.mixer.len() < MAX_TRACKS,
        exec: |s, v| {
            let p: TrackProps = props(v)?;
            let n = s.project.mixer.len();
            s.project.mixer.push(MixerTrack { hidden: p.hidden.unwrap_or(false), ..MixerTrack::new(p.name.unwrap_or_else(|| format!("Track {n}"))) });
            Ok(())
        },
    },
    Operator {
        id: "mixer.remove_track",
        label: "Remove Mixer Track",
        description: "Delete a mixer track; whatever played into it goes to the master.",
        example: r#"{"track": 1}"#,
        undo: true,
        poll: |s| s.project.mixer.len() > 1,
        exec: |s, v| {
            let TrackProps { track: t, .. } = props(v)?;
            if t == 0 || t >= s.project.mixer.len() {
                return Err("the master can't be removed".into());
            }
            let gone: Vec<u32> = s.project.mixer.remove(t).devices.iter().map(|d| d.id).collect();
            let fix = |i: &mut usize| *i = if *i == t { 0 } else if *i > t { *i - 1 } else { *i };
            s.project.channels.iter_mut().for_each(|c| fix(&mut c.track));
            s.project.clips.iter_mut().for_each(|c| fix(&mut c.fx.track));
            for track in &mut s.project.mixer {
                fix(&mut track.route);
                track.sends.retain(|x| x.to != t);
                track.sends.iter_mut().for_each(|x| fix(&mut x.to));
                for d in &mut track.devices {
                    d.sidechain = d.sidechain.filter(|&k| k != t).map(|k| if k > t { k - 1 } else { k });
                }
            }
            let alive = |target: Target| match target {
                Target::TrackGain { track } | Target::TrackPan { track } => track != t,
                Target::Device { id, .. } => !gone.contains(&id),
                Target::Synth { .. } | Target::ChannelGain { .. } | Target::Macro { .. } => true,
            };
            s.project.mappings.retain(|m| alive(m.target));
            s.project.midi_map.retain(|m| alive(m.target));
            // automation clips of the track go with it; the others follow the renumbering
            s.project.clips.retain(|c| !matches!(&c.kind, ClipKind::Automation { target, .. } if !alive(*target)));
            s.project.patterns.iter_mut().for_each(|p| p.automation.retain(|a| alive(a.target)));
            let targets = s
                .project
                .mappings
                .iter_mut()
                .map(|m| &mut m.target)
                .chain(s.project.clips.iter_mut().filter_map(|c| match &mut c.kind {
                    ClipKind::Automation { target, .. } => Some(target),
                    _ => None,
                }))
                .chain(s.project.patterns.iter_mut().flat_map(|p| p.automation.iter_mut().map(|a| &mut a.target)))
                .chain(s.project.midi_map.iter_mut().map(|m| &mut m.target));
            for target in targets {
                if let Target::TrackGain { track } | Target::TrackPan { track } = target {
                    fix(track);
                }
            }
            Ok(())
        },
    },
    Operator {
        id: "mixer.set",
        label: "Edit Mixer Track",
        description: "Change a track's name, gain (0–1.25), pan (-1–1), EQ (low, mid, high: dB, -15–15; low shelf 120 Hz, mid 1 kHz, high shelf 6 kHz), mute, solo, route (the track it feeds), color ([r, g, b]; auto_color: true for the palette's), icon (a name; \"\" for automatic) or hidden (a ghost track, shown only with the mixer's ghosts).",
        example: r#"{"track": 1, "gain": 0.8}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let p: TrackProps = props(v)?;
            let t = track_index(s, p.track)?;
            if let Some(route) = p.route {
                track_index(s, route)?;
                if t == 0 || route == t || s.project.would_loop(t, route) {
                    return Err("that routing would make a loop".into());
                }
            }
            let track = &mut s.project.mixer[t];
            if let Some(name) = p.name {
                track.name = name;
            }
            if let Some(g) = p.gain {
                track.gain = finite(g, "gain")?.clamp(0.0, 1.25);
            }
            if let Some(pan) = p.pan {
                track.pan = finite(pan, "pan")?.clamp(-1.0, 1.0);
            }
            for (band, db) in [p.low, p.mid, p.high].into_iter().enumerate() {
                if let Some(db) = db {
                    track.eq[band] = finite(db, "EQ")?.clamp(-15.0, 15.0);
                }
            }
            if let Some(m) = p.mute {
                track.mute = m;
            }
            if let Some(solo) = p.solo {
                track.solo = solo && t > 0;
            }
            if let Some(route) = p.route {
                track.route = route;
            }
            if p.auto_color {
                track.color = None;
            }
            track.color = p.color.or(track.color);
            if let Some(icon) = p.icon {
                track.icon = icon_name(icon)?;
            }
            track.hidden = p.hidden.unwrap_or(track.hidden);
            Ok(())
        },
    },
    Operator {
        id: "mixer.set_send",
        label: "Set Send",
        description: "Send part of a track to another track (e.g. a reverb bus). Amount 0 removes the send.",
        example: r#"{"track": 1, "to": 3, "amount": 0.3}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let p: TrackProps = props(v)?;
            let (t, to) = (track_index(s, p.track)?, track_index(s, p.to.ok_or("to is required")?)?);
            let amount = finite(p.amount.unwrap_or(0.5), "amount")?.clamp(0.0, 1.0);
            let existing = s.project.mixer[t].sends.iter().position(|x| x.to == to);
            if amount <= 0.0 {
                if let Some(i) = existing {
                    s.project.mixer[t].sends.remove(i);
                }
                return Ok(());
            }
            if t == 0 || to == t || (existing.is_none() && s.project.would_loop(t, to)) {
                return Err("that send would make a loop".into());
            }
            match existing {
                Some(i) => s.project.mixer[t].sends[i].amount = amount,
                None => s.project.mixer[t].sends.push(Send { to, amount }),
            }
            Ok(())
        },
    },
    Operator {
        id: "mixer.duck",
        label: "Duck To (Sidechain)",
        description: "Make a track duck under another one (e.g. the bass under the kick): adds a compressor keyed by the source track.",
        example: r#"{"track": 2, "source": 1}"#,
        undo: true,
        poll: |s| s.project.mixer.len() > 1,
        exec: |s, v| {
            let p: TrackProps = props(v)?;
            let (t, source) = (track_index(s, p.track)?, track_index(s, p.to.or(p.source).ok_or("source is required")?)?);
            if t == source || s.project.would_loop(source, t) {
                return Err("a track can't duck under itself or under a track it feeds".into());
            }
            let id = s.project.next_id;
            s.project.next_id += 1;
            let mut params = DeviceKind::Compressor.defaults();
            params.copy_from_slice(&DUCK);
            s.project.mixer[t].devices.push(Device { id, kind: DeviceKind::Compressor, params, bypass: false, sidechain: Some(source), plugin: None, name: String::new(), color: None, icon: None });
            Ok(())
        },
    },
    // ---- devices ----
    Operator {
        id: "device.add",
        label: "Add Effect",
        description: "Add an effect to a mixer track: eq, filter, compressor, distortion, chorus, delay, reverb or limiter.",
        example: r#"{"track": 0, "kind": "reverb"}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let p: DeviceProps = props(v)?;
            let t = track_index(s, p.track)?;
            let kind = p.kind.ok_or("kind is required")?;
            if (kind == DeviceKind::Plugin) != p.plugin.is_some() {
                return Err("a plugin device needs its plugin (and only it has one)".into());
            }
            let id = s.project.next_id;
            s.project.next_id += 1;
            let devices = &mut s.project.mixer[t].devices;
            let at = p.index.unwrap_or(devices.len()).min(devices.len());
            devices.insert(at, Device { id, kind, params: kind.defaults(), bypass: false, sidechain: None, plugin: p.plugin, name: String::new(), color: None, icon: None });
            Ok(())
        },
    },
    Operator {
        id: "midi.learn",
        label: "Map MIDI Controller",
        description: "Make a controller knob (MIDI CC, channel 0-15) move a parameter. Replaces that controller's and that parameter's old mapping.",
        example: r#"{"channel": 0, "cc": 74, "target": {"type": "track_gain", "track": 1}}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let MidiLearn { channel, cc, target } = props(v)?;
            if channel > 15 || cc > 127 {
                return Err("channel is 0-15 and cc 0-127".into());
            }
            s.project.midi_map.retain(|m| !(m.cc == cc && m.channel == channel) && m.target != target);
            s.project.midi_map.push(CcMap { channel, cc, target });
            Ok(())
        },
    },
    Operator {
        id: "midi.forget",
        label: "Forget MIDI Mapping",
        description: "Stop a controller from moving this parameter.",
        example: r#"{"target": {"type": "track_gain", "track": 1}}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let Forget { target } = props(v)?;
            s.project.midi_map.retain(|m| m.target != target);
            Ok(())
        },
    },
    Operator {
        id: "plugin.store",
        label: "Store Plugin State",
        description: "Keep a plugin's state (base64) in the project: on a channel, or on a plugin effect by id. Not an undo step.",
        example: r#"{"channel": 0, "state": "AAAA"}"#,
        undo: false,
        poll: always,
        exec: |s, v| {
            let StoreState { channel, id, state } = props(v)?;
            let plugin = match (channel, id) {
                (Some(c), _) => match &mut s.project.channels.get_mut(c).ok_or("no such channel")?.instrument {
                    Instrument::Plugin { plugin } => plugin,
                    _ => return Err("that channel isn't a plugin".into()),
                },
                (None, Some(id)) => {
                    let (t, i, _) = s.project.device(id).ok_or("no such effect")?;
                    s.project.mixer[t].devices[i].plugin.as_mut().ok_or("that effect isn't a plugin")?
                }
                (None, None) => return Err("give a channel or an effect id".into()),
            };
            plugin.state = state;
            Ok(())
        },
    },
    Operator {
        id: "device.remove",
        label: "Remove Effect",
        description: "Delete an effect (by id) and its modulation.",
        example: r#"{"id": 1}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let DeviceProps { id, .. } = props(v)?;
            let (t, i, _) = s.project.device(id).ok_or("no such effect")?;
            s.project.mixer[t].devices.remove(i);
            s.project.mappings.retain(|m| !matches!(m.target, Target::Device { id: d, .. } if d == id));
            s.project.midi_map.retain(|m| !matches!(m.target, Target::Device { id: d, .. } if d == id));
            Ok(())
        },
    },
    Operator {
        id: "device.duplicate",
        label: "Duplicate Effect",
        description: "Copy an effect (by id), its settings and a plugin's stored state, right after it in its chain.",
        example: r#"{"id": 1}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let DeviceProps { id, .. } = props(v)?;
            let (t, i, _) = s.project.device(id).ok_or("no such effect")?;
            let mut copy = s.project.mixer[t].devices[i].clone();
            copy.id = s.project.next_id;
            s.project.next_id += 1;
            s.project.mixer[t].devices.insert(i + 1, copy);
            Ok(())
        },
    },
    Operator {
        id: "device.move",
        label: "Move Effect",
        description: "Move an effect to another position in its chain.",
        example: r#"{"id": 1, "index": 0}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let DeviceProps { id, index, .. } = props(v)?;
            let (t, i, _) = s.project.device(id).ok_or("no such effect")?;
            let devices = &mut s.project.mixer[t].devices;
            let d = devices.remove(i);
            devices.insert(index.ok_or("index is required")?.min(devices.len()), d);
            Ok(())
        },
    },
    Operator {
        id: "device.set",
        label: "Set Effect Parameter",
        description: "Set one effect parameter (by effect id and parameter index).",
        example: r#"{"id": 1, "param": 0, "value": 0.5}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let DeviceProps { id, param, value, .. } = props(v)?;
            let (t, i, d) = s.project.device(id).ok_or("no such effect")?;
            let param = param.ok_or("param is required")?;
            let spec = d.kind.params().get(param).ok_or(format!("no parameter {param}"))?;
            let value = spec.clamp(finite(value.ok_or("value is required")?, "value")?);
            s.project.mixer[t].devices[i].params[param] = value;
            Ok(())
        },
    },
    Operator {
        id: "device.sidechain",
        label: "Set Sidechain",
        description: "Let another track drive a compressor (e.g. the kick ducking the bass). Leave out \"source\" to switch it off.",
        example: r#"{"id": 3, "source": 1}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let DeviceProps { id, source, .. } = props(v)?;
            let (t, i, d) = s.project.device(id).ok_or("no such effect")?;
            if d.kind != DeviceKind::Compressor {
                return Err("only compressors take a sidechain".into());
            }
            if let Some(k) = source {
                track_index(s, k)?;
                if k == t || s.project.would_loop(k, t) {
                    return Err("a track can't key itself or a track that feeds it".into());
                }
            }
            s.project.mixer[t].devices[i].sidechain = source;
            Ok(())
        },
    },
    Operator {
        id: "device.style",
        label: "Rename Effect",
        description: "An effect's name (\"\" for its kind's), color ([r, g, b]; auto_color: true for none) and icon (a name; \"\" for its kind's).",
        example: r#"{"id": 1, "name": "Space"}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let p: DeviceStyle = props(v)?;
            let (t, i, _) = s.project.device(p.id).ok_or("no such effect")?;
            let d = &mut s.project.mixer[t].devices[i];
            if let Some(name) = p.name {
                d.name = name.trim().to_string();
            }
            if p.auto_color {
                d.color = None;
            }
            d.color = p.color.or(d.color);
            if let Some(icon) = p.icon {
                d.icon = icon_name(icon)?;
            }
            Ok(())
        },
    },
    Operator {
        id: "device.bypass",
        label: "Bypass Effect",
        description: "Switch an effect off or on without removing it.",
        example: r#"{"id": 1}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let DeviceProps { id, .. } = props(v)?;
            let (t, i, _) = s.project.device(id).ok_or("no such effect")?;
            let d = &mut s.project.mixer[t].devices[i];
            d.bypass = !d.bypass;
            Ok(())
        },
    },
    // ---- modulation ----
    Operator {
        id: "macro.randomize",
        label: "Randomize Macros",
        description: "Turn every macro that moves something to a random position (try again for another).",
        example: r#"{}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let MacroTool { seed, .. } = props(v)?;
            let mut x = seed.unwrap_or_else(random_seed) | 1;
            let used: Vec<usize> = (0..s.project.macros.len()).filter(|i| s.project.mappings.iter().any(|m| m.source == Source::Macro(*i as u8))).collect();
            if used.is_empty() {
                return Err("no macro moves anything yet: map one first (right-click a knob)".into());
            }
            for i in used {
                x ^= x >> 12;
                x ^= x << 25;
                x ^= x >> 27;
                s.project.macros[i].value = ((x.wrapping_mul(0x2545F4914F6CDD1D) >> 40) as f32 / (1u64 << 24) as f32 * 100.0).round() / 100.0;
            }
            Ok(())
        },
    },
    Operator {
        id: "macro.store",
        label: "Store Macro Variation",
        description: "Keep where the macros are now as a variation, to come back to with macro.recall.",
        example: r#"{}"#,
        undo: true,
        poll: always,
        exec: |s, _| {
            if s.project.variations.len() >= VARIATIONS {
                return Err("8 variations at most: forget one first".into());
            }
            let values = s.project.macros.iter().map(|m| m.value).collect();
            s.project.variations.push(values);
            Ok(())
        },
    },
    Operator {
        id: "macro.recall",
        label: "Recall Macro Variation",
        description: "Put the macros back where a stored variation had them.",
        example: r#"{"index": 0}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let MacroTool { index, .. } = props(v)?;
            let values = s.project.variations.get(index.unwrap_or(0)).ok_or("no such variation")?.clone();
            for (m, value) in s.project.macros.iter_mut().zip(values) {
                m.value = value;
            }
            Ok(())
        },
    },
    Operator {
        id: "macro.forget",
        label: "Forget Macro Variation",
        description: "Delete a stored variation.",
        example: r#"{"index": 0}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let MacroTool { index, .. } = props(v)?;
            let i = index.unwrap_or(0);
            if i >= s.project.variations.len() {
                return Err("no such variation".into());
            }
            s.project.variations.remove(i);
            Ok(())
        },
    },
    Operator {
        id: "macro.set",
        label: "Set Macro",
        description: "Turn a macro knob (0–1) or rename it.",
        example: r#"{"index": 0, "value": 0.5}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let p: ModProps = props(v)?;
            let m = s.project.macros.get_mut(p.index).ok_or("no such macro")?;
            if let Some(value) = p.value {
                m.value = finite(value, "value")?.clamp(0.0, 1.0);
            }
            if let Some(name) = p.name {
                m.name = name;
            }
            Ok(())
        },
    },
    Operator {
        id: "lfo.add",
        label: "Add LFO",
        description: "Add a low-frequency oscillator you can map onto any knob.",
        example: "{}",
        undo: true,
        poll: |s| s.project.lfos.len() < MAX_LFOS,
        exec: |s, _| {
            s.project.lfos.push(Lfo::default());
            Ok(())
        },
    },
    Operator {
        id: "lfo.set",
        label: "Edit LFO",
        description: "Change an LFO's shape (0 sine, 1 triangle, 2 saw, 3 square, 4 random), rate in Hz, tempo sync and length in beats.",
        example: r#"{"index": 0, "sync": true, "beats": 2}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let p: ModProps = props(v)?;
            let l = s.project.lfos.get_mut(p.index).ok_or("no such LFO")?;
            l.shape = p.shape.map_or(l.shape, |x| x.min(4));
            l.hz = p.hz.map_or(Ok(l.hz), |x| finite(x, "hz").map(|x| x.clamp(0.01, 30.0)))?;
            l.sync = p.sync.unwrap_or(l.sync);
            l.beats = p.beats.map_or(Ok(l.beats), |x| finite(x, "beats").map(|x| x.clamp(0.0625, 64.0)))?;
            Ok(())
        },
    },
    Operator {
        id: "lfo.remove",
        label: "Remove LFO",
        description: "Delete an LFO and what it was mapped to.",
        example: r#"{"index": 0}"#,
        undo: true,
        poll: |s| !s.project.lfos.is_empty(),
        exec: |s, v| {
            let ModProps { index, .. } = props(v)?;
            if index >= s.project.lfos.len() {
                return Err("no such LFO".into());
            }
            s.project.lfos.remove(index);
            let i = index as u8;
            s.project.mappings.retain(|m| m.source != Source::Lfo(i));
            for m in &mut s.project.mappings {
                if let Source::Lfo(k) = &mut m.source
                    && *k > i
                {
                    *k -= 1;
                }
            }
            Ok(())
        },
    },
    Operator {
        id: "mod.map",
        label: "Map Modulation",
        description: "Let a macro or LFO move a knob. Amount -1–1 is a share of the knob's range.",
        example: r#"{"source": {"type": "macro", "index": 0}, "target": {"type": "track_gain", "track": 1}, "amount": 0.5}"#,
        undo: true,
        poll: always,
        exec: |s, v| {
            let p: ModProps = props(v)?;
            let (source, target) = (p.source.ok_or("source is required")?, p.target.ok_or("target is required")?);
            let exists = match source {
                Source::Macro(i) => (i as usize) < s.project.macros.len(),
                Source::Lfo(i) => (i as usize) < s.project.lfos.len(),
            };
            if !exists {
                return Err("no such macro or LFO".into());
            }
            check_target(&s.project, target)?;
            let amount = finite(p.amount.unwrap_or(0.5), "amount")?.clamp(-1.0, 1.0);
            match s.project.mappings.iter_mut().find(|m| m.source == source && m.target == target) {
                Some(m) => m.amount = amount,
                None => s.project.mappings.push(Mapping { source, target, amount }),
            }
            Ok(())
        },
    },
    Operator {
        id: "mod.set",
        label: "Set Modulation Amount",
        description: "Change how far a mapping moves its knob (-1–1).",
        example: r#"{"index": 0, "amount": 0.25}"#,
        undo: true,
        poll: |s| !s.project.mappings.is_empty(),
        exec: |s, v| {
            let p: ModProps = props(v)?;
            let m = s.project.mappings.get_mut(p.index).ok_or("no such mapping")?;
            m.amount = finite(p.amount.ok_or("amount is required")?, "amount")?.clamp(-1.0, 1.0);
            Ok(())
        },
    },
    Operator {
        id: "mod.remove",
        label: "Remove Modulation",
        description: "Delete a mapping.",
        example: r#"{"index": 0}"#,
        undo: true,
        poll: |s| !s.project.mappings.is_empty(),
        exec: |s, v| {
            let ModProps { index, .. } = props(v)?;
            if index >= s.project.mappings.len() {
                return Err("no such mapping".into());
            }
            s.project.mappings.remove(index);
            Ok(())
        },
    },
    // ---- channels ----
    Operator {
        id: "channel.add",
        label: "Add Channel",
        description: "Add a channel: a sampler for a sample path, or a VA-1 synth (optionally from a preset, e.g. \"Acid Bass\").",
        example: "{}",
        undo: true,
        poll: |s| s.project.channels.len() < MAX_CHANNELS,
        exec: |s, v| {
            let AddChannel { sample, name, preset, plugin } = props(v)?;
            let slot = s.project.free_slot().ok_or("all 64 channels are in use")?;
            let (default, instrument) = instrument(sample, preset, plugin)?;
            s.project.channels.push(Channel::new(slot, name.unwrap_or(default), instrument));
            s.selected = s.project.channels.len() - 1;
            Ok(())
        },
    },
    Operator {
        id: "channel.replace",
        label: "Replace Instrument",
        description: "Swap a channel's instrument (a sample, a VA-1 preset or a plugin) and keep its notes, mixer track and level.",
        example: r#"{"channel": 0, "preset": "Acid Bass"}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let ReplaceChannel { channel, sample, preset, plugin } = props(v)?;
            if sample.is_none() && preset.is_none() && plugin.is_none() {
                return Err("give a sample, a preset or a plugin".into());
            }
            let i = s.channel_index(channel)?;
            let (name, instrument) = instrument(sample, preset, plugin)?;
            let c = &mut s.project.channels[i];
            (c.name, c.instrument) = (name, instrument);
            s.selected = i;
            Ok(())
        },
    },
    Operator {
        id: "channel.remove",
        label: "Remove Channel",
        description: "Delete a channel with its steps and notes in every pattern (the selected one by default).",
        example: "{}",
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let ChannelRef { channel } = props(v)?;
            let i = s.channel_index(channel)?;
            let slot = s.project.channels.remove(i).slot;
            s.project.patterns.iter_mut().for_each(|p| p.forget(slot));
            Ok(())
        },
    },
    Operator {
        id: "channel.select",
        label: "Select Channel",
        description: "Choose which channel the keyboard plays and the device and piano roll show.",
        example: r#"{"channel": 1}"#,
        undo: false,
        poll: has_channels,
        exec: |s, v| {
            let ChannelRef { channel } = props(v)?;
            s.selected = s.channel_index(Some(channel.ok_or("channel is required")?))?;
            Ok(())
        },
    },
    Operator {
        id: "channel.set",
        label: "Edit Channel",
        description: "Change a channel's name, gain (0–1), mute, pitch (semitones), choke group, sample, attack and release (seconds, samplers), start and end (the part of the sound a sampler plays, 0–1 of its length), looped (true: that part repeats while the note is held), or swing (0–1; negative follows the song's).",
        example: r#"{"channel": 0, "gain": 0.7}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let p: SetChannel = props(v)?;
            if let Some(t) = p.track {
                track_index(s, t)?;
            }
            let c = s.channel_mut(p.channel)?;
            if let Some(t) = p.track {
                c.track = t;
            }
            let named = p.name.is_some();
            if let Some(name) = p.name {
                c.name = name;
            }
            if let Some(g) = p.gain {
                c.gain = finite(g, "gain")?.clamp(0.0, 1.0);
            }
            if let Some(m) = p.mute {
                c.mute = m;
            }
            if let Some(pitch) = p.pitch {
                c.pitch = (finite(pitch, "pitch")?.clamp(-48.0, 48.0) * 100.0).round() / 100.0;
            }
            if let Some(g) = p.choke {
                c.choke = g.min(8);
            }
            if let Some(a) = p.attack {
                c.attack = finite(a, "attack")?.clamp(0.0, 1.0);
            }
            if let Some(r) = p.release {
                c.release = finite(r, "release")?.clamp(0.0, 2.0);
            }
            if let Some(a) = p.start {
                c.start = finite(a, "start")?.clamp(0.0, 1.0);
            }
            if let Some(e) = p.end {
                c.end = finite(e, "end")?.clamp(0.0, 1.0);
            }
            if c.end - c.start < 0.005 {
                return Err("start must come before end".into());
            }
            if let Some(l) = p.looped {
                c.looped = l;
            }
            if let Some(sw) = p.swing {
                let sw = finite(sw, "swing")?;
                c.swing = (sw >= 0.0).then_some(sw.min(1.0));
            }
            if let Some(path) = p.sample {
                if let Instrument::Synth { .. } = c.instrument {
                    return Err("only sampler channels take a sample".into());
                }
                // the channel takes the new sound's name, wherever the sound came from
                if !named {
                    c.name = sample_name(&path).to_string();
                }
                // a new sound plays whole, unless this call cuts it too
                (c.start, c.end) = (p.start.map_or(0.0, |_| c.start), p.end.map_or(1.0, |_| c.end));
                c.instrument = Instrument::from_file(path);
            }
            Ok(())
        },
    },
    // ---- synth ----
    Operator {
        id: "synth.set_param",
        label: "Set Synth Parameter",
        description: "Set one VA-1 parameter (on the selected channel unless `channel` is given).",
        example: r#"{"param": "cutoff", "value": 1200}"#,
        undo: true,
        poll: has_channels,
        exec: |s, v| {
            let SetParam { channel, param, value } = props(v)?;
            let value = param.info().clamp(finite(value, "value")?);
            s.synth_mut(channel)?[param as usize] = value;
            Ok(())
        },
    },
    Operator {
        id: "synth.reset",
        label: "Init Synth",
        description: "Put every VA-1 parameter back to its default.",
        example: "{}",
        undo: true,
        poll: selected_is_synth,
        exec: |s, v| {
            let ChannelRef { channel } = props(v)?;
            *s.synth_mut(channel)? = default_synth();
            Ok(())
        },
    },
    Operator {
        id: "synth.surprise",
        label: "Randomize Synth",
        description: "A random but musical VA-1 patch.",
        example: "{}",
        undo: true,
        poll: selected_is_synth,
        exec: |s, v| {
            let Surprise { channel, seed } = props(v)?;
            surprise(s.synth_mut(channel)?, seed.unwrap_or_else(random_seed));
            Ok(())
        },
    },
    // ---- edit ----
    Operator {
        id: "edit.undo",
        label: "Undo",
        description: "Take back the last change.",
        example: "{}",
        undo: false,
        poll: |s| !s.undo.is_empty(),
        exec: |s, _| {
            s.step(true);
            Ok(())
        },
    },
    Operator {
        id: "edit.redo",
        label: "Redo",
        description: "Bring back the change you just undid.",
        example: "{}",
        undo: false,
        poll: |s| !s.redo.is_empty(),
        exec: |s, _| {
            s.step(false);
            Ok(())
        },
    },
];

fn random_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new().build_hasher().finish()
}

/// Random but musically sensible patch (xorshift64*, no extra crates).
fn surprise(synth: &mut [f32; PARAM_COUNT], seed: u64) {
    use Param::*;
    let mut x = seed | 1;
    let mut rnd = || {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        (x.wrapping_mul(0x2545F4914F6CDD1D) >> 40) as f32 / (1u64 << 24) as f32
    };
    let mut set = |p: Param, n: f32| synth[p as usize] = p.info().from_norm(n);
    set(Osc1Wave, rnd());
    set(Osc2Wave, rnd());
    set(Osc2Detune, rnd() * 0.6);
    set(OscMix, 0.2 + rnd() * 0.6);
    set(Cutoff, 0.35 + rnd() * 0.5);
    set(Resonance, rnd() * 0.7);
    set(EnvAmount, rnd() * 0.8);
    set(Attack, rnd() * rnd() * 0.6);
    set(Decay, 0.3 + rnd() * 0.5);
    set(Sustain, rnd());
    set(Release, 0.25 + rnd() * 0.45);
    const INTERVALS: [f32; 6] = [0.0, 0.0, 7.0, 12.0, -12.0, 5.0];
    synth[Osc2Pitch as usize] = INTERVALS[(rnd() * 6.0) as usize % 6];
}
