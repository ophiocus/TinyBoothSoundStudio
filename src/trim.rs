//! Project-level audio trim. **Destructive** — every WAV in the
//! project (stems + the optional Suno mixdown) is cropped in place to
//! the same `[start_secs, end_secs]` range, atomically via a `.tmp`
//! sibling + rename so a crash mid-write leaves either the old or the
//! new file intact, never a truncated one.
//!
//! Design notes:
//!   • The trim range is stored as **seconds**, not frames. Per-file
//!     frame ranges are computed from each WAV's own sample rate at
//!     apply time, so mixed-rate projects (rare but possible) work
//!     without fuss.
//!   • Coherence stays valid post-trim because every file in the
//!     project shares the same new frame-0 — the bundled Suno mixdown
//!     gets the same crop as the stems, so summing them produces the
//!     same residual it always did, just over a shorter window.
//!   • `Track.duration_secs` is updated on each touched track; the
//!     manifest gets re-saved by the caller (`app.save_project()`).
//!   • Peak tables (in `TrackPlay`) are rebuilt by the player on next
//!     project re-open via `Player::new`. Live trim during playback
//!     would need a player rebuild path; we don't go there in v1 —
//!     the panel asks the user to stop playback first.

use anyhow::{anyhow, Context, Result};
use std::io::Cursor;
use std::path::Path;

use crate::project::{Project, TRACKS_DIR};
use crate::tib::TibDb;
use crate::tib_project::MIXDOWN_TRACK_ID;

/// Outcome of a project-wide trim. Surfaced to the user in the trim
/// panel's status area. Per-file successes are folded into a count;
/// the diagnostic detail we *actually* surface is the failure list,
/// because that's what the user needs to act on.
#[derive(Debug, Clone)]
pub struct TrimReport {
    pub start_secs: f32,
    pub end_secs: f32,
    pub trimmed_count: usize,
    pub failures: Vec<TrimFileFailure>,
}

#[derive(Debug, Clone)]
pub struct TrimFileFailure {
    pub path_relative: String,
    pub error: String,
}

impl TrimReport {
    /// Multi-line human-readable summary for the panel status field.
    /// On the happy path: a single line with file count + new range.
    /// When any file failed: appends a per-failure breakdown so the
    /// user can see which file (and why) without digging in the log.
    pub fn summary_line(&self) -> String {
        let kept_secs = (self.end_secs - self.start_secs).max(0.0);
        let mut s = if self.failures.is_empty() {
            format!(
                "Trimmed {} file(s) to {:.2}s ({:.2}s → {:.2}s).",
                self.trimmed_count, kept_secs, self.start_secs, self.end_secs
            )
        } else {
            format!(
                "Trimmed {} file(s); {} failed. Range {:.2}s → {:.2}s.",
                self.trimmed_count,
                self.failures.len(),
                self.start_secs,
                self.end_secs
            )
        };
        for f in &self.failures {
            s.push_str(&format!("\n  ✗ {}: {}", f.path_relative, f.error));
        }
        s
    }
}

/// Validate a trim range. Shared by the folder and `.tib` trim entries.
fn validate_range(start_secs: f32, end_secs: f32) -> Result<()> {
    if !(start_secs.is_finite() && end_secs.is_finite()) {
        return Err(anyhow!("trim range must be finite"));
    }
    if start_secs < 0.0 {
        return Err(anyhow!("start must be ≥ 0"));
    }
    if end_secs <= start_secs {
        return Err(anyhow!(
            "end ({end_secs:.3}s) must be > start ({start_secs:.3}s)"
        ));
    }
    Ok(())
}

