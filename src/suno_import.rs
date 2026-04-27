//! Suno stem-bundle ingestion.
//!
//! Suno's Pro/Premier export ships either as a folder of `*.wav` files
//! (per-stem download) or as a "Download All" zip archive. Filenames are
//! lowercase hints — `vocals.wav`, `drums.wav`, `bass.wav`, etc. — and
//! the schema is not officially published, so the matcher works on
//! case-insensitive substrings rather than exact names.
//!
//! Import is **lenient** — a malformed entry never aborts the whole
//! ingest, it gets skipped and noted in the per-import log file at
//! `%APPDATA%\TinyBooth Sound Studio\logs\import-<timestamp>.log`.
//! Callers always receive an [`ImportOutcome`] with a populated
//! `summary`, `log_path`, and either a built [`Project`] or `None`.
//!
//! Out of scope here: MP3 ingestion, online stem fetch via unofficial
//! Suno APIs, null-test against an embedded master.

use chrono::Utc;
use std::fs;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::project::{Project, StemRole, Track, TrackSource, MANIFEST_NAME, TRACKS_DIR};
use crate::suno_meta::{read_wav_session, SunoSession};

/// Outcome of an import attempt. Always carries a populated `summary`
/// (shown to the user in the modal) and a `log_path` (where every
/// per-entry decision was recorded).
pub struct ImportOutcome {
    pub project: Option<Project>,
    pub log_path: PathBuf,
    pub summary: String,
    pub success: bool,
    pub source: String,
}

#[derive(Default)]
struct Counts {
    total_entries: usize,
    skipped_dir: usize,
    skipped_unsafe: usize,
    skipped_non_wav: usize,
    skipped_tempo_locked: usize,
    extract_errors: usize,
    wav_meta_errors: usize,
    kept: usize,
}

struct Detected {
    role: StemRole,
    original_filename: String,
    track_filename: String,
    sample_rate: u32,
    channels: u16,
    duration_secs: f32,
    session: Option<SunoSession>,
}

// ───────────────────── public API ─────────────────────

/// Import every WAV stem in `source_folder` into a brand-new project at
/// `project_root`.
pub fn import_folder(source_folder: &Path, project_root: &Path, project_name: &str) -> ImportOutcome {
    let source = source_folder.display().to_string();
    let mut log = ImportLog::open("folder", project_name);
    log.line(&format!("source folder = {}", source));
    log.line(&format!("project root  = {}", project_root.display()));

    if !source_folder.is_dir() {
        let summary = format!("Source is not a folder:\n  {}", source);
        log.line(&format!("FATAL: {summary}"));
        return ImportOutcome {
            project: None, log_path: log.path.clone(), summary, success: false, source,
        };
    }

    if let Err(e) = prepare_project_dirs(project_root) {
        let summary = format!("Could not create project folders:\n  {}\n  {}",
            project_root.display(), e);
        log.line(&format!("FATAL: {summary}"));
        return ImportOutcome {
            project: None, log_path: log.path.clone(), summary, success: false, source,
        };
    }

    let mut counts = Counts::default();
    let mut detected = Vec::new();

    let entries = match fs::read_dir(source_folder) {
        Ok(it) => it,
        Err(e) => {
            let summary = format!("Could not read folder:\n  {}\n  {}", source, e);
            log.line(&format!("FATAL: {summary}"));
            return ImportOutcome {
                project: None, log_path: log.path.clone(), summary, success: false, source,
            };
        }
    };

    for entry_res in entries {
        counts.total_entries += 1;
        let entry = match entry_res {
            Ok(e) => e,
            Err(e) => {
                log.line(&format!("SKIP (read_dir error): {e}"));
                counts.extract_errors += 1;
                continue;
            }
        };
        let path = entry.path();
        let display = path.display().to_string();
        if path.is_dir() {
            log.line(&format!("SKIP dir: {display}"));
            counts.skipped_dir += 1;
            continue;
        }
        let lower = path.file_name().and_then(|n| n.to_str()).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        if !lower.ends_with(".wav") {
            log.line(&format!("SKIP non-wav: {display}"));
            counts.skipped_non_wav += 1;
            continue;
        }
        if is_tempo_locked(&lower) {
            log.line(&format!("SKIP tempo-locked: {display}"));
            counts.skipped_tempo_locked += 1;
            continue;
        }

        let original = path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let role = match_role(&original);
        let track_filename = match unique_track_filename(project_root, &original) {
            Ok(s) => s,
            Err(e) => {
                log.line(&format!("SKIP (filename collision exhausted): {original} — {e}"));
                counts.extract_errors += 1;
                continue;
            }
        };
        let dest = project_root.join(TRACKS_DIR).join(&track_filename);
        if let Err(e) = fs::copy(&path, &dest) {
            log.line(&format!("SKIP (copy failed): {} -> {} — {e}", path.display(), dest.display()));
            counts.extract_errors += 1;
            continue;
        }
        match read_wav_meta(&dest) {
            Ok(info) => {
                let session = read_wav_session(&dest);
                let session_str = session.as_ref()
                    .map(|s| format!(" suno_epoch={} iso={}", s.epoch, s.iso_timestamp))
                    .unwrap_or_default();
                log.line(&format!(
                    "KEEP: {original} -> {track_filename}  role={:?}  rate={}  ch={}  dur={:.2}s{}",
                    role, info.sample_rate, info.channels, info.duration_secs, session_str,
                ));
                counts.kept += 1;
                detected.push(Detected {
                    role,
                    original_filename: original,
                    track_filename,
                    sample_rate: info.sample_rate,
                    channels: info.channels,
                    duration_secs: info.duration_secs,
                    session,
                });
            }
            Err(e) => {
                log.line(&format!("SKIP (WAV header read failed): {} — {e}", dest.display()));
                let _ = fs::remove_file(&dest);
                counts.wav_meta_errors += 1;
            }
        }
    }

    finalize(log, source, project_root, project_name, counts, detected)
}

