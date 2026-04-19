//! Export a TinyBooth project to common audio formats.
//!
//! Strategy: read every non-muted track via hound, resample-by-padding to the
//! longest track, apply per-track gain, sum into a mono float buffer, then
//! either (a) write the result as WAV natively, or (b) pipe a temporary WAV
//! through ffmpeg to reach the target codec.
//!
//! ffmpeg is located by searching, in order:
//!   1. `<exe_dir>\ffmpeg.exe`
//!   2. `<exe_dir>\ffmpeg\bin\ffmpeg.exe`
//!   3. the system `PATH` (`ffmpeg` or `ffmpeg.exe`)
//!
//! If none is found and the target is not WAV, the export fails with a
//! human-readable message.

use anyhow::{anyhow, Context, Result};
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::project::{Project, Track};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Wav,
    Flac,
    Mp3,
    OggVorbis,
    OggOpus,
    M4aAac,
}

impl ExportFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Wav => "WAV (16-bit)",
            Self::Flac => "FLAC",
            Self::Mp3 => "MP3",
            Self::OggVorbis => "Ogg Vorbis",
            Self::OggOpus => "Ogg Opus",
            Self::M4aAac => "M4A (AAC)",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::OggVorbis => "ogg",
            Self::OggOpus => "opus",
            Self::M4aAac => "m4a",
        }
    }
    pub fn needs_ffmpeg(self) -> bool { !matches!(self, Self::Wav) }
    pub fn all() -> [Self; 6] {
        [Self::Wav, Self::Flac, Self::Mp3, Self::OggVorbis, Self::OggOpus, Self::M4aAac]
    }
}

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub format: ExportFormat,
    /// kbps for lossy codecs; ignored for lossless ones.
    pub bitrate_kbps: u32,
    pub out_path: PathBuf,
}

/// Mix unmuted tracks to mono f32 at the project's sample rate and write
/// `options.out_path` in the requested format.
pub fn export(project: &Project, options: &ExportOptions) -> Result<()> {
    let active: Vec<&Track> = project.tracks.iter().filter(|t| !t.mute).collect();
    if active.is_empty() {
        return Err(anyhow!("nothing to export — no unmuted tracks"));
    }

    // Decide the mix sample rate: use the first track's rate as the reference.
    let (mix, sample_rate) = mixdown(project, &active)?;
    match options.format {
        ExportFormat::Wav => write_wav_16(&options.out_path, &mix, sample_rate)?,
        _ => encode_via_ffmpeg(&mix, sample_rate, options)?,
    }
    Ok(())
}

/// Sum all non-muted tracks into a single mono f32 buffer, applying per-track
/// linear gain. All tracks must share a sample rate (first track wins; any
/// track with a differing rate yields an error — keep the tool honest).
fn mixdown(project: &Project, tracks: &[&Track]) -> Result<(Vec<f32>, u32)> {
    let mut sample_rate = 0u32;
    let mut per_track: Vec<Vec<f32>> = Vec::with_capacity(tracks.len());

    for t in tracks {
        let abs = project.track_abs_path(t);
        let reader = WavReader::open(&abs)
            .with_context(|| format!("opening track {}", abs.display()))?;
        let spec = reader.spec();
        if sample_rate == 0 {
            sample_rate = spec.sample_rate;
        } else if spec.sample_rate != sample_rate {
            return Err(anyhow!(
                "track '{}' has {} Hz but project expects {} Hz — resampling is not yet supported",
                t.name, spec.sample_rate, sample_rate
            ));
        }
        let gain = db_to_lin(t.gain_db);
        let samples: Vec<f32> = match spec.sample_format {
            SampleFormat::Int => reader
                .into_samples::<i32>()
                .filter_map(|r| r.ok())
                .map(|s| s as f32 / i16::MAX as f32 * gain)
                .collect(),
            SampleFormat::Float => reader
                .into_samples::<f32>()
                .filter_map(|r| r.ok())
                .map(|s| s * gain)
                .collect(),
        };
        per_track.push(samples);
    }

    let longest = per_track.iter().map(|v| v.len()).max().unwrap_or(0);
    let mut mix = vec![0.0f32; longest];
    for track in &per_track {
        for (i, s) in track.iter().enumerate() {
            mix[i] += *s;
        }
    }
    // Soft limit at [-1.0, 1.0] — tracks can sum above unity.
    let peak = mix.iter().copied().fold(0.0f32, |a, b| a.max(b.abs()));
    if peak > 1.0 {
        let k = 1.0 / peak;
        for s in &mut mix { *s *= k; }
    }
    Ok((mix, sample_rate))
}

