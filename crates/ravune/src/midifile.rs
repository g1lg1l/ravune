//! Standard MIDI files (`.mid`): their notes, track by track, and the tempo, for File → Open
//! and drops. Format 0 and 1; a format 0 file's channels become tracks of their own.

use std::collections::BTreeMap;
use std::path::Path;

use ravune_core::{MAX_CHANNELS, MAX_STEPS, PPQ, STEP, Session};
use serde_json::json;

use crate::App;
use crate::shell::Tone;

/// A note: key, start and length in the caller's ticks, velocity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    pub key: u8,
    pub start: u32,
    pub length: u32,
    pub velocity: u8,
}

#[derive(Debug, PartialEq)]
pub struct Track {
    pub name: String,
    /// MIDI channel 10, where General MIDI puts the drums.
    pub drums: bool,
    pub notes: Vec<Note>,
}

#[derive(Debug, PartialEq)]
pub struct MidiSong {
    /// The first tempo the file sets.
    pub bpm: Option<f32>,
    /// Only tracks with notes, in the file's order.
    pub tracks: Vec<Track>,
}

/// Reads a MIDI file, its times turned into ticks at `ppq` per beat.
pub fn parse(bytes: &[u8], ppq: u32) -> Result<MidiSong, String> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(4).ok() != Some(b"MThd".as_slice()) {
        return Err("not a MIDI file".into());
    }
    let header = r.u32()? as usize;
    let (_format, count, division) = (r.u16()?, r.u16()?, r.u16()?);
    r.skip(header.saturating_sub(6))?;
    if division & 0x8000 != 0 || division == 0 {
        return Err("MIDI files timed in SMPTE frames aren't supported".into());
    }
    let ticks = |t: u64| (t * ppq as u64 / division as u64) as u32;
    let mut bpm = None;
    // (track, channel) → its notes
    let mut parts: BTreeMap<(usize, u8), Vec<Note>> = BTreeMap::new();
    let mut names: Vec<String> = Vec::new();
    for track in 0..count as usize {
        // a file cut short ends the reading with what it had
        let Ok(id) = r.take(4) else { break };
        let len = r.u32()? as usize;
        if id != b"MTrk" {
            r.skip(len)?;
            names.push(String::new());
            continue;
        }
        let mut t = Reader { bytes: r.take(len)?, at: 0 };
        let (mut now, mut status) = (0u64, 0u8);
        let mut name = String::new();
        // notes sounding: (channel, key) → (start, velocity), latest last
        let mut open: BTreeMap<(u8, u8), Vec<(u64, u8)>> = BTreeMap::new();
        while t.at < t.bytes.len() {
            now += t.var()? as u64;
            let mut byte = t.u8()?;
            if byte < 0x80 {
                // running status: the byte is data for the last message's kind
                t.at -= 1;
                byte = status;
            }
            match byte {
                0xFF => {
                    let (kind, len) = (t.u8()?, t.var()? as usize);
                    let data = t.take(len)?;
                    match kind {
                        0x03 if name.is_empty() => name = String::from_utf8_lossy(data).trim().to_string(),
                        0x51 if bpm.is_none() && len == 3 => {
                            let micros = u32::from_be_bytes([0, data[0], data[1], data[2]]);
                            bpm = (micros > 0).then(|| 60_000_000.0 / micros as f32);
                        }
                        0x2F => break,
                        _ => {}
                    }
                }
                0xF0 | 0xF7 => {
                    let len = t.var()? as usize;
                    t.skip(len)?;
                }
                0x80..=0xEF => {
                    status = byte;
                    let channel = byte & 0x0F;
                    let first = t.u8()? & 0x7F;
                    let second = if matches!(byte & 0xF0, 0xC0 | 0xD0) { 0 } else { t.u8()? & 0x7F };
                    match byte & 0xF0 {
                        0x90 if second > 0 => open.entry((channel, first)).or_default().push((now, second)),
                        0x80 | 0x90 => {
                            if let Some((start, velocity)) = open.get_mut(&(channel, first)).and_then(Vec::pop) {
                                let note = Note { key: first, start: ticks(start), length: ticks(now).saturating_sub(ticks(start)).max(1), velocity };
                                parts.entry((track, channel)).or_default().push(note);
                            }
                        }
                        _ => {}
                    }
                }
                _ => return Err(format!("unexpected byte {byte:#04x} in track {}", track + 1)),
            }
        }
        // notes never let go end with their track
        for ((channel, key), starts) in open {
            for (start, velocity) in starts {
                parts.entry((track, channel)).or_default().push(Note { key, start: ticks(start), length: ticks(now).saturating_sub(ticks(start)).max(1), velocity });
            }
        }
        names.push(name);
    }
    let channels_of = |track: usize| parts.keys().filter(|(t, _)| *t == track).count();
    let tracks = parts
        .iter()
        .map(|(&(track, channel), notes)| {
            let name = match names.get(track).filter(|n| !n.is_empty()) {
                Some(n) if channels_of(track) == 1 => n.clone(),
                Some(n) => format!("{n} {}", channel + 1),
                None if channel == 9 => "Drums".into(),
                None => format!("Channel {}", channel + 1),
            };
            let mut notes = notes.clone();
            notes.sort_by_key(|n| (n.start, n.key));
            Track { name, drums: channel == 9, notes }
        })
        .collect();
    Ok(MidiSong { bpm, tracks })
}

