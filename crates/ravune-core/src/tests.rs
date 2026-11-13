use serde_json::{Value, json};

use crate::*;

const SYNTH: usize = 5; // index of the VA-1 in the default template

fn session() -> Session {
    Session { selected: SYNTH, ..Default::default() }
}

fn cutoff(s: &Session) -> f32 {
    s.project.channels[SYNTH].synth().unwrap()[Param::Cutoff as usize]
}

fn set_cutoff(s: &mut Session, value: f32) {
    s.run("synth.set_param", json!({"param": "cutoff", "value": value})).unwrap();
}

fn ok(s: &mut Session, id: &str, props: Value) {
    s.run(id, props).unwrap_or_else(|e| panic!("{id}: {e}"));
}

#[test]
fn undo_redo_round_trip() {
    let mut s = session();
    let original = s.project.clone();
    set_cutoff(&mut s, 1000.0);
    assert_eq!(cutoff(&s), 1000.0);
    assert_eq!(s.undo_label(), Some("Set Synth Parameter"));
    ok(&mut s, "edit.undo", json!({}));
    assert_eq!(s.project, original);
    ok(&mut s, "edit.redo", json!({}));
    assert_eq!(cutoff(&s), 1000.0);
    assert!(s.run("edit.redo", json!({})).is_err(), "nothing left to redo");
}

#[test]
fn drags_merge_into_one_step() {
    let mut s = session();
    for v in [500.0, 600.0, 700.0] {
        s.run_merge("synth.set_param", json!({"param": "cutoff", "value": v}), "cutoff").unwrap();
    }
    s.close_merge();
    s.run_merge("synth.set_param", json!({"param": "cutoff", "value": 800.0}), "cutoff").unwrap();
    ok(&mut s, "edit.undo", json!({}));
    assert_eq!(cutoff(&s), 700.0);
    ok(&mut s, "edit.undo", json!({}));
    assert_eq!(s.project, Project::default());
}

#[test]
fn bad_input_changes_nothing() {
    let mut s = session();
    assert!(s.run("synth.set_param", json!({"param": "nope", "value": 1.0})).is_err());
    assert!(s.run("step.toggle", json!({"channel": 99, "step": 0})).is_err());
    assert!(s.run("step.toggle", json!({"channel": 0, "step": MAX_STEPS})).is_err());
    assert!(s.run("note.add", json!({"key": 60, "start": 0, "length": 0})).is_err());
    assert!(s.run("note.add", json!({"key": 200, "start": 0, "length": 24})).is_err());
    assert!(s.run("clip.add", json!({"lane": 0, "start": 0, "sample": "x.wav"})).is_err(), "audio needs a length");
    assert!(s.run("pattern.remove", json!({})).is_err(), "the last pattern stays");
    assert!(s.run("does.not_exist", json!({})).is_err());
    s.selected = 0; // a sampler
    assert!(s.run("synth.set_param", json!({"param": "cutoff", "value": 500.0})).is_err());
    assert_eq!(s.project, Project::default());
    assert_eq!(s.undo_label(), None);
}

#[test]
fn values_are_clamped_and_noops_skip_history() {
    let mut s = session();
    set_cutoff(&mut s, 1e9);
    assert_eq!(cutoff(&s), Param::Cutoff.info().max);
    ok(&mut s, "edit.undo", json!({}));
    set_cutoff(&mut s, Param::Cutoff.info().default);
    assert_eq!(s.undo_label(), None, "setting the same value is not an undo step");
}

#[test]
fn transport_and_selection_are_not_undoable() {
    let mut s = session();
    ok(&mut s, "transport.play_toggle", json!({}));
    ok(&mut s, "transport.mode_toggle", json!({}));
    ok(&mut s, "channel.select", json!({"channel": 2}));
    ok(&mut s, "transport.set_position", json!({"tick": 384}));
    assert!(s.playing && s.song_mode);
    assert_eq!((s.selected, s.position), (2, 384));
    ok(&mut s, "transport.jump", json!({"tick": 96}));
    assert_eq!((s.position, s.jumps), (96, 1), "a jump moves the position and asks the engine to follow");
    assert!(s.run("edit.undo", json!({})).is_err());
}

#[test]
fn steps_live_in_the_current_pattern() {
    let mut s = session();
    ok(&mut s, "step.toggle", json!({"channel": 0, "step": 4}));
    assert_eq!(s.project.patterns[0].step(0, 4), DEFAULT_VELOCITY);
    ok(&mut s, "pattern.add", json!({}));
    assert_eq!(s.pattern, 1);
    ok(&mut s, "step.set", json!({"channel": 0, "step": 4, "velocity": 60}));
    assert_eq!(s.project.patterns[1].step(0, 4), 60);
    assert_eq!(s.project.patterns[0].step(0, 4), DEFAULT_VELOCITY, "other pattern untouched");
    ok(&mut s, "pattern.set_length", json!({"steps": 20}));
    assert_eq!(s.project.patterns[1].length, 20, "any length");
    ok(&mut s, "pattern.clear", json!({}));
    assert_eq!(s.project.patterns[1].step(0, 4), 0);
}

