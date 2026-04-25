//! Suno stem-bundle ingestion.
//!
//! Suno's Pro/Premier export ships either as a folder of `*.wav` files
//! (per-stem download) or as a "Download All" zip archive. Filenames are
//! lowercase hints — `vocals.wav`, `drums.wav`, `bass.wav`, etc. — and
//! the schema is not officially published, so the matcher works on
//! case-insensitive substrings rather than exact names.
//!
//! What this module does:
//! 1. Walks a folder or streams a zip, collecting every `.wav` entry.
//! 2. Skips Tempo-Locked variants (those are time-stretched and won't
//!    sum back to the master).
//! 3. Extracts each retained file into the new project's `tracks/`
//!    directory.
//! 4. Reads the WAV header for sample rate, bit depth, channel count,
//!    duration. Filename is treated as advisory only.
//! 5. Tags each track with a `StemRole` derived from the filename.
//! 6. Builds a fresh `Project` with one `Track` per stem and saves.
//!
//! Out of scope here: MP3 ingestion, online stem fetch via unofficial
//! Suno APIs, null-test against an embedded master.

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use std::fs;
use std::io::Read;
use std::path::Path;

use crate::project::{Project, StemRole, Track, TrackSource, TRACKS_DIR};

/// One detected stem, ready to commit to a project.
struct Detected {
    role: StemRole,
    original_filename: String,
    track_filename: String,
    sample_rate: u32,
    channels: u16,
    duration_secs: f32,
}

/// Import every WAV stem in `source_folder` into a brand-new project at
/// `project_root`. The project is saved to disk; returns the loaded
/// `Project` ready for the UI to swap in.
pub fn import_folder(source_folder: &Path, project_root: &Path, project_name: &str) -> Result<Project> {
    if !source_folder.is_dir() {
        return Err(anyhow!("'{}' is not a folder", source_folder.display()));
    }
    prepare_project_dirs(project_root)?;

    let mut detected = Vec::new();
    for entry in fs::read_dir(source_folder).with_context(|| format!("reading {}", source_folder.display()))? {
        let entry = entry?;
        let path = entry.path();
        if !is_eligible_wav(&path) {
            continue;
        }
        let original = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let role = match_role(&original);
        let track_filename = unique_track_filename(project_root, &original)?;
        let dest = project_root.join(TRACKS_DIR).join(&track_filename);
        fs::copy(&path, &dest).with_context(|| format!("copying {}", path.display()))?;
        let info = read_wav_meta(&dest)?;
        detected.push(Detected {
            role,
            original_filename: original,
            track_filename,
            sample_rate: info.sample_rate,
            channels: info.channels,
            duration_secs: info.duration_secs,
        });
    }
    if detected.is_empty() {
        return Err(anyhow!(
            "no WAV stems found in '{}' (and no Tempo-Locked variants are imported)",
            source_folder.display()
        ));
    }
    finish_project(project_root, project_name, detected)
}

/// Same as [`import_folder`] but reads from a zip archive — Suno's
/// "Download All" delivery format.
pub fn import_zip(zip_path: &Path, project_root: &Path, project_name: &str) -> Result<Project> {
    let file = fs::File::open(zip_path).with_context(|| format!("opening {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("reading zip archive")?;
    prepare_project_dirs(project_root)?;

    let mut detected = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).context("reading zip entry")?;
        if entry.is_dir() { continue; }
        let entry_name = entry
            .enclosed_name()
            .ok_or_else(|| anyhow!("zip contains an unsafe path"))?
            .to_path_buf();
        if !is_eligible_wav(&entry_name) {
            continue;
        }
        let original = entry_name
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let role = match_role(&original);
        let track_filename = unique_track_filename(project_root, &original)?;
        let dest = project_root.join(TRACKS_DIR).join(&track_filename);
        copy_zip_entry(&mut entry, &dest)?;
        let info = read_wav_meta(&dest)?;
        detected.push(Detected {
            role,
            original_filename: original,
            track_filename,
            sample_rate: info.sample_rate,
            channels: info.channels,
            duration_secs: info.duration_secs,
        });
    }
    if detected.is_empty() {
        return Err(anyhow!(
            "no WAV stems found in '{}' (Tempo-Locked variants are excluded)",
            zip_path.display()
        ));
    }
    finish_project(project_root, project_name, detected)
}

// ──────────────────────────── helpers ────────────────────────────