/// Same as [`import_folder`] but reads from a zip archive — Suno's
/// "Download All" delivery format.
pub fn import_zip(zip_path: &Path, project_root: &Path, project_name: &str) -> ImportOutcome {
    let source = zip_path.display().to_string();
    let mut log = ImportLog::open("zip", project_name);
    log.line(&format!("source zip   = {}", source));
    log.line(&format!("project root = {}", project_root.display()));

    let file = match fs::File::open(zip_path) {
        Ok(f) => f,
        Err(e) => {
            let summary = format!("Could not open zip:\n  {}\n  {}", source, e);
            log.line(&format!("FATAL: {summary}"));
            return ImportOutcome {
                project: None, log_path: log.path.clone(), summary, success: false, source,
            };
        }
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => {
            let summary = format!("Could not read zip archive:\n  {}\n  {}", source, e);
            log.line(&format!("FATAL: {summary}"));
            return ImportOutcome {
                project: None, log_path: log.path.clone(), summary, success: false, source,
            };
        }
    };

    log.line(&format!("zip entries  = {}", archive.len()));

    if let Err(e) = prepare_project_dirs(project_root) {
        let summary = format!("Could not create project folders:\n  {}\n  {}",
            project_root.display(), e);
        log.line(&format!("FATAL: {summary}"));
        return ImportOutcome {
            project: None, log_path: log.path.clone(), summary, success: false, source,
        };
    }

    let mut counts = Counts::default();
    let mut detected = Vec::new();

    for i in 0..archive.len() {
        counts.total_entries += 1;
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(e) => {
                log.line(&format!("SKIP (zip entry {i} unreadable): {e}"));
                counts.extract_errors += 1;
                continue;
            }
        };
        // Capture the raw name BEFORE checking enclosed_name so the log
        // shows what was in the archive, not just what was rejected.
        let raw_name = entry.name().to_string();
        if entry.is_dir() {
            log.line(&format!("SKIP dir entry: {raw_name}"));
            counts.skipped_dir += 1;
            continue;
        }
        let entry_name = match entry.enclosed_name() {
            Some(p) => p.to_path_buf(),
            None => {
                log.line(&format!("SKIP unsafe path: {raw_name}"));
                counts.skipped_unsafe += 1;
                continue;
            }
        };
        let lower = entry_name.file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        if !lower.ends_with(".wav") {
            log.line(&format!("SKIP non-wav: {raw_name}"));
            counts.skipped_non_wav += 1;
            continue;
        }
        if is_tempo_locked(&lower) {
            log.line(&format!("SKIP tempo-locked: {raw_name}"));
            counts.skipped_tempo_locked += 1;
            continue;
        }

        let original = entry_name.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let role = match_role(&original);
        let track_filename = match unique_track_filename(project_root, &original) {
            Ok(s) => s,
            Err(e) => {
                log.line(&format!("SKIP (filename collision exhausted): {original} — {e}"));
                counts.extract_errors += 1;
                continue;
            }
        };
        let dest = project_root.join(TRACKS_DIR).join(&track_filename);
        if let Err(e) = copy_zip_entry(&mut entry, &dest) {
            log.line(&format!("SKIP (extract failed): {raw_name} -> {} — {e}", dest.display()));
            counts.extract_errors += 1;
            continue;
        }
        match read_wav_meta(&dest) {
            Ok(info) => {
                let session = read_wav_session(&dest);
                let session_str = session.as_ref()
                    .map(|s| format!(" suno_epoch={} iso={}", s.epoch, s.iso_timestamp))
                    .unwrap_or_default();
                log.line(&format!(
                    "KEEP: {raw_name} -> {track_filename}  role={:?}  rate={}  ch={}  dur={:.2}s{}",
                    role, info.sample_rate, info.channels, info.duration_secs, session_str,
                ));
                counts.kept += 1;
                detected.push(Detected {
                    role,
                    original_filename: original,
                    track_filename,
                    sample_rate: info.sample_rate,
                    channels: info.channels,
                    duration_secs: info.duration_secs,
                    session,
                });
            }
            Err(e) => {
                log.line(&format!("SKIP (WAV header read failed): {} — {e}", dest.display()));
                let _ = fs::remove_file(&dest);
                counts.wav_meta_errors += 1;
            }
        }
    }

    finalize(log, source, project_root, project_name, counts, detected)
}