#[test]
fn steps_are_notes_and_notes_show_as_steps() {
    let mut s = session();
    ok(&mut s, "step.set", json!({"channel": 0, "step": 4, "velocity": 90}));
    let slot = s.project.channels[0].slot;
    assert_eq!(s.current_pattern().notes(slot), [Note { start: 4 * STEP, length: STEP, key: STEP_KEY, velocity: 90, ..Default::default() }], "a step is a note");
    ok(&mut s, "note.add", json!({"channel": 0, "key": STEP_KEY, "start": 8 * STEP, "length": STEP}));
    assert_eq!(s.current_pattern().step(slot, 8), DEFAULT_VELOCITY, "a one-step note on the step key is a step");
    assert!(s.current_pattern().is_steps(slot));
    ok(&mut s, "note.add", json!({"channel": 0, "key": 64, "start": 0, "length": 48}));
    assert!(!s.current_pattern().is_steps(slot), "a melody needs the piano roll");
    ok(&mut s, "step.set", json!({"channel": 0, "step": 4, "velocity": 0}));
    assert_eq!(s.current_pattern().notes(slot).len(), 2, "turning a step off deletes only that note");
    assert!(s.run("step.set", json!({"channel": 0, "step": MAX_STEPS, "velocity": 1})).is_err());
}

#[test]
fn replacing_an_instrument_keeps_the_notes() {
    let mut s = session();
    ok(&mut s, "step.set", json!({"channel": 0, "step": 4, "velocity": 90}));
    let slot = s.project.channels[0].slot;
    ok(&mut s, "channel.replace", json!({"channel": 0, "preset": "Acid Bass"}));
    assert_eq!(s.project.channels[0].name, "Acid Bass");
    assert!(matches!(s.project.channels[0].instrument, Instrument::Synth { .. }));
    assert_eq!((s.project.channels[0].slot, s.current_pattern().step(slot, 4)), (slot, 90), "same slot, same notes");
    assert!(s.run("channel.replace", json!({"channel": 0})).is_err(), "needs something to put in");
    ok(&mut s, "channel.set", json!({"channel": 1, "sample": "D:/Drums/Fat Snare.wav"}));
    assert_eq!(s.project.channels[1].name, "Fat Snare", "a new sound brings its name");
}

#[test]
fn lanes_mute_solo_insert_and_remove() {
    let mut s = session();
    for lane in 0..3 {
        ok(&mut s, "clip.add", json!({"lane": lane, "start": 0}));
    }
    ok(&mut s, "lane.set", json!({"lane": 1, "name": "Bass", "mute": true}));
    assert!(s.project.lane_plays(0) && !s.project.lane_plays(1));
    ok(&mut s, "lane.set", json!({"lane": 2, "solo": true}));
    assert!(!s.project.lane_plays(0) && s.project.lane_plays(2), "a solo silences the rest");
    ok(&mut s, "lane.insert", json!({"lane": 1}));
    assert_eq!(s.project.clips.iter().map(|c| c.lane).collect::<Vec<_>>(), [0, 2, 3]);
    assert_eq!(s.project.lane(2).name, "Bass", "settings move with their lane");
    ok(&mut s, "lane.remove", json!({"lane": 2}));
    assert_eq!(s.project.clips.iter().map(|c| c.lane).collect::<Vec<_>>(), [0, 2]);
    ok(&mut s, "lane.set", json!({"lane": 2, "solo": false}));
    assert!(s.project.lanes.is_empty(), "plain lanes aren't kept");
}

#[test]
fn make_unique_gives_a_clip_its_own_pattern() {
    let mut s = session();
    ok(&mut s, "clip.add", json!({"lane": 0, "start": 0}));
    ok(&mut s, "clip.add", json!({"lane": 0, "start": 384}));
    ok(&mut s, "clip.make_unique", json!({"index": 1}));
    assert_eq!(s.project.patterns.len(), 2);
    assert_eq!(s.project.clips[1].kind, ClipKind::Pattern { pattern: 1 });
    assert_eq!(s.project.clips[0].kind, ClipKind::Pattern { pattern: 0 }, "the other clip keeps the original");
}

#[test]
fn rack_tools_fill_rotate_solo_clone_move() {
    let mut s = session();
    ok(&mut s, "steps.fill", json!({"channel": 0, "every": 4}));
    let slot = s.project.channels[0].slot;
    assert_eq!((0..16).filter(|i| s.current_pattern().step(slot, *i) > 0).collect::<Vec<_>>(), [0, 4, 8, 12]);
    ok(&mut s, "steps.rotate", json!({"channel": 0, "by": -1}));
    assert_eq!((0..16).filter(|i| s.current_pattern().step(slot, *i) > 0).collect::<Vec<_>>(), [3, 7, 11, 15], "wraps around");
    ok(&mut s, "channel.solo", json!({"channel": 0}));
    assert!(s.project.channels.iter().enumerate().all(|(i, c)| c.mute == (i != 0)));
    ok(&mut s, "channel.solo", json!({"channel": 0}));
    assert!(s.project.channels.iter().all(|c| !c.mute), "again: everyone back");
    ok(&mut s, "channel.clone", json!({"channel": 0}));
    let copy = &s.project.channels[1];
    assert_eq!((copy.name.as_str(), s.current_pattern().notes(copy.slot).len()), ("Kick 2", 4), "notes come along");
    ok(&mut s, "channel.move", json!({"channel": 1, "by": -1}));
    assert_eq!(s.project.channels[0].name, "Kick 2");
}

#[test]
fn piano_roll_tools_chord_legato_strum_humanize() {
    let mut s = session();
    ok(&mut s, "notes.chord", json!({"key": 60, "start": 0, "length": 24, "chord": "minor"}));
    ok(&mut s, "notes.chord", json!({"key": 65, "start": 96, "length": 24, "chord": "Major"}));
    let slot = s.selected_channel().unwrap().slot;
    let keys = |s: &Session| s.current_pattern().notes(slot).iter().map(|n| n.key).collect::<Vec<_>>();
    assert_eq!(keys(&s), [60, 63, 67, 65, 69, 72]);
    ok(&mut s, "notes.legato", json!({}));
    assert!(s.current_pattern().notes(slot)[..3].iter().all(|n| n.length == 96), "up to the next chord");
    ok(&mut s, "notes.strum", json!({"ticks": 6}));
    let starts: Vec<u32> = s.current_pattern().notes(slot)[..3].iter().map(|n| n.start).collect();
    assert_eq!(starts, [0, 6, 12], "low to high");
    let before = s.current_pattern().notes(slot).to_vec();
    ok(&mut s, "notes.humanize", json!({"amount": 1.0, "seed": 7}));
    let after = s.current_pattern().notes(slot);
    assert!(after.iter().zip(&before).any(|(a, b)| a.velocity != b.velocity), "velocities vary");
    assert!(after.iter().zip(&before).all(|(a, b)| a.start.abs_diff(b.start) <= STEP / 4), "only a little off the grid");
    assert!(s.run("notes.chord", json!({"key": 60, "start": 0, "length": 24, "chord": "Mystery"})).is_err());
}