fn prepare_project_dirs(project_root: &Path) -> Result<()> {
    fs::create_dir_all(project_root.join(TRACKS_DIR)).context("creating project dirs")
}

/// Decide whether a file name represents a WAV we want. Excludes
/// Tempo-Locked variants outright (Suno markets these but they're
/// time-stretched and won't align with the original master).
fn is_eligible_wav(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else { return false };
    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".wav") { return false; }
    if lower.contains("tempo") && lower.contains("lock") { return false; }
    true
}

/// Case-insensitive substring matcher → `StemRole`. Designed to be
/// permissive — Suno's filenames are advisory, not contractual.
pub fn match_role(filename: &str) -> StemRole {
    let s = filename.to_ascii_lowercase();
    let has = |needle: &str| s.contains(needle);
    if has("vocal") && has("back") { return StemRole::BackingVocals; }
    if has("vocal") { return StemRole::Vocals; }
    if has("drum") { return StemRole::Drums; }
    if has("bass") { return StemRole::Bass; }
    if has("electric") && has("guitar") { return StemRole::ElectricGuitar; }
    if has("acoustic") && has("guitar") { return StemRole::AcousticGuitar; }
    if has("guitar") { return StemRole::ElectricGuitar; } // generic guitar → electric
    if has("piano") || has("key") { return StemRole::Keys; }
    if has("synth") || has("lead") { return StemRole::Synth; }
    if has("pad") || has("chord") { return StemRole::Pads; }
    if has("string") { return StemRole::Strings; }
    if has("brass") || has("wood") { return StemRole::Brass; }
    if has("perc") { return StemRole::Percussion; }
    if has("fx") || has("other") { return StemRole::FxOther; }
    if has("instrumental") { return StemRole::Instrumental; }
    if has("master") || has("mix") || has("final") { return StemRole::Master; }
    StemRole::Unknown
}

struct WavMeta {
    sample_rate: u32,
    channels: u16,
    duration_secs: f32,
}

fn read_wav_meta(path: &Path) -> Result<WavMeta> {
    let reader = hound::WavReader::open(path)
        .with_context(|| format!("reading WAV header on {}", path.display()))?;
    let spec = reader.spec();
    let frames = reader.duration() as f32;
    let dur = if spec.sample_rate > 0 { frames / spec.sample_rate as f32 } else { 0.0 };
    Ok(WavMeta {
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        duration_secs: dur,
    })
}

/// Avoid colliding with an existing track. Suno's lowercase names are
/// short ("drums.wav", "bass.wav") so collisions are common only when
/// importing into a project that already has tracks.
fn unique_track_filename(project_root: &Path, source_name: &str) -> Result<String> {
    let tracks_dir = project_root.join(TRACKS_DIR);
    let candidate = tracks_dir.join(source_name);
    if !candidate.exists() {
        return Ok(source_name.to_string());
    }
    let stem = Path::new(source_name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "track".into());
    let ext = Path::new(source_name)
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "wav".into());
    for n in 2..=999 {
        let cand = format!("{stem}-{n:03}.{ext}");
        if !tracks_dir.join(&cand).exists() {
            return Ok(cand);
        }
    }
    Err(anyhow!("could not generate a unique filename for '{source_name}'"))
}

fn copy_zip_entry<R: Read>(entry: &mut R, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() { fs::create_dir_all(parent)?; }
    let mut out = fs::File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    std::io::copy(entry, &mut out).context("extracting zip entry")?;
    Ok(())
}

fn finish_project(project_root: &Path, name: &str, detected: Vec<Detected>) -> Result<Project> {
    let mut project = Project {
        version: 1,
        name: name.to_string(),
        created: Utc::now(),
        tracks: Vec::with_capacity(detected.len()),
        root: project_root.to_path_buf(),
    };
    for (i, d) in detected.into_iter().enumerate() {
        let id = format!("track-{:03}", i + 1);
        let display_name = d.role.label().to_string();
        project.tracks.push(Track {
            id,
            name: display_name,
            file: format!("{TRACKS_DIR}/{}", d.track_filename),
            mute: false,
            gain_db: 0.0,
            sample_rate: d.sample_rate,
            channel_source: None,
            duration_secs: d.duration_secs,
            profile: None,
            stereo: d.channels >= 2,
            source: TrackSource::SunoStem {
                role: d.role,
                original_filename: d.original_filename,
            },
            correction: None,
        });
    }
    project.save()?;
    Ok(project)
}