// ────────────────────── pending-import state ──────────────────────

/// Folder vs. zip kind. The conflict-resolution path needs to remember
/// which import function to re-invoke after the user says Replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind { Folder, Zip }

/// An import that has been deferred because the target project root
/// already contains a manifest with a matching Suno session epoch.
/// Held on `app.import_conflict` while the modal is up.
#[derive(Debug, Clone)]
pub struct PendingImport {
    pub kind: ImportKind,
    pub source: std::path::PathBuf,
    pub project_root: std::path::PathBuf,
    pub project_name: String,
    pub probe: PreImportProbe,
}

// ────────────────────── duplicate-detection probe ──────────────────────

/// Outcome of pre-import inspection: the new bundle's session epoch
/// (if any) plus whether an existing project at the proposed root
/// already contains that same session.
#[derive(Debug, Clone)]
pub struct PreImportProbe {
    pub new_session_epoch: Option<i64>,
    pub new_session_iso: Option<String>,
    pub existing_session_epoch: Option<i64>,
    pub existing_project_name: Option<String>,
    pub existing_track_count: usize,
    pub existing_session_ordinal: Option<u32>,
}

impl PreImportProbe {
    pub fn is_duplicate(&self) -> bool {
        match (self.new_session_epoch, self.existing_session_epoch) {
            (Some(n), Some(e)) => n == e,
            _ => false,
        }
    }
}

/// Inspect a folder of stems (no extraction yet): find the first
/// eligible WAV, read its session epoch, then check whether a project
/// already exists at `project_root` with matching epoch.
pub fn probe_folder(source: &Path, project_root: &Path) -> PreImportProbe {
    let new = first_session_in_folder(source);
    let existing = existing_session_at_root(project_root);
    finalize_probe(new, existing)
}