/// Crop every WAV in the project (tracks + bundled mixdown) to the
/// shared `[start_secs, end_secs]` range.
///
/// Updates `Track.duration_secs` on each successfully-trimmed track
/// to reflect the new length. Caller is responsible for marking the
/// project dirty and saving the manifest.
///
/// Returns `Err` only on caller-error (invalid range); per-file
/// failures are collected in `TrimReport.failures` so partial-success
/// is surfaced rather than silently swallowed.
pub fn trim_project(project: &mut Project, start_secs: f32, end_secs: f32) -> Result<TrimReport> {
    validate_range(start_secs, end_secs)?;

    let project_root = project.root.clone();
    let mut trimmed_count: usize = 0;
    let mut failures = Vec::new();

    // Tracks first — keep the borrow on `project` short by collecting
    // (rel_path, idx) pairs before doing the I/O.
    let track_targets: Vec<(usize, String)> = project
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| (i, t.file.clone()))
        .collect();

    for (idx, rel) in track_targets {
        let abs = project_root.join(&rel);
        match trim_wav_atomic(&abs, start_secs, end_secs) {
            Ok((_orig, new)) => {
                // Reflect new length on the track. The per-file frame
                // count is at the WAV's own rate, which we read from
                // the track's stored sample_rate.
                let sr = project.tracks[idx].sample_rate.max(1) as f32;
                project.tracks[idx].duration_secs = new as f32 / sr;
                trimmed_count += 1;
            }
            Err(e) => {
                failures.push(TrimFileFailure {
                    path_relative: rel,
                    error: format!("{e:#}"),
                });
            }
        }
    }

    // Bundled Suno mixdown (if present). Same trim range so coherence
    // analysis stays valid after the crop.
    if let Some(rel) = project.suno_mixdown_path.clone() {
        let abs = project_root.join(&rel);
        match trim_wav_atomic(&abs, start_secs, end_secs) {
            Ok(_) => {
                trimmed_count += 1;
            }
            Err(e) => {
                failures.push(TrimFileFailure {
                    path_relative: rel,
                    error: format!("{e:#}"),
                });
            }
        }
    }

    Ok(TrimReport {
        start_secs,
        end_secs,
        trimmed_count,
        failures,
    })
}

/// Crop every track in a `.tib`-backed project (stems + the bundled
/// mixdown, if any) to the shared `[start_secs, end_secs]` range. **The
/// `.tib` counterpart of [`trim_project`].** Instead of overwriting WAV
/// files, each track gets a new `destructive` revision carrying the
/// cropped audio, committed atomically with a `current_rev_id` repoint
/// and a FIFO-5 prune ([`TibDb::commit_destructive_revision`]) — so the
/// edit is reversible (roll back by repointing) and crash-safe.
///
/// Updates `Track.duration_secs` on each cropped track. The caller marks
/// the project dirty + saves the metadata (which persists the new
/// durations) and drops the player so it rebuilds from the new revisions.
pub fn trim_project_tib(
    project: &mut Project,
    db: &mut TibDb,
    start_secs: f32,
    end_secs: f32,
) -> Result<TrimReport> {
    validate_range(start_secs, end_secs)?;

    let mut trimmed_count = 0;
    let mut failures = Vec::new();

    // Collect (idx, track_id) first to keep the project borrow short.
    let targets: Vec<(usize, String)> = project
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| (i, t.id.clone()))
        .collect();

    for (idx, track_id) in targets {
        match trim_revision_in_db(db, &track_id, start_secs, end_secs) {
            Ok(new_frames) => {
                let sr = project.tracks[idx].sample_rate.max(1) as f32;
                project.tracks[idx].duration_secs = new_frames as f32 / sr;
                trimmed_count += 1;
            }
            Err(e) => failures.push(TrimFileFailure {
                path_relative: track_id,
                error: format!("{e:#}"),
            }),
        }
    }

    // The bundled mixdown is the reserved not-in-mix MIXDOWN_TRACK_ID
    // track in a .tib (load_project stamps suno_mixdown_path with it).
    // Crop it on the same range so coherence stays valid post-trim.
    if project.suno_mixdown_path.as_deref() == Some(MIXDOWN_TRACK_ID) {
        match trim_revision_in_db(db, MIXDOWN_TRACK_ID, start_secs, end_secs) {
            Ok(_) => trimmed_count += 1,
            Err(e) => failures.push(TrimFileFailure {
                path_relative: MIXDOWN_TRACK_ID.to_string(),
                error: format!("{e:#}"),
            }),
        }
    }

    Ok(TrimReport {
        start_secs,
        end_secs,
        trimmed_count,
        failures,
    })
}

