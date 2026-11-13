//! What a render sounds like, in numbers an agent (or a test) can read: level, balance across
//! the spectrum, stereo width, onsets, key and tempo. Time-domain only: biquads for the
//! bands, Goertzel for the pitch classes.

use std::f32::consts::TAU;

use crate::{Sample, detect_bpm};

/// Bands of [`Analysis::bands`]: sub, low, mid, high, air.
pub const BANDS: [(&str, f32, f32); 5] = [("sub", 20.0, 60.0), ("low", 60.0, 250.0), ("mid", 250.0, 2000.0), ("high", 2000.0, 8000.0), ("air", 8000.0, 20000.0)];
pub const KEYS: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub seconds: f32,
    pub peak_db: f32,
    pub rms_db: f32,
    /// Each band's share of the energy, dB (0 = all of it), in [`BANDS`] order.
    pub bands: [f32; 5],
    /// Side energy over mid energy, dB: very low is mono, around 0 is very wide.
    pub width_db: f32,
    /// When something hits, in seconds.
    pub onsets: Vec<f32>,
    /// The best-fitting key: pitch class (0 = C), minor, and how well it fits (-1..1).
    pub key: (u8, bool, f32),
    pub bpm: Option<f32>,
    /// Integrated loudness, LUFS (EBU R128 / BS.1770-4); -99 for silence.
    pub lufs: f32,
    /// True peak, dBTP (4× oversampled), what a streaming service's encoder will meet.
    pub true_peak_db: f32,
}