/// Puts `song` into the project as one undo step: a VA-1 channel per track, its notes in
/// patterns of up to 32 bars, those in a row on a new playlist lane from the start. An empty
/// playlist takes the file's tempo too. Returns how many tracks came in.
pub fn import(session: &mut Session, song: &MidiSong, name: &str) -> Result<usize, String> {
    let room = MAX_CHANNELS - session.project.channels.len();
    let tracks = &song.tracks[..song.tracks.len().min(room)];
    if tracks.is_empty() {
        return Err(if song.tracks.is_empty() { "it has no notes".into() } else { "all 64 channels are in use".into() });
    }
    let bar = 16 * STEP;
    let end = tracks.iter().flat_map(|t| t.notes.iter().map(|n| n.start + n.length)).max().unwrap_or(0).div_ceil(bar).max(1) * bar;
    let most = MAX_STEPS as u32 * STEP;
    let parts = end.div_ceil(most);
    session.begin_group("Import MIDI");
    let result = (|| {
        if session.project.clips.is_empty()
            && let Some(bpm) = song.bpm
        {
            session.run("transport.set_tempo", json!({ "bpm": bpm.clamp(20.0, 300.0) }))?;
        }
        let first = session.project.channels.len();
        for track in tracks {
            session.run("channel.add", json!({ "name": track.name }))?;
        }
        let lane = session.project.clips.iter().map(|c| c.lane + 1).max().unwrap_or(0);
        for k in 0..parts {
            let (from, to) = (k * most, ((k + 1) * most).min(end));
            let title = if parts == 1 { name.to_string() } else { format!("{name} {}", k + 1) };
            session.run("pattern.add", json!({ "name": title }))?;
            let pattern = session.project.patterns.len() - 1;
            session.run("pattern.set_length", json!({ "pattern": pattern, "steps": (to - from) / STEP }))?;
            for (i, track) in tracks.iter().enumerate() {
                // a note crossing into the next pattern ends with this one
                let notes: Vec<_> = track
                    .notes
                    .iter()
                    .filter(|n| (from..to).contains(&n.start))
                    .map(|n| json!({ "key": n.key, "start": n.start - from, "length": n.length.min(to - n.start), "velocity": n.velocity.max(1) }))
                    .collect();
                if !notes.is_empty() {
                    session.run("notes.add", json!({ "channel": first + i, "pattern": pattern, "notes": notes }))?;
                }
            }
            session.run("clip.add", json!({ "lane": lane, "start": from, "pattern": pattern }))?;
        }
        Ok(tracks.len())
    })();
    session.end_group();
    // half an import goes back; a failure that changed nothing leaves no step to take back
    if result.is_err() && session.undo_label() == Some("Import MIDI") {
        let _ = session.run("edit.undo", json!({}));
    }
    result
}