#[test]
fn old_files_turn_steps_into_notes() {
    let old = r#"{"name": "Beat", "length": 16, "steps": {"0": [100, 0, 0, 0, 80]}, "notes": {}}"#;
    let p: Pattern = serde_json::from_str(old).unwrap();
    assert_eq!((p.step(0, 0), p.step(0, 4), p.notes(0).len()), (100, 80, 2));
    let saved = serde_json::to_string(&p).unwrap();
    assert!(!saved.contains("steps"), "saved as notes only: {saved}");
}

#[test]
fn pattern_clips_follow_the_pattern_length_and_patterns_get_colors() {
    let mut s = session();
    ok(&mut s, "clip.add", json!({"lane": 0, "start": 0}));
    ok(&mut s, "clip.add", json!({"lane": 1, "start": 0, "length": 96}));
    ok(&mut s, "pattern.set_length", json!({"steps": 32}));
    assert_eq!(s.project.clips[0].length, 32 * STEP, "a clip one pattern long follows it");
    assert_eq!(s.project.clips[1].length, 96, "a resized clip keeps its length");
    ok(&mut s, "pattern.add", json!({}));
    assert_eq!(s.current_pattern().color, Some(PATTERN_COLORS[1]));
    ok(&mut s, "pattern.set_color", json!({"color": [1, 2, 3]}));
    assert_eq!(s.current_pattern().color, Some([1, 2, 3]));
    ok(&mut s, "pattern.set_color", json!({"color": null}));
    assert_eq!(s.current_pattern().color, None);
}

#[test]
fn notes_add_edit_quantize_remove() {
    let mut s = session();
    ok(&mut s, "note.add", json!({"key": 60, "start": 5, "length": 24}));
    ok(&mut s, "note.add", json!({"key": 64, "start": 50, "length": 12, "velocity": 80}));
    ok(&mut s, "note.set", json!({"index": 0, "key": 62, "length": 48}));
    let notes = s.current_pattern().notes(5);
    assert_eq!((notes[0].key, notes[0].length, notes[1].velocity), (62, 48, 80));
    ok(&mut s, "notes.quantize", json!({}));
    let starts: Vec<u32> = s.current_pattern().notes(5).iter().map(|n| n.start).collect();
    assert_eq!(starts, [0, 48]);
    ok(&mut s, "note.remove", json!({"index": 0}));
    assert_eq!(s.current_pattern().notes(5).len(), 1);
    assert!(s.run("note.set", json!({"index": 9, "key": 1})).is_err());
}

#[test]
fn clips_add_move_split_duplicate() {
    let mut s = session();
    ok(&mut s, "clip.add", json!({"lane": 0, "start": 0}));
    assert_eq!(s.project.clips[0].length, 16 * STEP, "pattern clips default to the pattern length");
    ok(&mut s, "clip.set", json!({"index": 0, "length": 4 * BAR, "lane": 2}));
    ok(&mut s, "clip.split", json!({"index": 0, "at": BAR}));
    let (a, b) = (&s.project.clips[0], &s.project.clips[1]);
    assert_eq!((a.start, a.length, b.start, b.length, b.offset), (0, BAR, BAR, 3 * BAR, BAR));
    ok(&mut s, "clip.duplicate", json!({"index": 1}));
    assert_eq!(s.project.clips[2].start, 4 * BAR);
    assert_eq!(s.project.song_end(), 7 * BAR);
    ok(&mut s, "clip.add", json!({"lane": 1, "start": 0, "sample": "builtin:sub.wav", "length": 200}));
    assert!(matches!(s.project.clips[3].kind, ClipKind::Audio { .. }));
    assert!(s.run("clip.split", json!({"index": 0, "at": 0})).is_err());
}

#[test]
fn removing_a_pattern_fixes_the_playlist() {
    let mut s = session();
    ok(&mut s, "pattern.add", json!({})); // 1
    ok(&mut s, "pattern.add", json!({})); // 2
    for p in 0..3 {
        ok(&mut s, "clip.add", json!({"lane": p, "start": 0, "pattern": p}));
    }
    ok(&mut s, "pattern.remove", json!({"pattern": 1}));
    let refs: Vec<usize> = s
        .project
        .clips
        .iter()
        .map(|c| match c.kind {
            ClipKind::Pattern { pattern } => pattern,
            _ => usize::MAX,
        })
        .collect();
    assert_eq!(refs, [0, 1], "pattern 1's clip removed, pattern 2 renumbered");
    assert_eq!(s.pattern, 1, "selection clamped");
}

#[test]
fn a_sampler_plays_the_part_of_its_sound_it_is_cut_to() {
    let mut s = session();
    ok(&mut s, "channel.add", json!({"sample": "kick.wav"}));
    ok(&mut s, "channel.set", json!({"start": 0.25, "end": 0.75}));
    let c = s.project.channels.last().unwrap();
    assert_eq!((c.start, c.end), (0.25, 0.75));
    assert!(s.run("channel.set", json!({"start": 0.8})).is_err(), "not past the end");
    ok(&mut s, "channel.set", json!({"sample": "snare.wav"}));
    let c = s.project.channels.last().unwrap();
    assert_eq!((c.start, c.end), (0.0, 1.0), "a new sound plays whole");
    let text = ron::to_string(c).unwrap();
    assert!(!text.contains("start") && !text.contains("end:"), "a whole sound writes nothing: {text}");
}

