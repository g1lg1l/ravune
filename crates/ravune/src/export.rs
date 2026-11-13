//! Audio export: renders the playlist offline on a worker thread, then writes WAV, FLAC,
//! Ogg Vorbis or (through ffmpeg, when it's installed) MP3.

use std::io::Write;
use std::num::{NonZeroU8, NonZeroU32};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, mpsc};

use ravune_engine::{Mode, Song, Transport, lufs, render_offline, true_peak, write_wav_as};
use serde::{Deserialize, Serialize};

use crate::App;
use crate::shell::Tone;

const RATE: u32 = 44_100;
/// Lets tails (releases, reverb, echoes) ring out after the last bar.
const TAIL_SECONDS: f32 = 2.0;

pub struct Export {
    progress: Arc<AtomicU32>,
    done: mpsc::Receiver<Result<Exported, String>>,
}

/// Where an export went, and the song's loudness (LUFS, dBTP) when it's one file.
type Exported = (PathBuf, Option<(f32, f32)>);

/// What File → Export writes (the last one picked is what the export key repeats).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    #[default]
    Wav16,
    Wav24,
    Wav32,
    Flac,
    Ogg,
    Mp3,
}

impl Format {
    pub const ALL: [Format; 6] = [Format::Wav16, Format::Wav24, Format::Wav32, Format::Flac, Format::Ogg, Format::Mp3];

    pub fn label(self) -> &'static str {
        match self {
            Format::Wav16 => tr!("WAV 16-bit"),
            Format::Wav24 => tr!("WAV 24-bit"),
            Format::Wav32 => tr!("WAV 32-bit float"),
            Format::Flac => tr!("FLAC 24-bit"),
            Format::Ogg => tr!("Ogg Vorbis"),
            Format::Mp3 => tr!("MP3 320 kbps"),
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Format::Wav16 => tr!("Uncompressed, CD quality: plays everywhere"),
            Format::Wav24 | Format::Wav32 => tr!("Uncompressed, full detail: for mastering or another DAW"),
            Format::Flac => tr!("Lossless, about half the size of a WAV"),
            Format::Ogg => tr!("Lossy and small (about 224 kbps): games and the web"),
            Format::Mp3 => tr!("Lossy, plays everywhere. Needs ffmpeg installed (ffmpeg.org)"),
        }
    }

    fn ext(self) -> &'static str {
        match self {
            Format::Wav16 | Format::Wav24 | Format::Wav32 => "wav",
            Format::Flac => "flac",
            Format::Ogg => "ogg",
            Format::Mp3 => "mp3",
        }
    }
}

/// A copy of the project per mixer track (the master aside) with only that track soloed:
/// (track, its name, the project).
fn stem_projects(project: &ravune_core::Project) -> Vec<(usize, String, ravune_core::Project)> {
    (1..project.mixer.len())
        .map(|i| {
            let mut p = project.clone();
            p.mixer.iter_mut().enumerate().for_each(|(k, t)| t.solo = k == i);
            (i, project.mixer[i].name.clone(), p)
        })
        .collect()
}

/// Writes interleaved stereo `frames` as `format`.
fn encode(format: Format, path: &Path, frames: &[f32], rate: u32) -> Result<(), String> {
    let wav = |bits| write_wav_as(path, frames, 2, rate, bits).map_err(|e| e.to_string());
    match format {
        Format::Wav16 => wav(16),
        Format::Wav24 => wav(24),
        Format::Wav32 => wav(32),
        Format::Flac => flac(path, frames, rate),
        Format::Ogg => ogg(path, frames, rate),
        Format::Mp3 => mp3(path, frames, rate),
    }
}

fn flac(path: &Path, frames: &[f32], rate: u32) -> Result<(), String> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let config = flacenc::config::Encoder::default().into_verified().map_err(|(_, e)| e.to_string())?;
    // whole blocks only (a few ms of silence more): symphonia, Ravune's own decoder, won't
    // open a stream whose last block is short
    let mut ints: Vec<i32> = frames.iter().map(|x| (x.clamp(-1.0, 1.0) * 8_388_607.0) as i32).collect();
    ints.resize(ints.len().div_ceil(2 * config.block_size) * 2 * config.block_size, 0);
    let source = flacenc::source::MemSource::from_samples(&ints, 2, 24, rate as usize);
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size).map_err(|e| e.to_string())?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|e| e.to_string())?;
    std::fs::write(path, sink.as_slice()).map_err(|e| e.to_string())
}