/// Read a track's current audio out of the `.tib`, crop it in memory,
/// and commit the result as a new destructive revision. Returns the new
/// frame count. The read borrow (`&self`) and the commit borrow
/// (`&mut self`) don't overlap.
fn trim_revision_in_db(
    db: &mut TibDb,
    track_id: &str,
    start_secs: f32,
    end_secs: f32,
) -> Result<u64> {
    let bytes = db
        .read_current_audio(track_id)
        .with_context(|| format!("reading current audio for {track_id}"))?;
    let cropped = crop_wav_bytes(&bytes, start_secs, end_secs)
        .with_context(|| format!("cropping audio for {track_id}"))?;
    db.commit_destructive_revision(
        track_id,
        "trim",
        cropped.sample_rate,
        cropped.stereo,
        cropped.duration_secs,
        &cropped.bytes,
        5,
    )
    .with_context(|| format!("committing trim revision for {track_id}"))?;
    db.incremental_vacuum()?;
    Ok(cropped.new_frames)
}

/// Result of an in-memory WAV crop.
pub(crate) struct CroppedWav {
    pub(crate) bytes: Vec<u8>,
    pub(crate) new_frames: u64,
    pub(crate) sample_rate: u32,
    pub(crate) stereo: bool,
    pub(crate) duration_secs: f32,
}

/// Crop in-memory PCM/float WAV bytes to `[start_secs, end_secs]`,
/// preserving the original sample format and bit depth. Int files
/// (8/16/24/32-bit) round-trip through `i32`; float files through `f32`
/// — hound writes each at the width the spec declares.
pub(crate) fn crop_wav_bytes(bytes: &[u8], start_secs: f32, end_secs: f32) -> Result<CroppedWav> {
    let reader = hound::WavReader::new(Cursor::new(bytes)).context("parsing WAV for trim")?;
    let spec = reader.spec();
    let total_frames = reader.duration() as u64;
    let sr = spec.sample_rate as f32;

    let start_frame = ((start_secs * sr).floor() as i64).max(0) as u64;
    let end_frame = (((end_secs * sr).floor() as i64).max(0) as u64).min(total_frames);
    if start_frame >= end_frame {
        return Err(anyhow!(
            "computed empty trim range (start={start_frame}, end={end_frame})"
        ));
    }
    let new_frames = end_frame - start_frame;
    let channels = spec.channels.max(1) as u64;
    let skip = (start_frame * channels) as usize;
    let take = (new_frames * channels) as usize;

    let mut out: Vec<u8> = Vec::new();
    {
        let mut w =
            hound::WavWriter::new(Cursor::new(&mut out), spec).context("creating in-memory WAV")?;
        match spec.sample_format {
            hound::SampleFormat::Int => {
                for (i, s) in reader.into_samples::<i32>().enumerate() {
                    if i < skip {
                        continue;
                    }
                    if i >= skip + take {
                        break;
                    }
                    w.write_sample(s.context("reading int sample")?)
                        .context("writing int sample")?;
                }
            }
            hound::SampleFormat::Float => {
                for (i, s) in reader.into_samples::<f32>().enumerate() {
                    if i < skip {
                        continue;
                    }
                    if i >= skip + take {
                        break;
                    }
                    w.write_sample(s.context("reading float sample")?)
                        .context("writing float sample")?;
                }
            }
        }
        w.finalize().context("finalising in-memory WAV")?;
    }

    Ok(CroppedWav {
        bytes: out,
        new_frames,
        sample_rate: spec.sample_rate,
        stereo: spec.channels >= 2,
        duration_secs: new_frames as f32 / sr,
    })
}