#[test]
fn channels_add_remove_and_forget_their_data() {
    let mut s = session();
    ok(&mut s, "channel.add", json!({"sample": "C:\\Samples\\Big Kick.wav"}));
    let added = s.project.channels.len() - 1;
    assert_eq!(s.project.channels[added].name, "Big Kick");
    assert_eq!(s.selected, added, "new channel gets selected");
    let slot = s.project.channels[added].slot;
    ok(&mut s, "step.toggle", json!({"channel": added, "step": 0}));
    ok(&mut s, "note.add", json!({"key": 60, "start": 0, "length": 24}));
    ok(&mut s, "channel.remove", json!({}));
    assert_eq!(s.current_pattern().step(slot, 0), 0);
    assert!(s.current_pattern().notes(slot).is_empty(), "a reused slot starts clean");
    while !s.project.channels.is_empty() {
        ok(&mut s, "channel.remove", json!({}));
    }
    assert!(s.run("channel.remove", json!({})).is_err());
}

#[test]
fn channel_set_validates() {
    let mut s = session();
    ok(&mut s, "channel.set", json!({"channel": 0, "gain": 2.0, "pitch": -3.4, "choke": 2, "mute": true}));
    let c = &s.project.channels[0];
    assert_eq!((c.gain, c.pitch, c.choke, c.mute), (1.0, -3.4, 2, true), "pitch keeps its cents");
    assert!(s.run("channel.set", json!({"channel": SYNTH, "sample": "x.wav"})).is_err());
}

#[test]
fn scales_highlight_and_fold() {
    use Scale::*;
    assert!(Major.contains(0, 64) && !Major.contains(0, 61)); // E in C major, C# not
    assert!(Minor.contains(9, 72)); // C in A minor
    assert_eq!(Major.fold(0, 61), 60, "C# folds down to C");
    assert_eq!(MinorPentatonic.fold(9, 71), 72, "B → C in A minor pentatonic");
    assert_eq!(Chromatic.fold(0, 61), 61);
    assert_eq!(note_name(60), "C4");
    let mut s = session();
    ok(&mut s, "song.set_key", json!({"root": 21, "scale": "minor"}));
    assert_eq!((s.project.root, s.project.scale), (9, Minor));
}

#[test]
fn surprise_is_deterministic_and_in_range() {
    let (mut a, mut b) = (session(), session());
    ok(&mut a, "synth.surprise", json!({"seed": 42}));
    ok(&mut b, "synth.surprise", json!({"seed": 42}));
    assert_eq!(a.project, b.project);
    for p in Param::ALL {
        let v = a.project.channels[SYNTH].synth().unwrap()[p as usize];
        assert_eq!(v, p.info().clamp(v), "{p:?} out of range");
    }
}

#[test]
fn project_round_trips_through_json() {
    let mut s = session();
    ok(&mut s, "step.toggle", json!({"channel": 0, "step": 3}));
    ok(&mut s, "note.add", json!({"key": 60, "start": 0, "length": 24}));
    ok(&mut s, "clip.add", json!({"lane": 0, "start": 0}));
    let text = serde_json::to_string(&s.project).unwrap();
    assert_eq!(serde_json::from_str::<Project>(&text).unwrap(), s.project);
}

#[test]
fn registry_is_consistent() {
    for (i, op) in OPERATORS.iter().enumerate() {
        assert!(OPERATORS[..i].iter().all(|o| o.id != op.id), "duplicate id {}", op.id);
        assert!(serde_json::from_str::<Value>(op.example).is_ok(), "{} example", op.id);
    }
    for (i, p) in Param::ALL.iter().enumerate() {
        assert_eq!(*p as usize, i);
    }
    assert_eq!(sample_name("builtin:kick.wav"), "kick");
    assert_eq!(sample_name("/a/b/Snare 01.flac"), "Snare 01");
}

#[test]
fn norm_round_trips() {
    for info in &PARAMS {
        let v = info.from_norm(0.37);
        assert!((info.from_norm(info.to_norm(v)) - v).abs() <= v.abs() * 1e-4 + 1e-4);
    }
}

#[test]
fn mixer_routing_rejects_loops_and_survives_removal() {
    let mut s = session();
    ok(&mut s, "mixer.add_track", json!({"name": "Reverb"})); // 3
    ok(&mut s, "mixer.set", json!({"track": 1, "route": 2})); // drums → synth
    assert!(s.run("mixer.set", json!({"track": 2, "route": 1})).is_err(), "synth → drums would loop");
    ok(&mut s, "mixer.set_send", json!({"track": 2, "to": 3, "amount": 0.3}));
    assert!(s.run("mixer.set_send", json!({"track": 3, "to": 1})).is_err(), "reverb → drums → synth → reverb");
    assert!(s.run("mixer.set", json!({"track": 0, "route": 1})).is_err(), "the master feeds nothing");
    ok(&mut s, "mixer.set", json!({"track": 1, "gain": 9.0, "pan": -2.0}));
    assert_eq!((s.project.mixer[1].gain, s.project.mixer[1].pan), (1.25, -1.0));
    ok(&mut s, "mixer.set", json!({"track": 1, "low": -40.0, "high": 3.5}));
    assert_eq!(s.project.mixer[1].eq, [-15.0, 0.0, 3.5], "bands left out stay; the rest clamp to ±15 dB");
    ok(&mut s, "mixer.remove_track", json!({"track": 2}));
    assert_eq!(s.project.mixer[1].route, 0, "drums fall back to the master");
    assert!(s.project.mixer.iter().all(|t| t.sends.iter().all(|x| x.to < s.project.mixer.len())));
    assert_eq!(s.project.channels[SYNTH].track, 0, "the synth channel lost its track");
    assert!(s.run("mixer.remove_track", json!({"track": 0})).is_err());
}