fn ogg(path: &Path, frames: &[f32], rate: u32) -> Result<(), String> {
    use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};
    let file = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| e.to_string())?);
    // the stream's serial only has to differ between streams chained in one file
    let serial = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos() as i32);
    let (rate, channels) = (NonZeroU32::new(rate).ok_or("rate 0")?, NonZeroU8::new(2).unwrap());
    let mut encoder = VorbisEncoderBuilder::new_with_serial(rate, channels, file, serial)
        .bitrate_management_strategy(VorbisBitrateManagementStrategy::QualityVbr { target_quality: 0.7 })
        .build()
        .map_err(|e| e.to_string())?;
    for block in frames.chunks(2 * 4096) {
        let (l, r): (Vec<f32>, Vec<f32>) = block.chunks(2).map(|f| (f[0], f[1])).unzip();
        encoder.encode_audio_block([l, r]).map_err(|e| e.to_string())?;
    }
    encoder.finish().map_err(|e| e.to_string())?.flush().map_err(|e| e.to_string())
}

/// ffmpeg from the PATH, without a console window flashing up on Windows.
fn ffmpeg() -> Command {
    #[cfg_attr(not(windows), allow(unused_mut))] // only Windows changes it
    let mut command = Command::new("ffmpeg");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

fn has_ffmpeg() -> bool {
    ffmpeg().arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// MP3 through ffmpeg's LAME (LAME is LGPL, so Ravune doesn't link it): raw floats in on stdin.
fn mp3(path: &Path, frames: &[f32], rate: u32) -> Result<(), String> {
    let rate = rate.to_string();
    let mut child = ffmpeg()
        .args(["-y", "-loglevel", "error", "-f", "f32le", "-ar", &rate, "-ac", "2", "-i", "-", "-codec:a", "libmp3lame", "-b:a", "320k"])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| tr!("MP3 needs ffmpeg installed: {}", e))?;
    let pcm: Vec<u8> = frames.iter().flat_map(|x| x.to_le_bytes()).collect();
    let fed = child.stdin.take().map(|mut stdin| stdin.write_all(&pcm));
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    fed.unwrap_or(Ok(())).map_err(|e| e.to_string())
}

impl App {
    /// Exports as `format`, which becomes the one the export key repeats.
    pub fn export_as(&mut self, format: Format) {
        self.settings.export_format = format;
        self.export_dialog();
    }

    pub fn export_dialog(&mut self) {
        if self.export.is_some() {
            return;
        }
        if self.session.project.clips.is_empty() {
            self.say_as(Tone::Error, tr!("The playlist is empty: place some patterns first (export renders the song)."));
            return;
        }
        let format = self.settings.export_format;
        if format == Format::Mp3 && !has_ffmpeg() {
            self.say_as(Tone::Error, tr!("MP3 needs ffmpeg installed (ffmpeg.org): pick another format, or install it and try again"));
            return;
        }
        let ext = format.ext();
        let dialog = rfd::FileDialog::new().add_filter(format.label(), &[ext]).set_file_name(format!("{}.{ext}", self.file_name()));
        let Some(path) = dialog.save_file() else { return };
        let library = &mut self.library;
        let transport = Transport { playing: true, mode: Mode::Song, start: 0, jump: 0, looped: None };
        let song = Song::compile(&self.session.project, transport, |p| library.get(p).ok());
        let (progress, (tx, done)) = (Arc::new(AtomicU32::new(0)), mpsc::channel());
        let shared = progress.clone();
        std::thread::spawn(move || {
            let frames = render_offline(song, RATE as f32, TAIL_SECONDS, |p| shared.store(p.to_bits(), Relaxed));
            let loudness = (lufs(&frames, RATE as f32), 20.0 * true_peak(&frames).max(1e-9).log10());
            let _ = tx.send(encode(format, &path, &frames, RATE).map(|_| (path, Some(loudness))));
        });
        self.export = Some(Export { progress, done });
    }

    /// Stems: every mixer track rendered on its own (soloed, so its buses and its reverb and
    /// delay returns come along), one file each in the format picked, into a folder.
    pub fn export_stems(&mut self) {
        if self.export.is_some() {
            return;
        }
        if self.session.project.clips.is_empty() {
            self.say_as(Tone::Error, tr!("The playlist is empty: place some patterns first (export renders the song)."));
            return;
        }
        let format = self.settings.export_format;
        if format == Format::Mp3 && !has_ffmpeg() {
            self.say_as(Tone::Error, tr!("MP3 needs ffmpeg installed (ffmpeg.org): pick another format, or install it and try again"));
            return;
        }
        let Some(folder) = rfd::FileDialog::new().set_title(tr!("A folder for the stems")).pick_folder() else { return };
        let song_name = self.file_name();
        let transport = Transport { playing: true, mode: Mode::Song, start: 0, jump: 0, looped: None };
        let library = &mut self.library;
        let stems: Vec<(PathBuf, Song)> = stem_projects(&self.session.project)
            .into_iter()
            .map(|(i, name, project)| {
                let file = folder.join(format!("{song_name} - {i:02} {}.{}", name.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_"), format.ext()));
                (file, Song::compile(&project, transport, |p| library.get(p).ok()))
            })
            .collect();
        let (progress, (tx, done)) = (Arc::new(AtomicU32::new(0)), mpsc::channel());
        let shared = progress.clone();
        std::thread::spawn(move || {
            let count = stems.len().max(1) as f32;
            let mut written = 0;
            for (k, (file, song)) in stems.into_iter().enumerate() {
                let frames = render_offline(song, RATE as f32, TAIL_SECONDS, |p| shared.store(((k as f32 + p) / count).to_bits(), Relaxed));
                // a track that never makes a sound gets no file
                if frames.iter().any(|s| s.abs() > 1e-4) {
                    if let Err(e) = encode(format, &file, &frames, RATE) {
                        let _ = tx.send(Err(e));
                        return;
                    }
                    written += 1;
                }
            }
            let _ = tx.send(if written > 0 { Ok((folder, None)) } else { Err(tr!("every track was silent").into()) });
        });
        self.export = Some(Export { progress, done });
    }

    /// 0..1 while an export runs.
    pub fn export_progress(&self) -> Option<f32> {
        self.export.as_ref().map(|e| f32::from_bits(e.progress.load(Relaxed)))
    }

    pub fn poll_export(&mut self) {
        let Some(export) = &self.export else { return };
        let result = match export.done.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(tr!("export crashed").into()),
        };
        self.export = None;
        match result {
            Ok((path, None)) => self.say_as(Tone::Done, tr!("Exported {}", path.display())),
            Ok((path, Some((lufs, peak)))) => {
                let mut text = tr!("Exported {} · {} LUFS · true peak {} dBTP", path.display(), format!("{lufs:.1}"), format!("{peak:.1}"));
                // streaming services turn louder songs down to about -14 LUFS, and their encoders clip peaks over -1 dBTP
                if peak > -1.0 {
                    text += tr!(" · peaks over -1 dBTP can distort once streamed: turn the master down a little");
                }
                self.say_as(if peak > -1.0 { Tone::Info } else { Tone::Done }, text);
            }
            Err(e) => self.say_as(Tone::Error, tr!("Export failed: {}", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ravune_engine::{Sample, write_wav};

    /// A stem per mixer track: the track alone, plus the returns it sends to.
    #[test]
    fn stems_solo_one_track_each() {
        let techno: ravune_core::Project = ron::from_str(crate::files::TEMPLATES[0].1).unwrap();
        let stems = stem_projects(&techno);
        assert_eq!(stems.len(), techno.mixer.len() - 1);
        let (_, name, kick) = &stems[1];
        assert_eq!(name, "Kick");
        let heard = kick.tracks_heard();
        assert!(heard[2] && heard[1] && heard[0], "the kick, its drum bus, the master");
        assert!(!heard[3], "not the bass");
    }

    /// Every format comes back through the decoder as written (MP3 only where ffmpeg is).
    #[test]
    fn formats_decode_back() {
        let rate = 22_050;
        let sine: Vec<f32> = (0..rate * 2).flat_map(|i| [0.5 * (i as f32 * 0.05).sin(); 2]).collect();
        for format in Format::ALL.into_iter().filter(|f| *f != Format::Mp3 || has_ffmpeg()) {
            let path = std::env::temp_dir().join(format!("ravune-format-{format:?}.{}", format.ext()));
            encode(format, &path, &sine, rate as u32).unwrap();
            let back = Sample::decode_file(&path).unwrap();
            assert!((back.seconds() - 2.0).abs() < 0.05, "{format:?}: {} s", back.seconds());
            let peak = back.frames.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!((peak - 0.5).abs() < 0.05, "{format:?}: peak {peak}");
        }
    }

    /// Every template plays out loud and clean from its first bar.
    #[test]
    fn templates_sound() {
        let mut library = crate::browser::Library::default();
        let transport = Transport { playing: true, mode: Mode::Song, start: 0, jump: 0, looped: None };
        for (name, text) in crate::files::TEMPLATES {
            let p: ravune_core::Project = ron::from_str(text).unwrap();
            let song = Song::compile(&p, transport, |s| library.get(s).ok());
            let frames = render_offline(song, 22_050.0, 0.0, |_| {});
            assert!(frames.iter().all(|s| s.is_finite() && s.abs() <= 1.0), "{name} clips");
            let rms = (frames.iter().map(|s| s * s).sum::<f32>() / frames.len() as f32).sqrt();
            assert!(rms > 0.08, "{name} too quiet: rms {rms}");
            write_wav(&std::env::temp_dir().join(format!("ravune-template-{}.wav", name.split(' ').next().unwrap())), &frames, 2, 22_050).unwrap();
        }
    }
}
