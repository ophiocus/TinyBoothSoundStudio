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
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::automation::SplineSampler;
use crate::dsp::FilterChainStereo;
use crate::project::{Project, Track};
use crate::tib::TibDb;

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
    pub fn needs_ffmpeg(self) -> bool {
        !matches!(self, Self::Wav)
    }
    pub fn all() -> [Self; 6] {
        [
            Self::Wav,
            Self::Flac,
            Self::Mp3,
            Self::OggVorbis,
            Self::OggOpus,
            Self::M4aAac,
        ]
    }
}

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub format: ExportFormat,
    /// kbps for lossy codecs; ignored for lossless ones.
    pub bitrate_kbps: u32,
    pub out_path: PathBuf,
}

/// Render the master mix into memory without writing it anywhere.
/// Same DSP as [`export`] — just stops before the encode step. Used by
/// the Bounce path (TBSS-FR-0011 §A) to stash the result in the .tib's
/// `mix_run` row.
pub fn render_master_mix(project: &Project, db: Option<&TibDb>) -> Result<(Vec<f32>, u32, u16)> {
    let active: Vec<&Track> = project.tracks.iter().filter(|t| !t.mute).collect();
    if active.is_empty() {
        return Err(anyhow!("nothing to render — no unmuted tracks"));
    }
    mixdown(project, &active, db)
}

/// Render the master mix and encode it as a 16-bit WAV byte stream
/// (complete with header) so consumers can decode via `WavReader::new`.
/// Returns `(wav_bytes, sample_rate, channels, frames)`. TBSS-FR-0011 §A.
pub fn render_master_mix_to_wav_bytes(
    project: &Project,
    db: Option<&TibDb>,
) -> Result<(Vec<u8>, u32, u16, u64)> {
    let (samples, sample_rate, channels) = render_master_mix(project, db)?;
    let frames = (samples.len() as u64) / (channels as u64).max(1);
    let mut buf = std::io::Cursor::new(Vec::<u8>::with_capacity(samples.len() * 2 + 44));
    {
        let spec = WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::new(&mut buf, spec).context("creating in-memory WAV writer")?;
        for s in &samples {
            let clamped = s.clamp(-1.0, 1.0);
            w.write_sample((clamped * i16::MAX as f32) as i16)?;
        }
        w.finalize().context("finalising in-memory WAV")?;
    }
    Ok((buf.into_inner(), sample_rate, channels, frames))
}

/// Compute a stable hash over the project state that influences the
/// rendered master mix. The Bounce path stamps the `.tib`'s `mix_run`
/// row with this signature; the Mix tab compares the live signature
/// against the stored one to flag the cache as fresh / stale.
/// TBSS-FR-0011 §A.
///
/// Includes everything that materially affects the mix bytes:
///   * each track's current revision id (.tib backing) or relative
///     file path (folder backing) — captures "the audio source changed"
///   * each track's mute / gain_db / polarity / gain_automation / correction
///   * project master_gain_db / master_gain_automation / corrections_disabled
///
/// Excludes UI-only state (selected tab, viewport zoom, etc.).
pub fn compute_mixrun_signature(project: &Project, db: Option<&TibDb>) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    #[derive(serde::Serialize)]
    struct TrackSig<'a> {
        id: &'a str,
        rev: Option<i64>,
        file: &'a str,
        mute: bool,
        gain_db_bits: u32,
        polarity: bool,
        automation: &'a Option<crate::automation::AutomationLane>,
        correction: &'a Option<crate::dsp::Profile>,
    }
    #[derive(serde::Serialize)]
    struct MixSig<'a> {
        master_gain_db_bits: u32,
        master_automation: &'a Option<crate::automation::AutomationLane>,
        corrections_disabled: bool,
        tracks: Vec<TrackSig<'a>>,
    }
    let tracks: Vec<TrackSig> = project
        .tracks
        .iter()
        .map(|t| TrackSig {
            id: &t.id,
            rev: db.and_then(|d| d.current_rev_id(&t.id).ok().flatten()),
            file: &t.file,
            mute: t.mute,
            gain_db_bits: t.gain_db.to_bits(),
            polarity: t.polarity_inverted,
            automation: &t.gain_automation,
            correction: &t.correction,
        })
        .collect();
    let sig = MixSig {
        master_gain_db_bits: project.master_gain_db.to_bits(),
        master_automation: &project.master_gain_automation,
        corrections_disabled: project.corrections_disabled,
        tracks,
    };
    let json = serde_json::to_string(&sig).unwrap_or_default();
    let mut h = DefaultHasher::new();
    json.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Mix unmuted tracks at the project's sample rate and write `options.out_path`