#[test]
fn devices_and_modulation() {
    let mut s = session();
    ok(&mut s, "device.add", json!({"track": 2, "kind": "delay"}));
    ok(&mut s, "device.add", json!({"track": 2, "kind": "reverb", "index": 0}));
    let ids: Vec<u32> = s.project.mixer[2].devices.iter().map(|d| d.id).collect();
    assert_eq!(ids, [3, 2], "inserted at the front, ids stay unique (1 is the master's limiter)");
    ok(&mut s, "device.set", json!({"id": 2, "param": 1, "value": 2.0}));
    assert_eq!(s.project.mixer[2].devices[1].params[1], 0.95, "feedback clamped");
    ok(&mut s, "device.move", json!({"id": 2, "index": 0}));
    ok(&mut s, "device.bypass", json!({"id": 3}));
    assert!(s.project.mixer[2].devices[1].bypass);
    ok(&mut s, "lfo.add", json!({}));
    ok(&mut s, "mod.map", json!({"source": {"type": "lfo", "index": 0}, "target": {"type": "device", "id": 2, "param": 3}, "amount": 0.4}));
    ok(&mut s, "mod.map", json!({"source": {"type": "macro", "index": 2}, "target": {"type": "synth", "slot": 5, "param": "cutoff"}}));
    assert!(s.run("mod.map", json!({"source": {"type": "lfo", "index": 3}, "target": {"type": "track_gain", "track": 0}})).is_err());
    assert!(s.run("mod.map", json!({"source": {"type": "macro", "index": 0}, "target": {"type": "synth", "slot": 0, "param": "cutoff"}})).is_err(), "slot 0 is a sampler");
    ok(&mut s, "device.remove", json!({"id": 2}));
    assert_eq!(s.project.mappings.len(), 1, "the delay's mapping went with it");
    ok(&mut s, "macro.set", json!({"index": 2, "value": 0.7, "name": "Brightness"}));
    assert_eq!(s.project.macros[2].name, "Brightness");
    ok(&mut s, "lfo.remove", json!({"index": 0}));
    ok(&mut s, "channel.set", json!({"channel": 0, "track": 2}));
    assert!(s.run("channel.set", json!({"channel": 0, "track": 99})).is_err());
}

#[test]
fn channels_from_presets() {
    let mut s = session();
    ok(&mut s, "channel.add", json!({"preset": "acid bass"}));
    let ch = s.project.channels.last().unwrap();
    assert_eq!(ch.name, "Acid Bass");
    assert_eq!(ch.synth().unwrap()[Param::Resonance as usize], 0.8);
    assert!(s.run("channel.add", json!({"preset": "nope"})).is_err());
    assert!(SYNTH_PRESETS.iter().all(|(name, _)| synth_preset(name).is_some()));
}

#[test]
fn sidechain_ducking() {
    let mut s = session(); // master 0, drums 1, synth 2
    ok(&mut s, "mixer.duck", json!({"track": 2, "source": 1}));
    let d = s.project.mixer[2].devices.last().unwrap().clone();
    assert_eq!((d.kind, d.sidechain), (DeviceKind::Compressor, Some(1)));
    assert!(s.run("mixer.set", json!({"track": 2, "route": 1})).is_err(), "drums key the synth, so the synth can't feed the drums");
    assert!(s.run("mixer.duck", json!({"track": 1, "source": 1})).is_err(), "not under itself");
    ok(&mut s, "device.sidechain", json!({"id": d.id}));
    assert_eq!(s.project.mixer[2].devices.last().unwrap().sidechain, None);
    ok(&mut s, "device.sidechain", json!({"id": d.id, "source": 1}));
    ok(&mut s, "mixer.remove_track", json!({"track": 1}));
    assert_eq!(s.project.mixer[1].devices.last().unwrap().sidechain, None, "the source is gone");
    ok(&mut s, "device.add", json!({"track": 1, "kind": "reverb"}));
    let reverb = s.project.mixer[1].devices.last().unwrap().id;
    assert!(s.run("device.sidechain", json!({"id": reverb, "source": 0})).is_err(), "only compressors");
}