impl Analysis {
    pub fn key_name(&self) -> String {
        format!("{} {}", KEYS[self.key.0 as usize % 12], if self.key.1 { "minor" } else { "major" })
    }
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

/// RBJ biquad, low- or high-pass.
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Biquad {
    fn new(high: bool, freq: f32, rate: f32) -> Self {
        let w = TAU * (freq / rate).min(0.49);
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let b = if high { [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0] } else { [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0] };
        Self { b: b.map(|v| v / a0), a: [-2.0 * cos / a0, (1.0 - alpha) / a0], z: [0.0; 2] }
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// Analyzes interleaved stereo `frames` at `rate`.
pub fn analyze(frames: &[f32], rate: f32) -> Analysis {
    let n = frames.len() / 2;
    let mono: Vec<f32> = frames.as_chunks::<2>().0.iter().map(|[l, r]| (l + r) * 0.5).collect();
    let peak = frames.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let energy = |x: &[f32]| x.iter().map(|s| s * s).sum::<f32>();
    let total = energy(frames) / frames.len().max(1) as f32;
    let side = frames.as_chunks::<2>().0.iter().map(|[l, r]| ((l - r) * 0.5).powi(2)).sum::<f32>();

    // each band: a high-pass at its bottom, a low-pass at its top (two poles each)
    let mono_energy = energy(&mono).max(1e-12);
    let bands = BANDS.map(|(_, lo, hi)| {
        let (mut h, mut l) = (Biquad::new(true, lo, rate), Biquad::new(false, hi.min(rate * 0.45), rate));
        let e: f32 = mono.iter().map(|&x| l.run(h.run(x)).powi(2)).sum();
        10.0 * (e / mono_energy).max(1e-12).log10()
    });

    // onsets: the level every 10 ms jumping well above the last 100 ms
    let hop = ((rate / 100.0) as usize).max(1);
    let env: Vec<f32> = mono.chunks(hop).map(|c| (energy(c) / c.len() as f32).sqrt()).collect();
    let mut onsets = Vec::new();
    let mut last = -1.0f32;
    for i in 1..env.len() {
        let from = i.saturating_sub(10);
        let recent = env[from..i].iter().sum::<f32>() / (i - from) as f32;
        let t = i as f32 * hop as f32 / rate;
        if env[i] > 1.6 * recent + 1e-4 && env[i] > env[i - 1] && db(env[i]) > -45.0 && t - last > 0.05 {
            onsets.push(t);
            last = t;
        }
    }

    Analysis {
        seconds: n as f32 / rate,
        peak_db: db(peak),
        rms_db: db(total.sqrt()),
        bands,
        width_db: 10.0 * (side / mono_energy).max(1e-12).log10(),
        onsets,
        key: key(&mono, rate),
        bpm: detect_bpm(&Sample::new(mono, rate), ""),
        lufs: lufs(frames, rate),
        true_peak_db: db(true_peak(frames)),
    }
}

/// A biquad in f64 from its normalized coefficients (`a` without the leading 1).
struct Iir {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Iir {
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// BS.1770's K-weighting at any rate: the head's high shelf, then the RLB high-pass
/// (the coefficients as libebur128 derives them).
fn k_weighting(rate: f64) -> [Iir; 2] {
    use std::f64::consts::PI;
    let (f0, gain, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = (PI * f0 / rate).tan();
    let vh = 10f64.powf(gain / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Iir { b: [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0], a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0], z: [0.0; 2] };
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = (PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let high_pass = Iir { b: [1.0, -2.0, 1.0], a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0], z: [0.0; 2] };
    [shelf, high_pass]
}

/// Integrated loudness of interleaved stereo `frames`, LUFS: K-weighted 400 ms blocks every
/// 100 ms, gated at -70 LUFS and then 10 LU under the gated mean. -99 when nothing passes.
pub fn lufs(frames: &[f32], rate: f32) -> f32 {
    let mut filters = [k_weighting(rate as f64), k_weighting(rate as f64)];
    let hop = (rate as usize / 10).max(1);
    // the K-weighted energy of each 100 ms step, both channels summed
    let steps: Vec<f64> = frames
        .chunks(2 * hop)
        .map(|c| {
            c.as_chunks::<2>().0.iter().map(|f| (0..2).map(|ch| filters[ch].iter_mut().fold(f[ch] as f64, |x, b| b.run(x)).powi(2)).sum::<f64>()).sum::<f64>() / hop as f64
        })
        .collect();
    let blocks: Vec<f64> = steps.windows(4).map(|w| w.iter().sum::<f64>() / 4.0).collect();
    let loudness = |z: f64| -0.691 + 10.0 * z.max(1e-30).log10();
    let mean_above = |gate: f64| {
        let kept: Vec<f64> = blocks.iter().copied().filter(|&z| loudness(z) > gate).collect();
        (!kept.is_empty()).then(|| kept.iter().sum::<f64>() / kept.len() as f64)
    };
    let Some(absolute) = mean_above(-70.0) else { return -99.0 };
    mean_above(loudness(absolute) - 10.0).map_or(-99.0, |z| loudness(z).max(-99.0) as f32)
}

/// The highest peak between the samples too: each channel 4× oversampled through a
/// Hann-windowed sinc (16 taps a phase), as BS.1770's true-peak meter.
pub fn true_peak(frames: &[f32]) -> f32 {
    const SIDE: i32 = 8;
    let sinc = |t: f32| if t.abs() < 1e-6 { 1.0 } else { (std::f32::consts::PI * t).sin() / (std::f32::consts::PI * t) };
    let hann = |t: f32| 0.5 + 0.5 * (std::f32::consts::PI * t / SIDE as f32).cos();
    // the taps for the points a quarter, a half and three quarters past each sample
    let phases: Vec<Vec<f32>> = (1..4).map(|p| (-SIDE + 1..=SIDE).map(|k| { let t = k as f32 - p as f32 / 4.0; sinc(t) * hann(t) }).collect()).collect();
    let n = frames.len() / 2;
    let mut peak = frames.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    for ch in 0..2 {
        let x: Vec<f32> = (0..n).map(|i| frames[2 * i + ch]).collect();
        for i in (SIDE - 1) as usize..n.saturating_sub(SIDE as usize) {
            let window = &x[i + 1 - SIDE as usize..=i + SIDE as usize];
            for taps in &phases {
                peak = peak.max(window.iter().zip(taps).map(|(a, b)| a * b).sum::<f32>().abs());
            }
        }
    }
    peak
}

/// Krumhansl–Schmuckler: the pitch-class profile (C2–B5, by Goertzel on a 4× decimated
/// signal) against the major and minor key profiles, every root.
fn key(mono: &[f32], rate: f32) -> (u8, bool, f32) {
    const MAJOR: [f32; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
    const MINOR: [f32; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
    let mut lp = Biquad::new(false, rate / 10.0, rate);
    let small: Vec<f32> = mono.iter().map(|&x| lp.run(x)).step_by(4).collect();
    let sr = rate / 4.0;
    let mut chroma = [0.0f32; 12];
    for note in 36..84u32 {
        let f = 440.0 * ((note as f32 - 69.0) / 12.0).exp2();
        let k = 2.0 * (TAU * f / sr).cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &x in &small {
            let s0 = x + k * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        chroma[(note % 12) as usize] += (s1 * s1 + s2 * s2 - k * s1 * s2).max(0.0).sqrt();
    }
    let corr = |profile: &[f32; 12], root: usize| {
        let p: Vec<f32> = (0..12).map(|i| profile[(i + 12 - root) % 12]).collect();
        let (mx, my) = (chroma.iter().sum::<f32>() / 12.0, p.iter().sum::<f32>() / 12.0);
        let num: f32 = (0..12).map(|i| (chroma[i] - mx) * (p[i] - my)).sum();
        let den = ((0..12).map(|i| (chroma[i] - mx).powi(2)).sum::<f32>() * (0..12).map(|i| (p[i] - my).powi(2)).sum::<f32>()).sqrt();
        if den > 0.0 { num / den } else { 0.0 }
    };
    let mut best = (0u8, false, f32::MIN);
    for root in 0..12 {
        for (minor, profile) in [(false, &MAJOR), (true, &MINOR)] {
            let c = corr(profile, root);
            if c > best.2 {
                best = (root as u8, minor, c);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1 kHz sine at -20 dBFS in both channels reads -20 LUFS (BS.1770's calibration);
    /// silence passes no gate.
    #[test]
    fn measures_loudness() {
        for rate in [44_100.0, 48_000.0] {
            let sine: Vec<f32> = (0..(rate * 5.0) as usize).flat_map(|i| [0.1 * (TAU * 1000.0 * i as f32 / rate).sin(); 2]).collect();
            let l = lufs(&sine, rate);
            assert!((l + 20.0).abs() < 0.2, "{rate}: {l} LUFS");
        }
        assert_eq!(lufs(&vec![0.0; 88_200], 44_100.0), -99.0);
    }

    /// A sine at a quarter of the rate, sampled 45° off its crests: the samples peak at
    /// 0.707, the wave itself at 1.
    #[test]
    fn finds_peaks_between_samples() {
        let frames: Vec<f32> = (0..4000).flat_map(|i| [(std::f32::consts::FRAC_PI_2 * i as f32 + std::f32::consts::FRAC_PI_4).sin(); 2]).collect();
        let sample_peak = frames.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((sample_peak - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        let tp = true_peak(&frames);
        assert!((tp - 1.0).abs() < 0.02, "true peak {tp}");
    }

    /// An A minor arpeggio of plucks, two a second, the right side a little quieter.
    #[test]
    fn hears_key_hits_and_balance() {
        let rate = 22_050.0;
        let mut frames = vec![0.0f32; (rate * 8.0) as usize * 2];
        for (k, key) in [57, 60, 64, 69, 57, 60, 64, 72].iter().cycle().take(16).enumerate() {
            let f = 440.0 * ((*key as f32 - 69.0) / 12.0).exp2();
            let start = (k as f32 * 0.5 * rate) as usize;
            for i in 0..(0.4 * rate) as usize {
                let t = i as f32 / rate;
                let s = (TAU * f * t).sin() * (-t * 6.0).exp() * 0.5;
                frames[2 * (start + i)] += s;
                frames[2 * (start + i) + 1] += s * 0.8;
            }
        }
        let a = analyze(&frames, rate);
        assert_eq!(a.key_name(), "A minor", "{a:?}");
        assert!((14..=18).contains(&a.onsets.len()), "one onset per pluck: {:?}", a.onsets);
        assert!(a.bands[2] > a.bands[0] && a.bands[2] > a.bands[4], "notes in the mids: {:?}", a.bands);
        assert!(a.width_db < -15.0, "nearly mono: {}", a.width_db);
        assert!((a.peak_db - -6.0).abs() < 1.0 && a.rms_db < a.peak_db);
    }
}