/// Crop a single 16-bit PCM WAV file in place. Atomic via `.tmp`
/// sibling + `rename`; on any error the original file is left
/// untouched. Returns `(original_frame_count, new_frame_count)`.
fn trim_wav_atomic(path: &Path, start_secs: f32, end_secs: f32) -> Result<(u64, u64)> {
    let reader =
        hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spec = reader.spec();
    let total_frames = reader.duration() as u64;
    let sr = spec.sample_rate as f32;

    let start_frame = ((start_secs * sr).floor() as i64).max(0) as u64;
    let end_frame = ((end_secs * sr).floor() as i64).max(0) as u64;
    let end_frame = end_frame.min(total_frames);
    if start_frame >= end_frame {
        return Err(anyhow!(
            "computed empty range for {} (start={start_frame}, end={end_frame})",
            path.display()
        ));
    }
    let new_frames = end_frame - start_frame;

    // Read the slice we want to keep. hound doesn't expose a seek to
    // a frame offset for the typed sample iterator, so we walk the
    // full sample stream and skip / take. WAV reads are sequential
    // and fast — this isn't a hot path.
    let channels = spec.channels.max(1) as u64;
    let skip_samples = (start_frame * channels) as usize;
    let take_samples = (new_frames * channels) as usize;
    let mut samples_iter = reader.into_samples::<i16>();
    let mut kept: Vec<i16> = Vec::with_capacity(take_samples);
    for (i, s) in samples_iter.by_ref().enumerate() {
        if i < skip_samples {
            continue;
        }
        if kept.len() >= take_samples {
            break;
        }
        kept.push(s.with_context(|| format!("reading sample {i} from {}", path.display()))?);
    }
    drop(samples_iter);

    // Write to a sibling .tmp, then rename over the original. If the
    // rename fails the .tmp gets cleaned up so we don't leave debris.
    let tmp_path = path.with_extension("wav.tmp");
    {
        let mut writer = hound::WavWriter::create(&tmp_path, spec)
            .with_context(|| format!("creating {}", tmp_path.display()))?;
        for s in &kept {
            writer
                .write_sample(*s)
                .with_context(|| format!("writing samples to {}", tmp_path.display()))?;
        }
        writer
            .finalize()
            .with_context(|| format!("finalising {}", tmp_path.display()))?;
    }
    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(anyhow::Error::new(e).context(format!(
            "renaming {} → {}",
            tmp_path.display(),
            path.display()
        )));
    }

    Ok((total_frames, new_frames))
}

/// Compute a small peak-table for the project's "reference" WAV — the
/// Suno mixdown if present, else the first track. Used by the trim
/// panel to display a thumbnail behind the start/end markers. Returns
/// `(peaks, total_secs)`. `peaks` are `(min, max)` pairs in `[-1, 1]`,
/// `bin_count` entries.
///
/// The choice of reference is intentional: the user is picking a range
/// for the *whole project*, so showing one canonical track (preferably
/// the mixdown, which represents the full song) gives the most useful
/// visual cue. Per-stem waveforms would clutter without adding info.
pub fn reference_waveform(project: &Project, bin_count: usize) -> Result<(Vec<(f32, f32)>, f32)> {
    let rel = project
        .suno_mixdown_path
        .clone()
        .or_else(|| project.tracks.first().map(|t| t.file.clone()))
        .ok_or_else(|| anyhow!("project has no tracks and no mixdown"))?;
    let abs = project.root.join(&rel);
    let mut reader =
        hound::WavReader::open(&abs).with_context(|| format!("opening {}", abs.display()))?;
    let spec = reader.spec();
    let total_frames = reader.duration() as usize;
    let total_secs = total_frames as f32 / spec.sample_rate.max(1) as f32;
    if bin_count == 0 || total_frames == 0 {
        return Ok((Vec::new(), total_secs));
    }
    let denom = i16::MAX as f32;
    let channels = spec.channels.max(1) as usize;
    let frames_per_bin = total_frames.div_ceil(bin_count).max(1);
    let mut peaks = Vec::with_capacity(bin_count);

    let mut samples_iter = reader.samples::<i16>();
    let mut bin_min = 0.0f32;
    let mut bin_max = 0.0f32;
    let mut frames_in_bin = 0usize;

    for f in 0..total_frames {
        // Take `channels` samples, mix to mono via mean.
        let mut s_sum = 0.0f32;
        let mut got = 0usize;
        for _ in 0..channels {
            match samples_iter.next() {
                Some(Ok(s)) => {
                    s_sum += s as f32 / denom;
                    got += 1;
                }
                _ => break,
            }
        }
        if got == 0 {
            break;
        }
        let s = s_sum / got as f32;
        if s < bin_min {
            bin_min = s;
        }
        if s > bin_max {
            bin_max = s;
        }
        frames_in_bin += 1;
        if frames_in_bin >= frames_per_bin {
            peaks.push((bin_min, bin_max));
            bin_min = 0.0;
            bin_max = 0.0;
            frames_in_bin = 0;
            if peaks.len() == bin_count {
                break;
            }
        }
        let _ = f;
    }
    if frames_in_bin > 0 && peaks.len() < bin_count {
        peaks.push((bin_min, bin_max));
    }
    Ok((peaks, total_secs))
}