#[test]
fn midi_learn_keeps_one_controller_per_parameter() {
    let mut s = Session::default();
    let gain = json!({ "type": "track_gain", "track": 1 });
    s.run("midi.learn", json!({ "channel": 0, "cc": 74, "target": gain })).unwrap();
    s.run("midi.learn", json!({ "channel": 0, "cc": 71, "target": gain })).unwrap();
    assert_eq!(s.project.midi_map, vec![CcMap { channel: 0, cc: 71, target: Target::TrackGain { track: 1 } }], "relearning replaces");
    s.run("midi.learn", json!({ "channel": 0, "cc": 71, "target": { "type": "track_pan", "track": 1 } })).unwrap();
    assert_eq!(s.project.midi_map[0].target, Target::TrackPan { track: 1 }, "a controller moves one thing");
    s.run("midi.forget", json!({ "target": { "type": "track_pan", "track": 1 } })).unwrap();
    assert!(s.project.midi_map.is_empty());
    assert!(s.run("midi.learn", json!({ "channel": 16, "cc": 1, "target": gain })).is_err());
    s.run("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.midi_map.len(), 1, "undoable");
}

#[test]
fn tracks_and_effects_get_a_name_color_and_icon() {
    let mut s = Session::default();
    s.run("mixer.set", json!({ "track": 1, "color": [200, 40, 90], "icon": "guitar" })).unwrap();
    let t = &s.project.mixer[1];
    assert_eq!((t.color, t.icon.as_deref()), (Some([200, 40, 90]), Some("guitar")));
    s.run("mixer.set", json!({ "track": 1, "auto_color": true, "icon": "" })).unwrap();
    assert_eq!((s.project.mixer[1].color, s.project.mixer[1].icon.clone()), (None, None), "back to automatic");
    s.run("device.add", json!({ "track": 1, "kind": "reverb" })).unwrap();
    let id = s.project.mixer[1].devices.last().unwrap().id;
    s.run("device.style", json!({ "id": id, "name": " Space ", "color": [10, 20, 30], "icon": "planet" })).unwrap();
    let d = s.project.mixer[1].devices.last().unwrap();
    assert_eq!((d.name.as_str(), d.color, d.icon.as_deref()), ("Space", Some([10, 20, 30]), Some("planet")));
    assert!(s.run("device.style", json!({ "id": 9999, "name": "x" })).is_err());
}

#[test]
fn the_loop_region_switches_on_and_off() {
    let mut s = Session::default();
    s.run("transport.loop", json!({ "from": 384, "to": 768 })).unwrap();
    assert_eq!(s.loop_range, Some((384, 768)), "drawing a loop turns it on");
    s.run("transport.loop", json!({ "on": false })).unwrap();
    assert_eq!((s.loop_range, s.loop_region), (None, Some((384, 768))), "off keeps the region");
    s.run("transport.loop", json!({ "on": true })).unwrap();
    assert_eq!(s.loop_range, Some((384, 768)));
    assert!(s.run("transport.loop", json!({ "from": 768, "to": 384 })).is_err());
    s.run("transport.loop", json!({ "clear": true })).unwrap();
    s.run("transport.loop", json!({ "on": true })).unwrap();
    assert_eq!(s.loop_range, None, "nothing to turn on");
}

#[test]
fn a_joined_edit_undoes_with_the_one_before() {
    let mut s = Session::default();
    s.run("clip.add", json!({ "lane": 0, "start": 0, "sample": "a.wav", "length": 96 })).unwrap();
    s.run("clip.add", json!({ "lane": 1, "start": 0, "sample": "a.wav", "length": 96 })).unwrap();
    s.run("clip.set_fx", json!({ "index": 0, "gain": -6.0 })).unwrap();
    s.run_joined("clip.set_fx", json!({ "index": 1, "gain": -6.0 })).unwrap();
    s.run("edit.undo", json!({})).unwrap();
    assert_eq!((s.project.clips[0].fx.gain, s.project.clips[1].fx.gain), (0.0, 0.0), "one step for both");
}

#[test]
fn patterns_carry_their_automation() {
    let mut s = Session::default();
    let gain = json!({ "type": "track_gain", "track": 1 });
    s.run("pattern.automation", json!({ "target": gain, "points": [[96, 0.9], [0, 0.1]] })).unwrap();
    let lanes = &s.project.patterns[s.pattern].automation;
    assert_eq!(lanes, &vec![PatternAuto { target: Target::TrackGain { track: 1 }, points: vec![(0, 0.1), (96, 0.9)], curves: vec![] }]);
    s.run("pattern.automation", json!({ "target": gain, "points": [[0, 0.5]] })).unwrap();
    assert_eq!(s.project.patterns[s.pattern].automation.len(), 1, "one line per knob");
    assert!(s.run("pattern.automation", json!({ "target": { "type": "track_gain", "track": 99 } })).is_err());
    let original = s.pattern;
    s.run("pattern.duplicate", json!({})).unwrap();
    assert_eq!(s.project.patterns.last().unwrap().automation.len(), 1, "a copy keeps it");
    s.run("pattern.automation", json!({ "pattern": original, "target": gain, "remove": true })).unwrap();
    assert!(s.project.patterns[original].automation.is_empty());
}

#[test]
fn automation_clips_keep_clean_points() {
    let mut s = Session::default();
    let gain = json!({ "type": "track_gain", "track": 1 });
    s.run("clip.add", json!({ "lane": 3, "start": 0, "automation": gain, "points": [[384, 2.0], [0, 0.5], [384, 0.1]] })).unwrap();
    let c = s.project.clips.last().unwrap();
    assert_eq!(c.length, 4 * BAR, "four bars unless told");
    assert_eq!(c.kind, ClipKind::Automation { target: Target::TrackGain { track: 1 }, points: vec![(0, 0.5), (384, 1.0)], curves: vec![] }, "sorted, clamped, one per tick");
    let i = s.project.clips.len() - 1;
    s.run("clip.automation", json!({ "index": i, "points": [[0, 0.0], [96, 1.0]] })).unwrap();
    assert_eq!(curve_at(&[(0, 0.0), (96, 1.0)], &[], 48.0), Some(0.5));
    assert_eq!(s.project.clips[i].kind, ClipKind::Automation { target: Target::TrackGain { track: 1 }, points: vec![(0, 0.0), (96, 1.0)], curves: vec![] });
    s.run("clip.add", json!({ "lane": 0, "start": 0 })).unwrap();
    assert!(s.run("clip.automation", json!({ "index": i + 1, "points": [] })).is_err(), "only automation clips have points");
}

#[test]
fn curves_bend_the_line_and_follow_its_points() {
    let up = [(0, 0.0), (100, 1.0)];
    let down = [(0, 1.0), (100, 0.0)];
    assert_eq!(curve_at(&up, &[], 50.0), Some(0.5), "straight");
    assert!(curve_at(&up, &[1.0], 50.0).unwrap() > 0.8, "bowed up: rises early");
    assert!(curve_at(&up, &[-1.0], 50.0).unwrap() < 0.2, "bowed down: rises late");
    assert!(curve_at(&down, &[1.0], 50.0).unwrap() > 0.8, "bowed up: falls late");
    assert_eq!(curve_at(&up, &[1.0], 100.0), Some(1.0), "ends where its point is");
    // a point put in a bent stretch splits it, both halves bent alike; taking it out joins them
    let (mut points, mut curves) = (up.to_vec(), vec![0.5]);
    insert_point(&mut points, &mut curves, 1, (50, 0.7));
    assert_eq!((points.len(), curves.clone()), (3, vec![0.5, 0.5]));
    remove_point(&mut points, &mut curves, 1);
    assert_eq!((points, curves), (up.to_vec(), vec![0.5]));
    assert!(clean_curves(vec![0.0, 0.0], 3).is_empty(), "all straight: none kept");
    assert_eq!(clean_curves(vec![3.0], 3), vec![1.0, 0.0]);
    // the samples to draw: its points, and the bent stretch in between
    let samples = curve_samples(&up, &[1.0], 0.0, 100.0);
    assert!(samples.len() > 10 && samples.first() == Some(&(0.0, 0.0)) && samples.last() == Some(&(100.0, 1.0)));
    let mut s = Session::default();
    s.run("clip.add", json!({ "lane": 0, "start": 0, "automation": { "type": "track_gain", "track": 1 }, "points": [[0, 0.0], [96, 1.0]], "curves": [0.6] })).unwrap();
    let i = s.project.clips.len() - 1;
    s.run("clip.automation", json!({ "index": i, "points": [[0, 0.0], [48, 0.5], [96, 1.0]] })).unwrap();
    let ClipKind::Automation { curves, .. } = &s.project.clips[i].kind else { unreachable!() };
    assert_eq!(curves, &vec![0.6, 0.0], "points without curves keep the curves they had");
}

#[test]
fn patterns_become_a_song() {
    let mut s = Session::default(); // channels: Kick 0, Snare 1, Clap 2, Hat 3, Open Hat 4, VA-1 5
    ok(&mut s, "step.set", json!({ "channel": 0, "step": 0, "velocity": 120 }));
    ok(&mut s, "pattern.add", json!({}));
    ok(&mut s, "step.set", json!({ "channel": 3, "step": 2, "velocity": 90 }));
    ok(&mut s, "pattern.add", json!({}));
    ok(&mut s, "note.add", json!({ "channel": 5, "key": 33, "start": 48, "length": 24 }));
    ok(&mut s, "song.structure", json!({}));
    let names: Vec<&str> = s.project.markers.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["Intro", "Build", "Drop", "Break", "Drop 2", "Outro"]);
    assert_eq!(s.project.song_end(), 128 * BAR);
    let clips_at = |bar: u32| s.project.clips.iter().filter(|c| c.start == bar * BAR).count();
    assert_eq!((clips_at(0), clips_at(16), clips_at(32), clips_at(64)), (2, 3, 3, 2), "kick + hats, then the bass, the drop, the break without the kick");
    assert!(s.project.clips.iter().filter(|c| c.start == 64 * BAR).all(|c| c.lane != 0), "no kick in the break");
    ok(&mut s, "edit.undo", json!({}));
    assert!(s.project.clips.is_empty() && s.project.markers.is_empty(), "one undo");
}

