//! Ravune engine: real-time audio.
//!
//! The UI compiles the project into an immutable [`Song`] and swaps it into the audio
//! thread through a lock-free queue; the old one comes back on a second queue so the
//! audio thread never frees memory. The callback never allocates, locks or blocks
//! (enforced by `assert_no_alloc` in debug builds).

mod analyze;
mod effects;
mod instrument;
mod renderer;
mod sample;
mod smart;
mod song;
mod stretch;
mod synth;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, ErrorKind, FromSample, SampleFormat, SizedSample, StreamConfig, SupportedBufferSize};
pub use analyze::{Analysis, BANDS, KEYS, analyze, lufs, true_peak};
pub use effects::{Effect, Effects, LOOK_AHEAD};
use ravune_core::MAX_TRACKS;
use renderer::BLOCK;
pub use renderer::Renderer;
pub use instrument::{MultiMap, Region};
pub use sample::{Bucket, Sample, Sampler, VoiceMode, write_wav, write_wav_as};
pub use smart::{HeardNote, melody_notes, split_hpss};
pub use song::{Mode, Song, SongChannel, Sound, Transport};
pub use stretch::{Stretcher, detect_bpm};
/// The ring a recording comes back through ([`Msg::Record`]).
pub use rtrb;

/// Renders the whole playlist once (no looping) plus a release tail, for export.
/// Returns interleaved stereo. `progress` gets 0..1 as it goes.
pub fn render_offline(mut song: Song, sample_rate: f32, tail_seconds: f32, progress: impl Fn(f32)) -> Vec<f32> {
    song.playing = true;
    song.looping = false;
    song.mode = Mode::Song;
    song.start = 0.0;
    let seconds = song.end * song.tick_seconds() + tail_seconds as f64;
    render(song, sample_rate, (seconds * sample_rate as f64) as usize, progress)
}

/// Renders the playlist from tick `from` to `to` once, plus a release tail: the loop region's export.
pub fn render_region(mut song: Song, sample_rate: f32, (from, to): (f64, f64), tail_seconds: f32, progress: impl Fn(f32)) -> Vec<f32> {
    song.playing = true;
    song.looping = false;
    song.mode = Mode::Song;
    song.start = from;
    song.looped = Some((from, to));
    let seconds = (to - from) * song.tick_seconds() + tail_seconds as f64;
    render(song, sample_rate, (seconds * sample_rate as f64) as usize, progress)
}

/// Renders `seconds` from wherever `song`'s transport starts, in its mode (a pattern loops,
/// the song stops at its end): a listening test. Interleaved stereo.
pub fn render_span(mut song: Song, sample_rate: f32, seconds: f32) -> Vec<f32> {
    song.playing = true;
    render(song, sample_rate, (seconds * sample_rate) as usize, |_| {})
}

fn render(song: Song, sample_rate: f32, frames: usize, progress: impl Fn(f32)) -> Vec<f32> {
    let mut renderer = Renderer::new(sample_rate);
    for msg in Effects::default().messages(&song, sample_rate) {
        renderer.handle(msg);
    }
    renderer.handle(Msg::Song(Arc::new(song)));
    let mut out = Vec::with_capacity(frames * 2);
    let (mut l, mut r) = ([0.0; BLOCK], [0.0; BLOCK]);
    let blocks = frames.div_ceil(BLOCK);
    for i in 0..blocks {
        let n = BLOCK.min(frames - i * BLOCK);
        // SAFETY: only flips the FPU's flush-to-zero flags; reverb and delay tails decay into denormals
        unsafe { no_denormals::no_denormals(|| renderer.render(&mut l[..n], &mut r[..n])) };
        out.extend(l[..n].iter().zip(&r[..n]).flat_map(|(a, b)| [*a, *b]));
        if i % 1024 == 0 {
            progress(i as f32 / blocks as f32);
        }
    }
    progress(1.0);
    out
}

/// What the audio thread hands back to be freed elsewhere.
pub enum Garbage {
    Song(Arc<Song>),
    Effect(Box<Effect>),
    Plugin(Arc<ravune_bridge::Bridge>),
    /// A finished recording's end of the ring.
    Tap(rtrb::Producer<f32>),
}