impl App {
    /// A `.mid` dropped or opened: its tracks into this song.
    pub fn import_midi(&mut self, path: &Path) {
        let name = path.file_stem().map_or("MIDI".into(), |s| s.to_string_lossy().into_owned());
        let song = std::fs::read(path).map_err(|e| e.to_string()).and_then(|bytes| parse(&bytes, PPQ));
        self.user_edit("channel.add");
        match song.and_then(|song| import(&mut self.session, &song, &name).map(|n| (n, song.tracks.len()))) {
            Ok((n, all)) if n < all => self.say_as(Tone::Info, tr!("Brought in {} of {} tracks from {}: the other ones need free channels", n, all, name)),
            Ok((n, _)) => self.say_as(Tone::Done, tr!("Brought in {} tracks from {}, each on a VA-1: swap the sounds in the channel rack", n, name)),
            Err(e) => self.say_as(Tone::Error, tr!("Couldn't bring in {}: {}", name, e)),
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let out = self.bytes.get(self.at..self.at + n).ok_or("the MIDI file ends too soon")?;
        self.at += n;
        Ok(out)
    }

    fn skip(&mut self, n: usize) -> Result<(), String> {
        self.take(n).map(|_| ())
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A variable-length number: 7 bits a byte, the top bit set on all but the last.
    fn var(&mut self) -> Result<u32, String> {
        let mut v = 0u32;
        for _ in 0..4 {
            let b = self.u8()?;
            v = (v << 7) | (b & 0x7F) as u32;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err("a broken length in the MIDI file".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A format 1 file built by hand: a tempo track, then "Bass" playing on channel 1 (with
    /// running status, and a note-on of velocity 0 for a note-off) and a kick on channel 10.
    #[test]
    fn reads_tracks_notes_and_tempo() {
        let chunk = |events: &[u8]| [b"MTrk".as_slice(), &(events.len() as u32).to_be_bytes(), events].concat();
        let tempo = chunk(&[0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, 0x00, 0xFF, 0x2F, 0x00]); // 500000 µs: 120 BPM
        let bass = chunk(&[
            0x00, 0xFF, 0x03, 0x04, b'B', b'a', b's', b's', // its name
            0x00, 0x90, 60, 100, // C4 on
            0x83, 0x60, 60, 0, // 480 later, off: running status, velocity 0
            0x00, 64, 80, // E4 on
            0x81, 0x70, 0x80, 64, 0, // 240 later, off
            0x00, 0x99, 36, 127, // a kick on channel 10, never let go
            0x83, 0x60, 0xFF, 0x2F, 0x00, // the end, 480 later
        ]);
        let header = [b"MThd".as_slice(), &6u32.to_be_bytes(), &1u16.to_be_bytes(), &2u16.to_be_bytes(), &480u16.to_be_bytes()].concat();
        let song = parse(&[header, tempo, bass].concat(), 96).unwrap();
        assert_eq!(song.bpm, Some(120.0));
        assert_eq!(song.tracks.len(), 2);
        let bass = &song.tracks[0];
        assert_eq!((bass.name.as_str(), bass.drums), ("Bass 1", false));
        assert_eq!(bass.notes, [Note { key: 60, start: 0, length: 96, velocity: 100 }, Note { key: 64, start: 96, length: 48, velocity: 80 }]);
        let kick = &song.tracks[1];
        assert_eq!((kick.name.as_str(), kick.drums), ("Bass 10", true));
        assert_eq!(kick.notes, [Note { key: 36, start: 144, length: 96, velocity: 127 }]);
        assert!(parse(b"RIFF....", 96).is_err());
    }

    /// 40 bars of one note a bar: two patterns (32 bars, then 8) in a row on a new lane, a
    /// channel for the track, the file's tempo, and it all undoes in one step.
    #[test]
    fn imports_into_patterns_in_a_row() {
        let bar = 16 * STEP;
        let notes = (0..40).map(|k| Note { key: 48, start: k * bar, length: if k == 39 { bar } else { bar + STEP }, velocity: 90 }).collect();
        let song = MidiSong { bpm: Some(100.0), tracks: vec![Track { name: "Pad".into(), drums: false, notes }] };
        let mut session = Session::with_project(ravune_core::Project::default());
        let (channels, patterns) = (session.project.channels.len(), session.project.patterns.len());
        assert_eq!(import(&mut session, &song, "song"), Ok(1));
        let p = &session.project;
        assert_eq!(p.tempo, 100.0);
        assert_eq!(p.channels.last().unwrap().name, "Pad");
        assert_eq!(p.patterns.len(), patterns + 2);
        let (a, b) = (&p.patterns[patterns], &p.patterns[patterns + 1]);
        assert_eq!((a.name.as_str(), a.length, b.length), ("song 1", 512, 128));
        let slot = p.channels.last().unwrap().slot;
        assert_eq!((a.notes(slot).len(), b.notes(slot).len()), (32, 8));
        assert_eq!(a.notes(slot)[31].length, bar, "the last note stops at the pattern's end");
        assert_eq!(p.clips.iter().map(|c| (c.start, c.length)).collect::<Vec<_>>(), [(0, 32 * bar), (32 * bar, 8 * bar)]);
        let _ = session.run("edit.undo", json!({}));
        assert_eq!((session.project.channels.len(), session.project.patterns.len()), (channels, patterns));
    }
}