/// `mm:ss.mmm` → seconds. Lenient: also accepts `ss.mmm` and bare seconds.
/// Returns `None` on parse error.
pub fn parse_time_secs(s: &str) -> Option<f32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (mins, rest) = if let Some(pos) = s.find(':') {
        (s[..pos].parse::<u32>().ok()?, &s[pos + 1..])
    } else {
        (0u32, s)
    };
    let secs_f: f32 = rest.parse().ok()?;
    if !secs_f.is_finite() || secs_f < 0.0 {
        return None;
    }
    Some(mins as f32 * 60.0 + secs_f)
}

/// Seconds → `mm:ss.mmm` for display.
pub fn format_time_secs(secs: f32) -> String {
    let secs = secs.max(0.0);
    let total_ms = (secs * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let s = (total_ms / 1000) % 60;
    let m = total_ms / 60_000;
    format!("{m:02}:{s:02}.{ms:03}")
}

// ------ helpers used only at the suno_import level (kept here so the
// trim module owns its time-format conventions). ------
#[allow(dead_code)]
pub(crate) const TRIM_TRACKS_DIR: &str = TRACKS_DIR;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_time_accepts_mm_ss_mmm() {
        assert!((parse_time_secs("01:30.500").unwrap() - 90.5).abs() < 1e-3);
    }

    #[test]
    fn parse_time_accepts_bare_seconds() {
        assert!((parse_time_secs("5.25").unwrap() - 5.25).abs() < 1e-3);
    }

    #[test]
    fn parse_time_rejects_negative() {
        assert!(parse_time_secs("-5").is_none());
    }

    #[test]
    fn parse_time_rejects_garbage() {
        assert!(parse_time_secs("nonsense").is_none());
        assert!(parse_time_secs("").is_none());
    }

    #[test]
    fn format_time_round_trips_via_parse() {
        for s in [0.0, 5.0, 12.345, 90.5, 3600.0] {
            let formatted = format_time_secs(s);
            let back = parse_time_secs(&formatted).unwrap();
            assert!((back - s).abs() < 1e-3, "{s} → {formatted} → {back}");
        }
    }

    #[test]
    fn format_time_zero_seconds() {
        assert_eq!(format_time_secs(0.0), "00:00.000");
    }
}

#[cfg(test)]
mod tib_trim_tests {
    //! TBSS-FR-0007 phase 2c step 4: destructive trim over a `.tib`
    //! writes a new revision + repoints current + FIFO-5 prunes, all
    //! reversibly — no in-place WAV overwrite.
    use super::*;
    use crate::project::{Project, Track, TrackSource};
    use crate::telemetry::TelemetryProfile;
    use crate::tib::{RevKind, TibDb};
    use hound::{SampleFormat, WavSpec, WavWriter};
    use std::path::PathBuf;