/// What a plugin host serves: a channel's instrument (by slot) or a plugin effect (by id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PluginTarget {
    Channel(u8),
    Device(u32),
}

/// Messages to the audio thread. Sending never allocates on the audio side.
pub enum Msg {
    /// A key went down on the computer keyboard, the on-screen piano or MIDI.
    NoteOn { note: u8, velocity: f32 },
    NoteOff { note: u8 },
    /// A controller's channel message as it came (notes with their channel, pitch bend,
    /// pressure, CCs), so MPE reaches the synth and plugins per note.
    Midi([u8; 3]),
    AllNotesOff,
    /// Which channel slot the keys play.
    Target(Option<u8>),
    Song(Arc<Song>),
    /// Browser preview (`None` stops it).
    Preview(Option<Arc<Sample>>),
    /// A device's effect instance, built on the UI thread. Sent before the song that uses it.
    AddEffect(u32, Box<Effect>),
    RemoveEffect(u32),
    /// A plugin's bridge (its host process is up), or `None` to let go of it.
    Plugin { target: PluginTarget, bridge: Option<Arc<ravune_bridge::Bridge>> },
    /// The app's own output volume (0..2), after the mixer: not part of the project.
    Output(f32),
    /// Record: while the song plays, the master (as mixed: no preview, no click, before
    /// the output volume) goes into this ring, left and right interleaved. `None` stops.
    Record(Option<rtrb::Producer<f32>>),
}

// ---------------------------------------------------------------------------
// Stats: audio thread → UI, lock-free.
// ---------------------------------------------------------------------------

/// Frames of the output kept for the UI to look at (a visualizer).
pub const SCOPE: usize = 4096;

/// The latest output in mono, round and round: what the visualizer draws.
struct Scope {
    frames: Box<[AtomicU32]>,
    /// Frames written so far; the next goes at `at % SCOPE`.
    at: AtomicUsize,
}

impl Default for Scope {
    fn default() -> Self {
        Self { frames: (0..SCOPE).map(|_| AtomicU32::new(0)).collect(), at: AtomicUsize::new(0) }
    }
}

#[derive(Default)]
pub struct Stats {
    peak: AtomicU32,
    scope: Scope,
    beat: AtomicU64,
    held: [AtomicU64; 2],
    frames: AtomicU32,
    load: AtomicU32,
    xruns: AtomicU32,
    failed: AtomicBool,
    meters: [AtomicU32; MAX_TRACKS],
}