#[test]
fn markers_stay_in_time_order() {
    let mut s = Session { position: 4 * BAR, ..Default::default() };
    ok(&mut s, "marker.add", json!({}));
    ok(&mut s, "marker.add", json!({ "tick": 0, "name": "Intro" }));
    assert_eq!(s.project.markers.iter().map(|m| (m.tick, m.name.as_str())).collect::<Vec<_>>(), vec![(0, "Intro"), (4 * BAR, "Marker 1")]);
    ok(&mut s, "marker.set", json!({ "index": 0, "tick": 8 * BAR, "name": "Drop" }));
    assert_eq!(s.project.markers[1].name, "Drop", "moved past the other");
    ok(&mut s, "marker.remove", json!({ "index": 0 }));
    assert_eq!(s.project.markers.len(), 1);
    assert!(s.run("marker.remove", json!({ "index": 5 })).is_err());
}

#[test]
fn a_group_undoes_as_one_step() {
    let mut s = session();
    let original = s.project.clone();
    ok(&mut s, "transport.set_tempo", json!({ "bpm": 100.0 }));
    let before = s.project.clone();
    s.begin_group("Agent edits");
    set_cutoff(&mut s, 900.0);
    ok(&mut s, "pattern.add", json!({}));
    ok(&mut s, "transport.set_tempo", json!({ "bpm": 140.0 }));
    assert_eq!(s.undo_label(), Some("Agent edits"));
    ok(&mut s, "edit.undo", json!({}));
    assert_eq!(s.project, before, "the whole group, nothing before it");
    assert!(!s.in_group(), "undo closed the group");
    ok(&mut s, "edit.undo", json!({}));
    assert_eq!(s.project, original);
    s.begin_group("Nothing");
    s.end_group();
    assert_eq!(s.undo_label(), None, "an empty group leaves no step");
}