/// Same probe for a zip archive — reads the first eligible WAV's bytes
/// to a temp file (we need a Seek-able stream for the RIFF walker).
pub fn probe_zip(source: &Path, project_root: &Path) -> PreImportProbe {
    let new = first_session_in_zip(source);
    let existing = existing_session_at_root(project_root);
    finalize_probe(new, existing)
}

fn finalize_probe(
    new: Option<SunoSession>,
    existing: Option<(SunoSession, String, usize, Option<u32>)>,
) -> PreImportProbe {
    PreImportProbe {
        new_session_epoch: new.as_ref().map(|s| s.epoch),
        new_session_iso: new.map(|s| s.iso_timestamp),
        existing_session_epoch: existing.as_ref().map(|(s, _, _, _)| s.epoch),
        existing_project_name: existing.as_ref().map(|(_, n, _, _)| n.clone()),
        existing_track_count: existing.as_ref().map(|(_, _, c, _)| *c).unwrap_or(0),
        existing_session_ordinal: existing.and_then(|(_, _, _, o)| o),
    }
}

fn first_session_in_folder(folder: &Path) -> Option<SunoSession> {
    let entries = fs::read_dir(folder).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_file() { continue; }
        let lower = p.file_name()?.to_str()?.to_ascii_lowercase();
        if !lower.ends_with(".wav") || is_tempo_locked(&lower) { continue; }
        if let Some(s) = read_wav_session(&p) {
            return Some(s);
        }
    }
    None
}

fn first_session_in_zip(zip_path: &Path) -> Option<SunoSession> {
    let f = fs::File::open(zip_path).ok()?;
    let mut archive = zip::ZipArchive::new(f).ok()?;
    let tmp = std::env::temp_dir().join(format!("tinybooth-probe-{}.wav", std::process::id()));
    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) { Ok(e) => e, Err(_) => continue };
        if entry.is_dir() { continue; }
        let name = match entry.enclosed_name() { Some(p) => p.to_path_buf(), None => continue };
        let lower = match name.file_name().and_then(|n| n.to_str()) {
            Some(s) => s.to_ascii_lowercase(),
            None => continue,
        };
        if !lower.ends_with(".wav") || is_tempo_locked(&lower) { continue; }
        // Extract just enough to read the header chunks. RIFF metadata
        // sits at the front; copying the whole file would be wasteful
        // but copying a known prefix is fragile across encoders, so we
        // just take the whole thing — these files are 30–50 MB and the
        // probe runs once per import.
        if copy_zip_entry(&mut entry, &tmp).is_err() { continue; }
        let session = read_wav_session(&tmp);
        let _ = fs::remove_file(&tmp);
        if session.is_some() { return session; }
    }
    None
}