impl Stats {
    /// Peak level since the last call.
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.peak.swap(0, Relaxed))
    }
    /// The latest `out.len()` frames of the output (up to [`SCOPE`]), oldest first, in mono.
    pub fn scope(&self, out: &mut [f32]) {
        let n = out.len().min(SCOPE);
        let at = self.scope.at.load(Relaxed).wrapping_sub(n);
        for (k, o) in out[..n].iter_mut().enumerate() {
            *o = f32::from_bits(self.scope.frames[at.wrapping_add(k) % SCOPE].load(Relaxed));
        }
    }
    /// Peak level of a mixer track since the last call (post-fader).
    pub fn take_meter(&self, track: usize) -> f32 {
        self.meters.get(track).map_or(0.0, |m| f32::from_bits(m.swap(0, Relaxed)))
    }
    pub fn beat(&self) -> f64 {
        f64::from_bits(self.beat.load(Relaxed))
    }
    pub fn is_held(&self, note: u8) -> bool {
        let note = note & 127;
        self.held[note as usize / 64].load(Relaxed) & (1 << (note % 64)) != 0
    }
    pub fn any_held(&self) -> bool {
        self.held.iter().any(|h| h.load(Relaxed) != 0)
    }
    /// Frames per audio callback, as delivered by the device.
    pub fn buffer_frames(&self) -> u32 {
        self.frames.load(Relaxed)
    }
    /// Share of the callback's time budget spent rendering (0..1+).
    pub fn load(&self) -> f32 {
        f32::from_bits(self.load.load(Relaxed))
    }
    pub fn xruns(&self) -> u32 {
        self.xruns.load(Relaxed)
    }
    /// The stream died (device unplugged, invalidated…) and must be rebuilt.
    pub fn failed(&self) -> bool {
        self.failed.load(Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Engine: owns the device stream.
// ---------------------------------------------------------------------------

/// Where MIDI threads push notes. Swapped to the new queue whenever the engine restarts.
pub type MidiSink = Arc<Mutex<Option<rtrb::Producer<Msg>>>>;

pub struct Engine {
    _stream: cpal::Stream,
    tx: rtrb::Producer<Msg>,
    trash: rtrb::Consumer<Garbage>,
    stats: Arc<Stats>,
    effects: Effects,
    pub device: String,
    pub sample_rate: u32,
}

const QUEUE: usize = 1024;

struct Audio {
    renderer: Renderer,
    ui: rtrb::Consumer<Msg>,
    midi: rtrb::Consumer<Msg>,
    trash: rtrb::Producer<Garbage>,
    stats: Arc<Stats>,
    channels: usize,
    sr: f32,
    l: [f32; BLOCK],
    r: [f32; BLOCK],
}

impl Audio {
    fn handle(&mut self, msg: Msg) {
        if let Some(old) = self.renderer.handle(msg)
            && let Err(rtrb::PushError::Full(old)) = self.trash.push(old)
        {
            // ponytail: UI hasn't drained the trash; leak rather than free on the audio thread
            std::mem::forget(old);
        }
    }

    fn process<T: SizedSample + FromSample<f32>>(&mut self, data: &mut [T]) {
        let started = Instant::now();
        while let Ok(m) = self.ui.pop() {
            self.handle(m);
        }
        while let Ok(m) = self.midi.pop() {
            self.handle(m);
        }
        let mut peak = 0.0f32;
        let scope = &self.stats.scope;
        let mut at = scope.at.load(Relaxed);
        for chunk in data.chunks_mut(BLOCK * self.channels) {
            let n = chunk.len() / self.channels;
            let (l, r) = (&mut self.l[..n], &mut self.r[..n]);
            self.renderer.render(l, r);
            for (i, frame) in chunk.chunks_mut(self.channels).enumerate() {
                peak = peak.max(l[i].abs()).max(r[i].abs());
                scope.frames[at % SCOPE].store(((l[i] + r[i]) * 0.5).to_bits(), Relaxed);
                at = at.wrapping_add(1);
                for (c, out) in frame.iter_mut().enumerate() {
                    let x = match (c, self.channels) {
                        (_, 1) => (l[i] + r[i]) * 0.5,
                        (0, _) => l[i],
                        (1, _) => r[i],
                        _ => 0.0,
                    };
                    *out = T::from_sample(x);
                }
            }
        }
        scope.at.store(at, Relaxed);
        let frames = (data.len() / self.channels) as u32;
        let s = &self.stats;
        for (m, x) in s.meters.iter().zip(self.renderer.take_meters()) {
            m.fetch_max(x.to_bits(), Relaxed);
        }
        s.peak.fetch_max(peak.to_bits(), Relaxed); // positive floats order like their bits
        s.beat.store(self.renderer.beat().to_bits(), Relaxed);
        let held = self.renderer.held_notes();
        s.held[0].store(held as u64, Relaxed);
        s.held[1].store((held >> 64) as u64, Relaxed);
        s.frames.store(frames, Relaxed);
        let budget = frames as f32 / self.sr;
        let load = started.elapsed().as_secs_f32() / budget.max(1e-6);
        s.load.store(load.to_bits(), Relaxed);
        // a block that took longer than it lasts reached the device late: a dropout, even
        // where the driver doesn't report one
        if load > 1.0 {
            s.xruns.fetch_add(1, Relaxed);
        }
    }
}

impl Engine {
    pub fn devices() -> Vec<String> {
        cpal::default_host()
            .output_devices()
            .map(|ds| ds.map(|d| d.to_string()).collect())
            .unwrap_or_default()
    }

    /// Opens `device` (or the system default) with an optional fixed buffer size.
    pub fn start(device: Option<&str>, buffer: Option<u32>, midi: &MidiSink) -> Result<Self, String> {
        let host = cpal::default_host();
        let named = device.and_then(|name| host.output_devices().ok()?.find(|d| d.to_string() == name));
        let dev = named.or_else(|| host.default_output_device()).ok_or("No audio output found")?;
        let supported = dev.default_output_config().map_err(|e| e.to_string())?;
        let mut config = supported.config();
        if let (Some(n), SupportedBufferSize::Range { min, max }) = (buffer, supported.buffer_size())
            && (*min..=*max).contains(&n)
        {
            config.buffer_size = BufferSize::Fixed(n);
        }
        let format = supported.sample_format();
        let opened = match open(&dev, &config, format) {
            Err(_) if config.buffer_size != BufferSize::Default => {
                config.buffer_size = BufferSize::Default; // device refused the size: fall back
                open(&dev, &config, format)
            }
            r => r,
        }
        .map_err(|e| format!("Can't open {dev}: {e}"))?;
        opened.stream.play().map_err(|e| e.to_string())?;
        *midi.lock().unwrap() = Some(opened.midi_tx);
        Ok(Self {
            _stream: opened.stream,
            tx: opened.tx,
            trash: opened.trash,
            stats: opened.stats,
            effects: Effects::default(),
            device: dev.to_string(),
            sample_rate: config.sample_rate,
        })
    }

    pub fn send(&mut self, msg: Msg) {
        let _ = self.tx.push(msg); // a full queue means the audio thread is stalled; dropping is fine
    }

    /// Swaps in a new song, first creating / retiring the effect instances its devices need.
    pub fn load(&mut self, song: Song) {
        for msg in self.effects.messages(&song, self.sample_rate as f32) {
            self.send(msg);
        }
        self.send(Msg::Song(Arc::new(song)));
    }

    /// Frees what the audio thread has finished with. Call regularly from the UI thread.
    pub fn collect_garbage(&mut self) {
        while self.trash.pop().is_ok() {}
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }
}

struct Opened {
    stream: cpal::Stream,
    tx: rtrb::Producer<Msg>,
    midi_tx: rtrb::Producer<Msg>,
    trash: rtrb::Consumer<Garbage>,
    stats: Arc<Stats>,
}

fn open(dev: &cpal::Device, config: &StreamConfig, format: SampleFormat) -> Result<Opened, String> {
    let (tx, ui) = rtrb::RingBuffer::new(QUEUE);
    let (midi_tx, midi) = rtrb::RingBuffer::new(QUEUE);
    let (trash_tx, trash) = rtrb::RingBuffer::new(QUEUE);
    let stats = Arc::new(Stats::default());
    let sr = config.sample_rate as f32;
    let audio = Audio {
        renderer: Renderer::new(sr),
        ui,
        midi,
        trash: trash_tx,
        stats: stats.clone(),
        channels: config.channels.max(1) as usize,
        sr,
        l: [0.0; BLOCK],
        r: [0.0; BLOCK],
    };
    let stream = match format {
        SampleFormat::F32 => build::<f32>(dev, config, audio, stats.clone()),
        SampleFormat::F64 => build::<f64>(dev, config, audio, stats.clone()),
        SampleFormat::I16 => build::<i16>(dev, config, audio, stats.clone()),
        SampleFormat::I32 => build::<i32>(dev, config, audio, stats.clone()),
        SampleFormat::U16 => build::<u16>(dev, config, audio, stats.clone()),
        other => return Err(format!("unsupported sample format {other}")),
    }
    .map_err(|e| e.to_string())?;
    Ok(Opened { stream, tx, midi_tx, trash, stats })
}

fn build<T: SizedSample + FromSample<f32>>(
    dev: &cpal::Device,
    config: &StreamConfig,
    mut audio: Audio,
    stats: Arc<Stats>,
) -> Result<cpal::Stream, cpal::Error> {
    dev.build_output_stream(
        *config,
        move |data: &mut [T], _| {
            // SAFETY: no_denormals only flips the FPU's flush-to-zero flags for this closure.
            assert_no_alloc::assert_no_alloc(|| unsafe { no_denormals::no_denormals(|| audio.process(data)) })
        },
        move |e: cpal::Error| match e.kind() {
            ErrorKind::Xrun => {
                stats.xruns.fetch_add(1, Relaxed);
            }
            ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied => {} // rerouted / still playing
            _ => stats.failed.store(true, Relaxed),
        },
        None,
    )
}