/// in the requested format. The output is stereo iff any unmuted source track
/// is stereo; otherwise mono. Mono inputs in a stereo mix are centre-panned,
/// stereo inputs in a mono mix are averaged to (L+R)/2.
/// `db` is `Some` for `.tib`-backed projects (audio read from BLOBs by
/// track id) and `None` for folder projects (audio read from sibling
/// WAV files). TBSS-FR-0007 phase 2c.
pub fn export(project: &Project, options: &ExportOptions, db: Option<&TibDb>) -> Result<()> {
    let active: Vec<&Track> = project.tracks.iter().filter(|t| !t.mute).collect();
    if active.is_empty() {
        return Err(anyhow!("nothing to export — no unmuted tracks"));
    }

    let (mix, sample_rate, channels) = mixdown(project, &active, db)?;
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
fn mixdown(
    project: &Project,
    tracks: &[&Track],
    db: Option<&TibDb>,
) -> Result<(Vec<f32>, u32, u16)> {
    let mut sample_rate = 0u32;
    let is_stereo_mix = tracks.iter().any(|t| t.stereo);
    let out_channels: u16 = if is_stereo_mix { 2 } else { 1 };
    let mut per_track: Vec<Vec<f32>> = Vec::with_capacity(tracks.len());

    for t in tracks {
        let (spec, raw) = read_track_pcm(project, t, db)?;
        if sample_rate == 0 {
            sample_rate = spec.sample_rate;
        } else if spec.sample_rate != sample_rate {
            return Err(anyhow!(
                "track '{}' has {} Hz but project expects {} Hz — resampling is not yet supported",
                t.name,
                spec.sample_rate,
                sample_rate
            ));
        }

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
        let auto_sampler: Option<SplineSampler> =
            t.gain_automation.as_ref().map(SplineSampler::build);
        let static_gain_db = t.gain_db;
        let static_gain_lin = db_to_lin(static_gain_db);
        let sr_f = spec.sample_rate as f32;

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
            // Effective gain: spline sample if automation present at
            // this time, else the cached static linear gain.
            let g = match auto_sampler
                .as_ref()
                .and_then(|s| s.sample(f as f32 / sr_f))
            {
                Some(db) => db_to_lin(db),
                None => static_gain_lin,
            };
            let l_g = l * g;
            let r_g = r * g;
            if is_stereo_mix {
                buf.push(l_g);
                buf.push(r_g);
            } else {
                buf.push(0.5 * (l_g + r_g));
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

    // Master fader + automation. Per-frame gain so the rendered file
    // matches what the Mix-tab playback produces.
    let master_static = project.master_gain_db;
    let master_auto = project
        .master_gain_automation
        .as_ref()
        .map(SplineSampler::build);
    if master_static != 0.0 || master_auto.is_some() {
        let stride = out_channels as usize;
        let frames = mix.len() / stride.max(1);
        let sr_f = sample_rate as f32;
        for f in 0..frames {
            let t_secs = f as f32 / sr_f;
            let gain_db = master_auto
                .as_ref()
                .and_then(|s| s.sample(t_secs))
                .unwrap_or(master_static);
            let g = db_to_lin(gain_db);
            for c in 0..stride {
                mix[f * stride + c] *= g;
            }
        }
    }

    // Soft-limit to [-1, 1].
    let peak = mix.iter().copied().fold(0.0f32, |a, b| a.max(b.abs()));
    if peak > 1.0 {
        let k = 1.0 / peak;
        for s in &mut mix {
            *s *= k;
        }
    }
    Ok((mix, sample_rate, out_channels))
}

/// Read a track's raw PCM as unity-scaled f32 (no gain applied — gain
/// is folded in per-frame by the caller). Reads from a `.tib` BLOB by
/// track id when `db` is `Some`, else from the sibling WAV file.
fn read_track_pcm(project: &Project, t: &Track, db: Option<&TibDb>) -> Result<(WavSpec, Vec<f32>)> {
    match db {
        Some(db) => {
            let bytes = db
                .read_current_audio(&t.id)
                .with_context(|| format!("reading BLOB for track '{}'", t.name))?;
            let reader = WavReader::new(Cursor::new(bytes))
                .with_context(|| format!("parsing in-memory WAV for track '{}'", t.name))?;
            decode_pcm(reader)
        }
        None => {
            let abs = project.track_abs_path(t);
            let reader = WavReader::open(&abs)
                .with_context(|| format!("opening track {}", abs.display()))?;
            decode_pcm(reader)
        }
    }
}

/// Decode a WAV stream to unity f32. Mirrors the historical export
/// scaling (int samples read as i32, divided by `i16::MAX`).
fn decode_pcm<R: Read>(reader: WavReader<R>) -> Result<(WavSpec, Vec<f32>)> {
    let spec = reader.spec();
    let raw: Vec<f32> = match spec.sample_format {
        SampleFormat::Int => reader
            .into_samples::<i32>()
            .filter_map(|r| r.ok())
            .map(|s| s as f32 / i16::MAX as f32)
            .collect(),
        SampleFormat::Float => reader
            .into_samples::<f32>()
            .filter_map(|r| r.ok())
            .collect(),
    };
    Ok((spec, raw))
}

fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn write_wav_16(path: &Path, samples: &[f32], sample_rate: u32, channels: u16) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
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
    let ffmpeg = find_ffmpeg().ok_or_else(|| {
        anyhow!(
            "ffmpeg not found. Drop ffmpeg.exe next to the app (or into ./ffmpeg/bin/), \
         or install it on your PATH, then try again."
        )
    })?;

    let tmp = std::env::temp_dir().join(format!("tinybooth-export-{}.wav", std::process::id()));
    write_wav_16(&tmp, samples, sample_rate, channels)?;

    if let Some(p) = opt.out_path.parent() {
        std::fs::create_dir_all(p)?;
    }

    let mut cmd = Command::new(&ffmpeg);
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(&tmp);

    match opt.format {
        ExportFormat::Flac => {
            cmd.arg("-c:a").arg("flac");
        }
        ExportFormat::Mp3 => {
            cmd.arg("-c:a")
                .arg("libmp3lame")
                .arg("-b:a")
                .arg(format!("{}k", opt.bitrate_kbps));
        }
        ExportFormat::OggVorbis => {
            cmd.arg("-c:a")
                .arg("libvorbis")
                .arg("-b:a")
                .arg(format!("{}k", opt.bitrate_kbps));
        }
        ExportFormat::OggOpus => {
            cmd.arg("-c:a")
                .arg("libopus")
                .arg("-b:a")
                .arg(format!("{}k", opt.bitrate_kbps));
        }
        ExportFormat::M4aAac => {
            cmd.arg("-c:a")
                .arg("aac")
                .arg("-b:a")
                .arg(format!("{}k", opt.bitrate_kbps));
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
            if direct.is_file() {
                return Some(direct);
            }
            let bundled = dir.join("ffmpeg").join("bin").join("ffmpeg.exe");
            if bundled.is_file() {
                return Some(bundled);
            }
        }
    }
    // system PATH — rely on `where`/`which`-style resolution by spawning.
    // We just return the bare name; Command will resolve it.
    for candidate in ["ffmpeg.exe", "ffmpeg"] {
        if Command::new(candidate)
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
        {
            return Some(PathBuf::from(candidate));
        }
    }
    None
}

pub fn ffmpeg_available() -> bool {
    find_ffmpeg().is_some()
}

/// Write a pre-computed interleaved-stereo f32 buffer to disk via the
/// same WAV / ffmpeg pipeline `export()` uses. Reused by the Crossfade
/// tab (TBSS-FR-0010) — there's no `Project` to read from there.
pub fn write_crossfade(
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    options: &ExportOptions,
) -> Result<()> {
    match options.format {
        ExportFormat::Wav => write_wav_16(&options.out_path, samples, sample_rate, channels)?,
        _ => encode_via_ffmpeg(samples, sample_rate, channels, options)?,
    }
    Ok(())
}

#[cfg(test)]
mod tib_export_tests {
    //! TBSS-FR-0007 phase 2c step 6: exporting a `.tib` project must
    //! produce the exact same mix as exporting the folder project it was
    //! migrated from — the only difference is where the bytes are read.
    use super::*;
    use crate::project::{Project, Track, TrackSource};
    use crate::telemetry::TelemetryProfile;
    use crate::tib::TibDb;

    fn write_wav_file(path: &Path, frames: u32, rate: u32) {
        let spec = WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(path, spec).unwrap();
        for i in 0..frames {
            for c in 0..2u32 {
                w.write_sample(((i as i32 * 3 + c as i32 * 7) % 1000) as i16)
                    .unwrap();
            }
        }
        w.finalize().unwrap();
    }

    fn folder_track(id: &str, file: &str, name: &str, rate: u32) -> Track {
        Track {
            id: id.into(),
            name: name.into(),
            file: file.into(),
            mute: false,
            gain_db: 0.0,
            sample_rate: rate,
            channel_source: None,
            duration_secs: 0.0,
            profile: None,
            stereo: true,
            source: TrackSource::default(),
            correction: None,
            gain_automation: None,
            polarity_inverted: false,
            telemetry: None,
            telemetry_profile: TelemetryProfile::default(),
        }
    }

    #[test]
    fn export_from_tib_matches_folder_export_byte_for_byte() {
        let dir = std::env::temp_dir().join(format!("tbss-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tracks")).unwrap();
        write_wav_file(&dir.join("tracks/a.wav"), 500, 48_000);
        write_wav_file(&dir.join("tracks/b.wav"), 500, 48_000);

        let mut proj = Project::new("E", dir.clone());
        proj.tracks
            .push(folder_track("a", "tracks/a.wav", "A", 48_000));
        proj.tracks
            .push(folder_track("b", "tracks/b.wav", "B", 48_000));

        // Folder export (reads sibling WAVs).
        let out_folder = dir.join("folder.wav");
        export(
            &proj,
            &ExportOptions {
                format: ExportFormat::Wav,
                bitrate_kbps: 192,
                out_path: out_folder.clone(),
            },
            None,
        )
        .unwrap();

        // Migrate → .tib, reload, export from BLOBs.
        let tib = dir.join("e.tib");
        crate::tib_project::migrate_folder_to_tib(&proj, &tib).unwrap();
        let db = TibDb::open(&tib).unwrap();
        let tib_proj = crate::tib_project::load_project(&db, tib.clone()).unwrap();
        let out_tib = dir.join("tib.wav");
        export(
            &tib_proj,
            &ExportOptions {
                format: ExportFormat::Wav,
                bitrate_kbps: 192,
                out_path: out_tib.clone(),
            },
            Some(&db),
        )
        .unwrap();

        let fb = std::fs::read(&out_folder).unwrap();
        let tb = std::fs::read(&out_tib).unwrap();
        assert_eq!(fb, tb, "tib export must match folder export byte-for-byte");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