#[test]
fn a_solo_keeps_what_it_feeds_and_what_feeds_it() {
    let mut s = Session::default();
    s.project.mixer.truncate(1);
    for _ in 0..4 {
        ok(&mut s, "mixer.add_track", json!({}));
    }
    // 1 → 2 (a bus) → master, 3 → master with a send to 4 (a return), 4 → master
    ok(&mut s, "mixer.set", json!({ "track": 1, "route": 2 }));
    ok(&mut s, "mixer.set_send", json!({ "track": 3, "to": 4, "amount": 0.3 }));
    assert!(s.project.tracks_heard().iter().all(|h| *h), "no solo: everything");
    ok(&mut s, "mixer.set", json!({ "track": 2, "solo": true }));
    assert_eq!(s.project.tracks_heard(), vec![true, true, true, false, false], "the bus, its input, the master");
    ok(&mut s, "mixer.set", json!({ "track": 2, "solo": false }));
    ok(&mut s, "mixer.set", json!({ "track": 3, "solo": true }));
    assert_eq!(s.project.tracks_heard(), vec![true, false, false, true, true], "the track and its return");
    ok(&mut s, "mixer.set", json!({ "track": 0, "solo": true }));
    assert!(!s.project.mixer[0].solo, "the master can't be soloed");
}

#[test]
fn a_channel_takes_its_own_swing_and_envelope() {
    let mut s = Session::default();
    ok(&mut s, "channel.set", json!({ "channel": 0, "swing": 0.6, "attack": 0.01, "release": 9.0 }));
    let c = &s.project.channels[0];
    assert_eq!((c.swing, c.attack, c.release), (Some(0.6), 0.01, 2.0), "the release is clamped");
    ok(&mut s, "channel.set", json!({ "channel": 0, "swing": -1.0 }));
    assert_eq!(s.project.channels[0].swing, None, "negative: back to the song's");
}

#[test]
fn macros_randomize_and_come_back_from_a_variation() {
    let mut s = Session::default();
    assert!(s.run("macro.randomize", json!({})).is_err(), "nothing mapped: nothing to roll");
    ok(&mut s, "mod.map", json!({ "source": { "type": "macro", "index": 1 }, "target": { "type": "track_gain", "track": 0 }, "amount": 0.5 }));
    ok(&mut s, "macro.store", json!({}));
    ok(&mut s, "macro.randomize", json!({ "seed": 7 }));
    let rolled = s.project.macros[1].value;
    assert!(rolled != 0.0, "the mapped macro moved");
    assert_eq!(s.project.macros[0].value, 0.0, "an unmapped one stays");
    ok(&mut s, "macro.recall", json!({ "index": 0 }));
    assert_eq!(s.project.macros[1].value, 0.0);
    ok(&mut s, "macro.forget", json!({ "index": 0 }));
    assert!(s.project.variations.is_empty());
}

#[test]
fn deleting_a_track_renumbers_what_plays_into_the_ones_after() {
    let mut s = Session::default();
    ok(&mut s, "mixer.add_track", json!({}));
    ok(&mut s, "mixer.add_track", json!({}));
    let last = s.project.mixer.len() - 1;
    s.project.clips.push(Clip { lane: 0, start: 0, length: BAR, offset: 0, fx: ClipFx { track: last, ..ClipFx::default() }, kind: ClipKind::Audio { sample: "x.wav".into() } });
    s.project.midi_map.push(CcMap { channel: 0, cc: 1, target: Target::TrackGain { track: last } });
    s.project.midi_map.push(CcMap { channel: 0, cc: 2, target: Target::TrackGain { track: last - 1 } });
    ok(&mut s, "mixer.remove_track", json!({ "track": last - 1 }));
    assert_eq!(s.project.clips[0].fx.track, last - 1, "the clip follows its track down");
    assert_eq!(s.project.midi_map.len(), 1, "the deleted track's controller goes");
    assert_eq!(s.project.midi_map[0].target, Target::TrackGain { track: last - 1 });
}

#[test]
fn a_bent_stretch_bows_alike_both_ways_and_never_shoots_up() {
    for c in [0.3f32, 1.0, -1.0] {
        // rising or falling, the same curve is the same bow (mirror images)
        for k in [0.1f32, 0.5, 0.9] {
            assert!((bend(k, c, true) - (1.0 - bend(1.0 - k, c, false))).abs() < 1e-5, "{c} at {k}");
        }
        // no vertical start: the first hundredth of the stretch goes a little way, not half
        assert!(bend(0.01, c, true) < 0.1 && bend(0.01, c, false) < 0.1, "{c}");
    }
    assert!(bend(0.5, 1.0, true) > 0.9 && bend(0.5, 1.0, false) < 0.1, "bowed right up: rises early, falls late");
    assert_eq!(bend(0.25, 0.0, true), 0.25, "straight");
}

/// Typed values read in the unit they're shown in, and land inside the range.
#[test]
fn params_read_typed_values() {
    let unit = |unit: &'static str, min: f32, max: f32| ParamInfo { unit, min, max, ..*Param::Cutoff.info() };
    let hz = unit("Hz", 20.0, 20_000.0);
    assert_eq!(hz.parse("1.2k", 500.0), Some(1200.0));
    assert_eq!(hz.parse("440", 5000.0), Some(440.0));
    assert_eq!(hz.parse("99999", 500.0), Some(20_000.0));
    let s = unit("s", 0.001, 10.0);
    assert_eq!(s.parse("250 ms", 1.0), Some(0.25));
    assert_eq!(s.parse("300", 0.2), Some(0.3), "a bare number in the ms showing");
    assert_eq!(s.parse("2", 1.5), Some(2.0));
    assert_eq!(unit("%", 0.0, 1.0).parse("40%", 0.0), Some(0.4));
    let pan = unit("pan", -1.0, 1.0);
    assert_eq!(pan.parse("L30", 0.0), Some(-0.3));
    assert_eq!(pan.parse("c", 0.5), Some(0.0));
    assert_eq!(unit("dB", -60.0, 12.0).parse("-6,5 dB", 0.0), Some(-6.5));
    assert_eq!(unit("wave", 0.0, 3.0).parse("2", 0.0), None);
    assert_eq!(hz.parse("loud", 500.0), None);
}
