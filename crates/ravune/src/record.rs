//! Recording what plays: the engine copies the master into a ring, the UI drains it every
//! frame and, when the take ends, saves it as a WAV on a new playlist lane.

use std::path::PathBuf;

use ravune_core::PPQ;
use ravune_engine::rtrb::{Consumer, RingBuffer};
use ravune_engine::{Msg, write_wav};
use serde_json::json;

use crate::App;
use crate::shell::Tone;

/// Seconds of audio the ring holds between two drains.
// ponytail: drained from the UI loop; a window frozen longer than this drops blocks, a
// writer thread if that ever matters.
const RING_SECONDS: u32 = 10;

pub struct Recording {
    ring: Consumer<f32>,
    /// Interleaved stereo so far.
    frames: Vec<f32>,
    /// Where the take goes on the playlist (ticks).
    start: u32,
    rate: u32,
}

impl Recording {
    fn drain(&mut self) {
        if let Ok(chunk) = self.ring.read_chunk(self.ring.slots()) {
            let (a, b) = chunk.as_slices();
            self.frames.extend_from_slice(a);
            self.frames.extend_from_slice(b);
            chunk.commit_all();
        }
    }
}

/// Documents/Ravune/Recordings.
fn recordings_dir() -> PathBuf {
    crate::browser::library_dir().with_file_name("Recordings")
}

impl App {
    /// Starts a take (and playback, if stopped), or ends the one running.
    pub fn toggle_record(&mut self) {
        if self.recording.is_some() {
            self.stop_recording();
            return;
        }
        let Some(engine) = &mut self.engine else {
            self.say_as(Tone::Error, tr!("No audio output: nothing to record"));
            return;
        };
        let rate = engine.sample_rate;
        let (tap, ring) = RingBuffer::new((rate * 2 * RING_SECONDS) as usize);
        engine.send(Msg::Record(Some(tap)));
        let start = match self.beat {
            Some(beat) if self.session.song_mode => (beat * PPQ as f64) as u32,
            _ => self.session.position,
        };
        self.recording = Some(Recording { ring, frames: Vec::new(), start, rate });
        if !self.session.playing {
            self.run("transport.play_toggle", json!({}));
        }
    }

    /// Collects what the engine recorded; stopping playback ends the take.
    pub fn poll_recording(&mut self) {
        let Some(rec) = &mut self.recording else { return };
        rec.drain();
        if !self.session.playing {
            self.stop_recording();
        }
    }

    fn stop_recording(&mut self) {
        let Some(mut rec) = self.recording.take() else { return };
        if let Some(engine) = &mut self.engine {
            engine.send(Msg::Record(None));
        }
        rec.drain();
        if rec.frames.is_empty() {
            self.say_as(Tone::Error, tr!("Nothing was recorded"));
            return;
        }
        let dir = recordings_dir();
        let path = (1..).map(|n| dir.join(format!("Recording {n}.wav"))).find(|p| !p.exists()).unwrap();
        let saved = std::fs::create_dir_all(&dir).and_then(|_| write_wav(&path, &rec.frames, 2, rec.rate));
        if let Err(e) = saved {
            self.say_as(Tone::Error, tr!("Couldn't save the recording: {}", e));
            return;
        }
        let lane = self.session.project.clips.iter().map(|c| c.lane + 1).max().unwrap_or(0);
        self.add_audio_clip(&path.to_string_lossy(), lane, rec.start);
        let seconds = rec.frames.len() as f32 / 2.0 / rec.rate as f32;
        self.say_as(Tone::Done, tr!("Recorded {} s to {}", format!("{seconds:.1}"), path.display()));
    }
}
