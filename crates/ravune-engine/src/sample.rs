//! Samples: decoding (any format symphonia knows), WAV writing, and the sampler (one-shots,
//! and held notes with release and loops for multi-sample instruments).

use std::f64::consts::PI;
use std::io::Cursor;
use std::sync::{Arc, LazyLock};

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;

/// A decoded sample: the sides mixed (what analysis, waveforms and the stretcher's search
/// listen to) and, for a stereo sound, how the sides differ.
pub struct Sample {
    pub frames: Vec<f32>,
    /// (left - right) / 2, so left = frames + side and right = frames - side; empty when mono.
    pub side: Vec<f32>,
    pub rate: f32,
    /// Waveform summary, for drawing without touching every frame.
    pub overview: Vec<Bucket>,
}

/// A slice of a sample's waveform: its peak range and how loud each band is.
#[derive(Clone, Copy, Default)]
pub struct Bucket {
    pub lo: f32,
    pub hi: f32,
    /// Low (< 200 Hz), mid (200–2500 Hz), high (> 2500 Hz), like rekordbox's 3-band view.
    /// Each band is scaled so its 99.5th percentile is 1 (boost capped at 3×), 0..1.
    pub bands: [f32; 3],
}

impl Sample {
    pub fn new(frames: Vec<f32>, rate: f32) -> Self {
        Self::stereo(frames, Vec::new(), rate)
    }

    /// `side` as long as `frames`, or empty for mono.
    pub fn stereo(frames: Vec<f32>, side: Vec<f32>, rate: f32) -> Self {
        debug_assert!(side.is_empty() || side.len() == frames.len());
        let overview = overview(&frames, rate);
        Self { frames, side, rate, overview }
    }

    pub fn seconds(&self) -> f32 {
        self.frames.len() as f32 / self.rate
    }

    pub fn decode_file(path: &std::path::Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let ext = path.extension().and_then(|e| e.to_str());
        decode(Box::new(file), ext).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn decode_bytes(bytes: &'static [u8], ext: Option<&str>) -> Result<Self, String> {
        decode(Box::new(Cursor::new(bytes)), ext)
    }
}

fn overview(frames: &[f32], rate: f32) -> Vec<Bucket> {
    // ~2–4k buckets: fine for one-shots, still small for whole tracks
    let size = (frames.len() / 2048).next_power_of_two().clamp(32, 512);
    let coeff = |hz: f32| 1.0 - (-std::f32::consts::TAU * hz / rate).exp();
    let (a_low, a_mid) = (coeff(200.0), coeff(2500.0));
    // each band filtered on its own (2-pole low / band / high-pass from one-pole stages):
    // subtracting low-passes instead would leak a bass line into the mids through phase
    fn lp(state: &mut f32, a: f32, x: f32) -> f32 {
        *state += a * (x - *state);
        *state
    }
    let mut s = [0.0f32; 8];
    let mut out: Vec<Bucket> = frames
        .chunks(size)
        .map(|chunk| {
            let mut b = Bucket::default();
            for &x in chunk {
                let [s0, s1, s2, s3, s4, s5, s6, s7] = &mut s;
                let low = lp(s1, a_low, lp(s0, a_low, x));
                let h = x - lp(s2, a_mid, x);
                let high = h - lp(s3, a_mid, h);
                let m = lp(s5, a_mid, lp(s4, a_mid, x));
                let m = m - lp(s6, a_low, m);
                let mid = m - lp(s7, a_low, m);
                b.lo = b.lo.min(x);
                b.hi = b.hi.max(x);
                for (band, v) in b.bands.iter_mut().zip([low, mid, high]) {
                    *band = band.max(v.abs());
                }
            }
            b
        })
        .collect();
    if out.is_empty() {
        return out;
    }
    let tops: [f32; 3] = std::array::from_fn(|k| {
        let mut levels: Vec<f32> = out.iter().map(|b| b.bands[k]).collect();
        let i = ((levels.len() - 1) as f32 * 0.995) as usize;
        *levels.select_nth_unstable_by(i, f32::total_cmp).1
    });
    let loudest = tops.iter().copied().fold(0.0, f32::max);
    for (k, top) in tops.into_iter().enumerate() {
        let top = top.max(loudest / 3.0).max(1e-9); // a kick's highs stay small, not blown up
        out.iter_mut().for_each(|b| b.bands[k] = (b.bands[k] / top).min(1.0));
    }
    out
}

fn decode(source: Box<dyn MediaSource>, ext: Option<&str>) -> Result<Sample, String> {
    let mut hint = Hint::new();
    if let Some(ext) = ext {
        hint.with_extension(ext);
    }
    let stream = MediaSourceStream::new(source, Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(&hint, stream, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| format!("unsupported file ({e})"))?;
    let track = format.default_track(TrackType::Audio).ok_or("no audio track")?;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or("no audio parameters")?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| format!("unsupported codec ({e})"))?;
    let track_id = track.id;

    let (mut mono, mut side, mut interleaved, mut rate) = (Vec::new(), Vec::new(), Vec::new(), 0);
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                rate = buf.spec().rate();
                let channels = buf.spec().channels().count().max(1);
                interleaved.resize(buf.samples_interleaved(), 0.0f32);
                buf.copy_to_slice_interleaved(&mut interleaved);
                mono.extend(interleaved.chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32));
                if channels >= 2 {
                    side.extend(interleaved.chunks(channels).map(|f| (f[0] - f[1]) / 2.0));
                }
            }
            Err(Error::DecodeError(_)) => {} // skip a corrupt packet, keep the rest
            Err(e) => return Err(e.to_string()),
        }
    }
    if mono.is_empty() || rate == 0 {
        return Err("file contains no audio".into());
    }
    // the same sound on both sides is mono
    if side.len() != mono.len() || side.iter().all(|s| s.abs() < 1e-4) {
        side = Vec::new();
    }
    Ok(Sample::stereo(mono, side, rate as f32))
}

