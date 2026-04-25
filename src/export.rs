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

use crate::dsp::FilterChainStereo;
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

/// Mix unmuted tracks at the project's sample rate and write `options.out_path`
/// in the requested format. The output is stereo iff any unmuted source track
/// is stereo; otherwise mono. Mono inputs in a stereo mix are centre-panned,
/// stereo inputs in a mono mix are averaged to (L+R)/2.
pub fn export(project: &Project, options: &ExportOptions) -> Result<()> {
    let active: Vec<&Track> = project.tracks.iter().filter(|t| !t.mute).collect();
    if active.is_empty() {
        return Err(anyhow!("nothing to export — no unmuted tracks"));
    }

    let (mix, sample_rate, channels) = mixdown(project, &active)?;
    match options.format {
        ExportFormat::Wav => write_wav_16(&options.out_path, &mix, sample_rate, channels)?,
        _ => encode_via_ffmpeg(&mix, sample_rate, channels, options)?,
    }
    Ok(())
}

/// Sum all non-muted tracks into an interleaved f32 buffer (mono or stereo),
/// applying per-track linear gain. All tracks must share a sample rate
/// (first track wins; differing rates error out — resampling not supported).
///
/// Returns `(interleaved_samples, sample_rate, out_channels)` where
/// `out_channels` is `2` if any input track was stereo, else `1`.
fn mixdown(project: &Project, tracks: &[&Track]) -> Result<(Vec<f32>, u32, u16)> {
    let mut sample_rate = 0u32;
    let is_stereo_mix = tracks.iter().any(|t| t.stereo);
    let out_channels: u16 = if is_stereo_mix { 2 } else { 1 };
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
        let raw: Vec<f32> = match spec.sample_format {
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

        let in_channels = spec.channels.max(1) as usize;
        let frame_count = raw.len() / in_channels;
        let mut buf = Vec::with_capacity(frame_count * out_channels as usize);

        // Per-track correction: applies in stereo, even on mono inputs
        // (the input is centre-panned to L=R first, processed, then
        // either kept stereo or summed back to mono per the mix layout).
        let mut chain: Option<FilterChainStereo> = t
            .correction
            .as_ref()
            .map(|p| FilterChainStereo::new(p.clone(), spec.sample_rate));

        for f in 0..frame_count {
            let base = f * in_channels;
            let (mut l, mut r) = if in_channels >= 2 {
                (raw[base], raw[base + 1])
            } else {
                (raw[base], raw[base])
            };
            if let Some(c) = chain.as_mut() {
                let (ll, rr) = c.process(l, r);
                l = ll;
                r = rr;
            }
            if is_stereo_mix {
                buf.push(l);
                buf.push(r);
            } else {
                buf.push(0.5 * (l + r));
            }
        }
        per_track.push(buf);
    }

    let longest = per_track.iter().map(|v| v.len()).max().unwrap_or(0);
    let mut mix = vec![0.0f32; longest];
    for track in &per_track {
        for (i, s) in track.iter().enumerate() {
            mix[i] += *s;
        }
    }
    // Soft-limit to [-1, 1].
    let peak = mix.iter().copied().fold(0.0f32, |a, b| a.max(b.abs()));
    if peak > 1.0 {
        let k = 1.0 / peak;
        for s in &mut mix { *s *= k; }
    }
    Ok((mix, sample_rate, out_channels))
}

fn db_to_lin(db: f32) -> f32 { 10f32.powf(db / 20.0) }

fn write_wav_16(path: &Path, samples: &[f32], sample_rate: u32, channels: u16) -> Result<()> {
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    let spec = WavSpec {
        channels,
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
/// isn't findable. The temp file preserves the mix channel count so the
/// encoder sees mono or stereo as appropriate.
fn encode_via_ffmpeg(
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    opt: &ExportOptions,
) -> Result<()> {
    let ffmpeg = find_ffmpeg().ok_or_else(|| anyhow!(
        "ffmpeg not found. Drop ffmpeg.exe next to the app (or into ./ffmpeg/bin/), \
         or install it on your PATH, then try again."
    ))?;

    let tmp = std::env::temp_dir().join(format!(
        "tinybooth-export-{}.wav",
        std::process::id()
    ));
    write_wav_16(&tmp, samples, sample_rate, channels)?;

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