    fn wav_bytes(frames: u32, rate: u32, stereo: bool) -> Vec<u8> {
        let channels = if stereo { 2 } else { 1 };
        let spec = WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = WavWriter::new(Cursor::new(&mut buf), spec).unwrap();
            for i in 0..frames {
                for _ in 0..channels {
                    w.write_sample((i % 100) as i16).unwrap();
                }
            }
            w.finalize().unwrap();
        }
        buf
    }

    fn scratch(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("tbss-trim-{}-{}.tib", name, std::process::id()));
        cleanup(&p);
        p
    }
    fn cleanup(p: &Path) {
        for s in ["", "-wal", "-shm"] {
            let mut q = p.as_os_str().to_os_string();
            q.push(s);
            let _ = std::fs::remove_file(PathBuf::from(q));
        }
    }

    fn track(id: &str) -> Track {
        Track {
            id: id.into(),
            name: id.into(),
            file: String::new(),
            mute: false,
            gain_db: 0.0,
            sample_rate: 1000,
            channel_source: None,
            duration_secs: 1.0,
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

    fn seed_track(db: &TibDb, id: &str) -> i64 {
        db.insert_stem(&format!("stem-{id}"), id, 0).unwrap();
        db.insert_track(id, &format!("stem-{id}"), id, 0).unwrap();
        let wav = wav_bytes(1000, 1000, true); // exactly 1.0 s
        let rid = db
            .insert_revision(id, RevKind::Orig, "import", 1000, true, 1.0, &wav)
            .unwrap();
        db.set_current_rev(id, rid).unwrap();
        rid
    }

    #[test]
    fn trim_tib_writes_destructive_revision_and_crops() {
        let path = scratch("crop");
        let mut db = TibDb::create(&path).unwrap();
        let orig = seed_track(&db, "t1");

        let mut proj = Project::new("P", path.clone());
        proj.tracks.push(track("t1"));

        let report = trim_project_tib(&mut proj, &mut db, 0.2, 0.8).unwrap();
        assert_eq!(report.trimmed_count, 1);
        assert!(report.failures.is_empty());

        // current_rev moved off the orig onto a new destructive row.
        let cur = db.current_rev_id("t1").unwrap().unwrap();
        assert_ne!(cur, orig, "current moved off orig");
        assert_eq!(
            db.revision_count("t1", RevKind::Orig).unwrap(),
            1,
            "orig preserved (rollback target)"
        );
        assert_eq!(db.revision_count("t1", RevKind::Destructive).unwrap(), 1);

        // Cropped audio is 0.6 s = 600 frames at 1000 Hz.
        let cropped = db.read_current_audio("t1").unwrap();
        let r = hound::WavReader::new(Cursor::new(&cropped)).unwrap();
        assert_eq!(r.duration(), 600);
        assert!((proj.tracks[0].duration_secs - 0.6).abs() < 1e-3);

        // The orig is still byte-recoverable by repointing (no copy).
        db.set_current_rev("t1", orig).unwrap();
        let back = db.read_current_audio("t1").unwrap();
        assert_eq!(
            hound::WavReader::new(Cursor::new(&back))
                .unwrap()
                .duration(),
            1000,
            "rolling back to orig restores full length"
        );

        cleanup(&path);
    }

    #[test]
    fn trim_tib_prunes_to_five_destructive() {
        let path = scratch("prune");
        let mut db = TibDb::create(&path).unwrap();
        seed_track(&db, "t1");

        let mut proj = Project::new("P", path.clone());
        proj.tracks.push(track("t1"));

        // Six successive trims, each cropping 10% off the end — ranges
        // stay non-empty, and the 6th commit FIFO-prunes the oldest.
        for _ in 0..6 {
            let dur = proj.tracks[0].duration_secs;
            trim_project_tib(&mut proj, &mut db, 0.0, (dur * 0.9).max(0.05)).unwrap();
        }
        assert_eq!(
            db.revision_count("t1", RevKind::Orig).unwrap(),
            1,
            "orig never pruned"
        );
        assert_eq!(
            db.revision_count("t1", RevKind::Destructive).unwrap(),
            5,
            "FIFO-5 keeps the five newest destructive revisions"
        );
        cleanup(&path);
    }

    #[test]
    fn trim_tib_rejects_bad_range() {
        let path = scratch("badrange");
        let mut db = TibDb::create(&path).unwrap();
        seed_track(&db, "t1");
        let mut proj = Project::new("P", path.clone());
        proj.tracks.push(track("t1"));
        assert!(trim_project_tib(&mut proj, &mut db, 0.8, 0.2).is_err());
        cleanup(&path);
    }
}