/// Read an existing project's manifest (if any) and return the most
/// common Suno session epoch among its tracks alongside the project
/// name and track count. Returns None if no manifest, no Suno tracks,
/// or any read error.
fn existing_session_at_root(project_root: &Path) -> Option<(SunoSession, String, usize, Option<u32>)> {
    let manifest = project_root.join(MANIFEST_NAME);
    if !manifest.exists() { return None; }
    let s = fs::read_to_string(&manifest).ok()?;
    let proj: Project = serde_json::from_str(&s).ok()?;

    // Pick the first track with a session_epoch — every Suno-imported
    // track shares the epoch within one render, so the first hit is
    // representative.
    let mut chosen: Option<(i64, Option<u32>)> = None;
    for t in &proj.tracks {
        if let TrackSource::SunoStem { session_epoch: Some(e), session_ordinal, .. } = &t.source {
            chosen = Some((*e, *session_ordinal));
            break;
        }
    }
    let (epoch, ordinal) = chosen?;

    // Build a SunoSession with placeholder iso/provenance — we only
    // really need the epoch for comparison.
    let iso = chrono::DateTime::<chrono::Utc>::from_timestamp(epoch, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default();
    Some((
        SunoSession { epoch, iso_timestamp: iso, provenance: String::new() },
        proj.name,
        proj.tracks.len(),
        ordinal,
    ))
}

/// Wipe everything under `<project_root>/tracks/` and the manifest.
/// Caller invokes this when the user picks "Replace" in the duplicate-
/// import modal.
pub fn wipe_project_root(project_root: &Path) -> std::io::Result<()> {
    let tracks = project_root.join(TRACKS_DIR);
    if tracks.is_dir() {
        fs::remove_dir_all(&tracks)?;
    }
    let manifest = project_root.join(MANIFEST_NAME);
    if manifest.is_file() {
        let _ = fs::remove_file(&manifest);
    }
    Ok(())
}

// ──────────────────────────── helpers ────────────────────────────

fn finalize(
    mut log: ImportLog,
    source: String,
    project_root: &Path,
    project_name: &str,
    counts: Counts,
    detected: Vec<Detected>,
) -> ImportOutcome {
    log.line("");
    log.line("─── summary ───────────────────────");
    log.line(&format!("entries scanned     = {}", counts.total_entries));
    log.line(&format!("kept                = {}", counts.kept));
    log.line(&format!("skipped (dir)       = {}", counts.skipped_dir));
    log.line(&format!("skipped (unsafe)    = {}", counts.skipped_unsafe));
    log.line(&format!("skipped (non-wav)   = {}", counts.skipped_non_wav));
    log.line(&format!("skipped (tempo lk)  = {}", counts.skipped_tempo_locked));
    log.line(&format!("errors (extract)    = {}", counts.extract_errors));
    log.line(&format!("errors (wav meta)   = {}", counts.wav_meta_errors));

    if detected.is_empty() {
        let summary = format!(
            "No WAV stems were imported.\n\n\
             Scanned {} entr{}, kept 0.\n\
             • {} non-WAV file(s)\n\
             • {} Tempo-Locked variant(s) (excluded by design)\n\
             • {} extract error(s)\n\
             • {} WAV-header error(s)\n\n\
             v1 of the ingester is WAV-only — re-download as WAV from Suno \
             if your bundle is MP3.\n\n\
             Full log:\n  {}",
            counts.total_entries,
            if counts.total_entries == 1 { "y" } else { "ies" },
            counts.skipped_non_wav,
            counts.skipped_tempo_locked,
            counts.extract_errors,
            counts.wav_meta_errors,
            log.path.display(),
        );
        log.line(&format!("OUTCOME: empty — {summary}"));
        log.flush();
        return ImportOutcome {
            project: None, log_path: log.path.clone(), summary,
            success: false, source,
        };
    }

    let project = match build_project(project_root, project_name, detected) {
        Ok(p) => p,
        Err(e) => {
            let summary = format!("Stems extracted but project save failed:\n  {e}\n\nFull log:\n  {}", log.path.display());
            log.line(&format!("FATAL on save: {e}"));
            log.flush();
            return ImportOutcome {
                project: None, log_path: log.path.clone(), summary,
                success: false, source,
            };
        }
    };

    // Pull the Suno session info off the first track that has it
    // (every Suno track in this import shares the session).
    let session_line = project.tracks.iter().find_map(|t| {
        if let TrackSource::SunoStem { session_epoch, session_ordinal, .. } = &t.source {
            session_epoch.map(|e| {
                let iso = chrono::DateTime::<chrono::Utc>::from_timestamp(e, 0)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_else(|| e.to_string());
                let ord = session_ordinal.map(|o| format!(" (import #{o})")).unwrap_or_default();
                format!("\n\nSuno session: epoch {e}  ({iso}){ord}")
            })
        } else { None }
    }).unwrap_or_default();

    let summary = format!(
        "Imported {} stem(s) into:\n  {}{}\n\nLog:\n  {}",
        project.tracks.len(),
        project.manifest_path().display(),
        session_line,
        log.path.display(),
    );
    log.line(&format!("OUTCOME: success — {} tracks", project.tracks.len()));
    log.flush();

    ImportOutcome {
        project: Some(project),
        log_path: log.path.clone(),
        summary,
        success: true,
        source,
    }
}

fn build_project(project_root: &Path, name: &str, detected: Vec<Detected>) -> anyhow::Result<Project> {
    // If a project.tinybooth already exists at this root we ALWAYS create
    // a fresh project here (the duplicate-detection path runs before us
    // and has already cleared the old contents on Replace). Old manifest
    // is overwritten.
    let mut project = Project {
        version: 1,
        name: name.to_string(),
        created: Utc::now(),
        tracks: Vec::with_capacity(detected.len()),
        master_gain_db: 0.0,
        master_gain_automation: None,
        next_suno_ordinal: 1,
        corrections_disabled: false,
        default_correction: None,
        root: project_root.to_path_buf(),
    };
    let ordinal = project.next_suno_ordinal;
    project.next_suno_ordinal = ordinal.saturating_add(1);

    for (i, d) in detected.into_iter().enumerate() {
        let id = format!("track-{:03}", i + 1);
        let display_name = d.role.label().to_string();
        let (session_epoch, provenance) = match d.session.as_ref() {
            Some(s) => (Some(s.epoch), Some(s.provenance.clone())),
            None => (None, None),
        };
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
                session_epoch,
                session_ordinal: Some(ordinal),
                provenance,
            },
            correction: None,
            gain_automation: None,
        });
    }
    project.save()?;
    Ok(project)
}