/// Writes 16-bit PCM WAV from interleaved `samples`. Used by export and the demo-kit generator.
pub fn write_wav(path: &std::path::Path, samples: &[f32], channels: u16, rate: u32) -> std::io::Result<()> {
    write_wav_as(path, samples, channels, rate, 16)
}

/// A WAV of interleaved `samples`: 16 or 24-bit PCM, or (`bits` 32) float.
pub fn write_wav_as(path: &std::path::Path, samples: &[f32], channels: u16, rate: u32, bits: u16) -> std::io::Result<()> {
    let float = bits == 32;
    let bytes = bits as u32 / 8;
    let data_len = samples.len() as u32 * bytes;
    let align = channels as u32 * bytes;
    // float has the longer fmt chunk (cbSize) and a fact chunk with the frame count
    let (fmt_len, fact_len) = if float { (18u32, 12u32) } else { (16, 0) };
    let mut out = Vec::with_capacity(64 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(4 + 8 + fmt_len + fact_len + 8 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    // fmt chunk: size, PCM (1) or float (3), channels, rate, byte rate, block align, bits
    for (v, n) in [(fmt_len, 4), (if float { 3 } else { 1 }, 2), (channels as u32, 2), (rate, 4), (rate * align, 4), (align, 2), (bits as u32, 2)] {
        out.extend_from_slice(&v.to_le_bytes()[..n]);
    }
    if float {
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(b"fact");
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&(samples.len() as u32 / channels as u32).to_le_bytes());
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    // 16-bit gets TPDF dither (±1 LSB): quiet fades turn into soft hiss instead of grainy steps
    let mut seed = 0x9E37_79B9u32;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    for &x in samples {
        match bits {
            16 => {
                let dither = noise() - noise();
                out.extend_from_slice(&((x.clamp(-1.0, 1.0) * 32767.0 + dither).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes())
            }
            24 => out.extend_from_slice(&((x.clamp(-1.0, 1.0) * 8_388_607.0) as i32).to_le_bytes()[..3]),
            _ => out.extend_from_slice(&x.to_le_bytes()),
        }
    }
    std::fs::write(path, out)
}

// ---------------------------------------------------------------------------
// Sampler: polyphonic one-shot player (runs on the audio thread, never allocates)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Reading between frames: a Kaiser-windowed sinc over the 8 frames around (3 before, 4
// after), from a table of 1024 fractions. A straight line between two frames folds a
// 44.1 kHz sound's highs back down at 48 kHz only 17 dB under them (grit on every hat);
// this keeps them over 50 dB under. On a whole frame it reads that frame exactly.
// ---------------------------------------------------------------------------

const TAPS: usize = 8;
const PHASES: usize = 1024;

static KERNEL: LazyLock<[[f32; TAPS]; PHASES + 1]> = LazyLock::new(|| {
    // I0, by its series
    let bessel = |x: f64| {
        let (mut sum, mut term) = (1.0, 1.0);
        for k in 1..32 {
            term *= (x / (2.0 * k as f64)).powi(2);
            sum += term;
        }
        sum
    };
    const BETA: f64 = 5.0;
    std::array::from_fn(|p| {
        let t = p as f64 / PHASES as f64;
        let row: [f64; TAPS] = std::array::from_fn(|k| {
            let d = k as f64 - 3.0 - t;
            let sinc = if d == 0.0 { 1.0 } else if t.fract() == 0.0 { 0.0 } else { (PI * d).sin() / (PI * d) };
            sinc * bessel(BETA * (1.0 - (d / 4.0).powi(2)).max(0.0).sqrt()) / bessel(BETA)
        });
        let sum: f64 = row.iter().sum();
        row.map(|h| (h / sum) as f32)
    })
});

/// Builds the table now, so the audio thread never does.
pub(crate) fn warm_up() {
    LazyLock::force(&KERNEL);
}

/// Where to read `t` (0..1) past a frame: the frames around it (`frame(k)`: where the one `k`
/// away is) and their weights.
pub(crate) fn taps(t: f32, frame: impl Fn(isize) -> usize) -> ([usize; TAPS], &'static [f32; TAPS]) {
    (std::array::from_fn(|k| frame(k as isize - 3)), &KERNEL[(t.clamp(0.0, 1.0) * PHASES as f32).round() as usize])
}

pub(crate) fn read(x: &[f32], (at, w): &([usize; TAPS], &[f32; TAPS])) -> f32 {
    at.iter().zip(w.iter()).map(|(i, w)| x[*i] * w).sum()
}

const VOICES: usize = 32;
/// Fade length when a voice is choked or stolen, avoids clicks.
const FADE_SAMPLES: f32 = 96.0;

#[derive(Default)]
struct Voice {
    sample: Option<Arc<Sample>>,
    pos: f64,
    step: f64,
    gain: f32,
    /// Per-sample gain decrement while fading out (0 = not fading).
    fade: f32,
    age: u64,
    /// Who started it (playlist clip index), so it can be stopped; `UNTAGGED` otherwise.
    tag: u32,
    /// The key holding it (multi-sample notes), released by [`Sampler::release`].
    key: u8,
    held: bool,
    /// Note-off fade, in samples.
    release: f32,
    looped: Option<(f64, f64)>,
    /// Fade-in: how far in (0..1) and how much further each sample.
    rise: f32,
    rise_step: f32,
    /// The frame it stops at (a sampler channel's cut end), fading out just before.
    end: f64,
}

const UNTAGGED: u32 = u32::MAX;

/// How a multi-sample note plays: held until its key is released, then fades.
#[derive(Clone, Copy, Debug)]
pub struct VoiceMode {
    pub key: u8,
    /// Seconds of fade after note-off.
    pub release: f32,
    /// Ignores note-off.
    pub one_shot: bool,
    /// Loop points in frames.
    pub looped: Option<(usize, usize)>,
    /// Start frame.
    pub offset: usize,
}

#[derive(Default)]
pub struct Sampler {
    voices: [Voice; VOICES],
    clock: u64,
}

impl Sampler {
    /// Starts `sample` at `gain`, transposed by `semitones`. The caller must keep another
    /// `Arc` to the sample alive so this voice never frees memory on the audio thread.
    pub fn trigger(&mut self, sample: Arc<Sample>, gain: f32, semitones: f32, out_rate: f32) {
        self.trigger_at(sample, gain, semitones, out_rate, 0.0, UNTAGGED, false);
    }

    /// Like [`Self::trigger`], starting `start_frame` frames into the sample, tagged for
    /// [`Self::stop_tag`]; `looped`, the sound goes round end to start until stopped.
    #[allow(clippy::too_many_arguments)]
    pub fn trigger_at(&mut self, sample: Arc<Sample>, gain: f32, semitones: f32, out_rate: f32, start_frame: f64, tag: u32, looped: bool) {
        let frames = sample.frames.len() as f64;
        let v = self.start(sample, gain, semitones, out_rate);
        v.pos = start_frame.max(0.0);
        if looped && frames > 1.0 {
            (v.pos, v.looped) = (v.pos % frames, Some((0.0, frames)));
        }
        v.tag = tag;
    }

    /// A multi-sample note: plays until [`Self::release`] of its key (unless one-shot).
    pub fn play(&mut self, sample: Arc<Sample>, gain: f32, semitones: f32, out_rate: f32, mode: VoiceMode) {
        let v = self.start(sample, gain, semitones, out_rate);
        v.pos = mode.offset as f64;
        (v.key, v.held) = (mode.key, !mode.one_shot);
        v.release = (mode.release * out_rate).max(FADE_SAMPLES);
        v.looped = mode.looped.map(|(a, b)| (a as f64, b as f64));
    }

    /// A sampler channel's hit with an envelope: `attack` seconds in, and with a `release`
    /// held until [`Self::release`] of its key, then fading over it. It plays the part of
    /// the sound from `start` to `end` (fractions of its length), over and over while held
    /// when `looped`.
    #[allow(clippy::too_many_arguments)]
    pub fn hit(&mut self, sample: Arc<Sample>, gain: f32, semitones: f32, out_rate: f32, key: u8, (attack, release): (f32, f32), (start, end, looped): (f32, f32, bool)) {
        let frames = sample.frames.len() as f64;
        let v = self.start(sample, gain, semitones, out_rate);
        let (a, b) = (start.clamp(0.0, 1.0) as f64 * frames, end.clamp(0.0, 1.0) as f64 * frames);
        v.pos = a;
        if looped && b > a + 1.0 {
            // round and round until its key lets go, then the release (a short fade at least)
            v.looped = Some((a, b));
            (v.key, v.held, v.release) = (key, true, (release * out_rate).max(FADE_SAMPLES));
        } else if end < 1.0 {
            v.end = b;
        }
        if attack > 0.0 {
            (v.rise, v.rise_step) = (0.0, 1.0 / (attack * out_rate).max(1.0));
        }
        if release > 0.0 && v.looped.is_none() {
            (v.key, v.held, v.release) = (key, true, (release * out_rate).max(FADE_SAMPLES));
        }
    }

    /// Fades out the held notes of `key`.
    pub fn release(&mut self, key: u8) {
        for v in self.voices.iter_mut().filter(|v| v.sample.is_some() && v.held && v.key == key) {
            v.held = false;
            if v.fade == 0.0 {
                v.fade = v.gain / v.release;
            }
        }
    }

    /// Takes the free (or oldest) voice and starts `sample` on it from the top.
    fn start(&mut self, sample: Arc<Sample>, gain: f32, semitones: f32, out_rate: f32) -> &mut Voice {
        self.clock += 1;
        let i = self
            .voices
            .iter()
            .position(|v| v.sample.is_none())
            .unwrap_or_else(|| (0..VOICES).min_by_key(|&i| self.voices[i].age).unwrap());
        let v = &mut self.voices[i];
        v.step = (sample.rate / out_rate) as f64 * ((semitones / 12.0).exp2() as f64);
        v.sample = Some(sample);
        v.pos = 0.0;
        v.gain = gain;
        v.fade = 0.0;
        v.age = self.clock;
        (v.tag, v.key, v.held, v.looped) = (UNTAGGED, 0, false, None);
        (v.rise, v.rise_step) = (1.0, 0.0);
        v.end = f64::MAX;
        v
    }

    /// Fades out the voices a given clip started.
    pub fn stop_tag(&mut self, tag: u32) {
        for v in self.voices.iter_mut().filter(|v| v.sample.is_some() && v.tag == tag && v.fade == 0.0) {
            v.fade = v.gain / FADE_SAMPLES;
        }
    }

    /// Fades out everything that's playing (choke groups, stop).
    pub fn choke(&mut self) {
        for v in self.voices.iter_mut().filter(|v| v.sample.is_some() && v.fade == 0.0) {
            v.fade = v.gain / FADE_SAMPLES;
        }
    }

    pub fn is_playing(&self) -> bool {
        self.voices.iter().any(|v| v.sample.is_some())
    }

    /// Adds the sampler's output to `l` and `r` (the same length).
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        for v in self.voices.iter_mut() {
            let Some(sample) = &v.sample else { continue };
            let (frames, side) = (&sample.frames, &sample.side);
            let mut done = false;
            for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                if let Some((a, b)) = v.looped
                    && v.pos >= b
                {
                    v.pos -= b - a;
                }
                let i = v.pos as usize;
                // at a loop's seam the next frame is the loop start
                let next = match v.looped {
                    Some((a, b)) if i + 1 >= b as usize => a as usize,
                    _ => i + 1,
                };
                if next >= frames.len() || v.gain <= 0.0 || v.pos >= v.end {
                    done = true;
                    break;
                }
                // a cut end fades over its last few ms, so it doesn't click
                if v.fade == 0.0 && v.end - v.pos <= FADE_SAMPLES as f64 * v.step {
                    v.fade = v.gain / FADE_SAMPLES;
                }
                // the frames around: round the loop's seam, held at either end
                let (last, looped) = (frames.len() as isize - 1, v.looped);
                let tap = taps((v.pos - i as f64) as f32, |k| {
                    let j = i as isize + k;
                    match looped {
                        Some((a, b)) if j >= b as isize => (a as isize + j - b as isize).clamp(0, last) as usize,
                        _ => j.clamp(0, last) as usize,
                    }
                });
                let (m, s) = (read(frames, &tap), if side.is_empty() { 0.0 } else { read(side, &tap) });
                let g = v.gain * v.rise;
                *a += (m + s) * g;
                *b += (m - s) * g;
                v.pos += v.step;
                v.gain -= v.fade;
                v.rise = (v.rise + v.rise_step).min(1.0);
            }
            if done {
                v.sample = None; // the caller holds another Arc, so this never deallocates
            }
        }
    }
}