fn db_to_lin(db: f32) -> f32 { 10f32.powf(db / 20.0) }

fn write_wav_16(path: &Path, samples: &[f32], sample_rate: u32) -> Result<()> {
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut w = WavWriter::create(path, spec).context("creating WAV")?;
    for s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        w.write_sample((clamped * i16::MAX as f32) as i16)?;
    }
    w.finalize()?;
    Ok(())
}

/// Pipe a temporary WAV through ffmpeg. Returns a friendly error if ffmpeg
/// isn't findable.
fn encode_via_ffmpeg(samples: &[f32], sample_rate: u32, opt: &ExportOptions) -> Result<()> {
    let ffmpeg = find_ffmpeg().ok_or_else(|| anyhow!(
        "ffmpeg not found. Drop ffmpeg.exe next to the app (or into ./ffmpeg/bin/), \
         or install it on your PATH, then try again."
    ))?;

    // Write the mixdown to a temp WAV first, then have ffmpeg read that file.
    let tmp = std::env::temp_dir().join(format!(
        "tinybooth-export-{}.wav",
        std::process::id()
    ));
    write_wav_16(&tmp, samples, sample_rate)?;

    if let Some(p) = opt.out_path.parent() { std::fs::create_dir_all(p)?; }

    let mut cmd = Command::new(&ffmpeg);
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel").arg("error")
        .arg("-i").arg(&tmp);

    match opt.format {
        ExportFormat::Flac => {
            cmd.arg("-c:a").arg("flac");
        }
        ExportFormat::Mp3 => {
            cmd.arg("-c:a").arg("libmp3lame")
                .arg("-b:a").arg(format!("{}k", opt.bitrate_kbps));
        }
        ExportFormat::OggVorbis => {
            cmd.arg("-c:a").arg("libvorbis")
                .arg("-b:a").arg(format!("{}k", opt.bitrate_kbps));
        }
        ExportFormat::OggOpus => {
            cmd.arg("-c:a").arg("libopus")
                .arg("-b:a").arg(format!("{}k", opt.bitrate_kbps));
        }
        ExportFormat::M4aAac => {
            cmd.arg("-c:a").arg("aac")
                .arg("-b:a").arg(format!("{}k", opt.bitrate_kbps));
        }
        ExportFormat::Wav => unreachable!("WAV handled natively"),
    }

    cmd.arg(&opt.out_path);

    let status = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning ffmpeg")?
        .wait_with_output()?;

    let _ = std::fs::remove_file(&tmp);

    if !status.status.success() {
        let err = String::from_utf8_lossy(&status.stderr).to_string();
        return Err(anyhow!("ffmpeg failed:\n{err}"));
    }
    Ok(())
}

fn find_ffmpeg() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let direct = dir.join("ffmpeg.exe");
            if direct.is_file() { return Some(direct); }
            let bundled = dir.join("ffmpeg").join("bin").join("ffmpeg.exe");
            if bundled.is_file() { return Some(bundled); }
        }
    }
    // system PATH — rely on `where`/`which`-style resolution by spawning.
    // We just return the bare name; Command will resolve it.
    for candidate in ["ffmpeg.exe", "ffmpeg"] {
        if Command::new(candidate).arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok() {
            return Some(PathBuf::from(candidate));
        }
    }
    None
}

pub fn ffmpeg_available() -> bool { find_ffmpeg().is_some() }