fn prepare_project_dirs(project_root: &Path) -> std::io::Result<()> {
    fs::create_dir_all(project_root.join(TRACKS_DIR))
}

fn is_tempo_locked(lower_name: &str) -> bool {
    lower_name.contains("tempo") && lower_name.contains("lock")
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
    if has("guitar") { return StemRole::ElectricGuitar; }
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

fn read_wav_meta(path: &Path) -> anyhow::Result<WavMeta> {
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let frames = reader.duration() as f32;
    let dur = if spec.sample_rate > 0 { frames / spec.sample_rate as f32 } else { 0.0 };
    Ok(WavMeta {
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        duration_secs: dur,
    })
}

fn unique_track_filename(project_root: &Path, source_name: &str) -> anyhow::Result<String> {
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
    anyhow::bail!("could not generate a unique filename for '{source_name}'")
}

fn copy_zip_entry<R: Read>(entry: &mut R, dest: &Path) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() { fs::create_dir_all(parent)?; }
    let mut out = fs::File::create(dest)?;
    std::io::copy(entry, &mut out)?;
    Ok(())
}

// ───────────────────── per-import log file ─────────────────────

struct ImportLog {
    path: PathBuf,
    writer: Option<BufWriter<fs::File>>,
}

impl ImportLog {
    fn open(mode: &str, project_name: &str) -> Self {
        let dir = Config::dir().unwrap_or_else(|| PathBuf::from(".")).join("logs");
        let _ = fs::create_dir_all(&dir);
        let safe_name: String = project_name.chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let path = dir.join(format!("import-{mode}-{safe_name}-{ts}.log"));
        let writer = fs::File::create(&path).ok().map(BufWriter::new);
        let mut me = Self { path, writer };
        me.line(&format!("TinyBooth Sound Studio import log — v{}", env!("CARGO_PKG_VERSION")));
        me.line(&format!("started   = {}", chrono::Local::now().to_rfc3339()));
        me.line(&format!("mode      = {mode}"));
        me
    }

    fn line(&mut self, msg: &str) {
        if let Some(w) = self.writer.as_mut() {
            let _ = writeln!(w, "{}  {msg}", chrono::Local::now().format("%H:%M:%S%.3f"));
        }
    }

    fn flush(&mut self) {
        if let Some(w) = self.writer.as_mut() {
            let _ = w.flush();
        }
    }
}

impl Drop for ImportLog {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Where import logs live. Used by tooling that wants to surface the
/// folder; the import-result modal uses `outcome.log_path.parent()` so
/// it can show the exact run.
#[allow(dead_code)]
pub fn log_dir() -> PathBuf {
    Config::dir().unwrap_or_else(|| PathBuf::from(".")).join("logs")
}
