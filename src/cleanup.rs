//! Project-cleanup logic for legacy bugs (v0.4.2).
//!
//! Pre-v0.4.0, recordings were appended to whatever project the user
//! had open at the time of capture — including imported Suno stem
//! projects. The result: a Suno project's `tracks/` folder ended up
//! with `TrackSource::Recorded` entries at the wrong rate, breaking
//! `Player::new`'s uniform-rate check on the next Mix-tab visit.
//!
//! v0.4.0 fixed the recording flow so future captures always land in
//! the dedicated recordings filespace. This module fixes the legacy
//! residue: the **cleanse protocol** scans a Suno-shaped project for
//! `Recorded` orphans and migrates them out into the recordings
//! filespace, preserving every Track field (gain, automation,
//! correction, polarity, etc.) so the user doesn't lose work.
//!
//! Trigger: Mix-tab `show()` runs this once before the lazy player
//! rebuild. Idempotent — returns an empty report when there are no
//! orphans, cheap to call on every visit.
//!
//! Detection signal: `TrackSource::Recorded` inside a project whose
//! `suno_mixdown_path` is `Some(_)`. Deterministic; doesn't depend on
//! length / rate heuristics. Recordings inside non-Suno projects
//! (legacy scratch sessions) are left alone — the user may have
//! intentionally curated those.

use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::project::{Project, TrackSource, TRACKS_DIR};

/// Outcome of a cleanse pass. Surfaced to the user as a status-bar
/// message via [`crate::app::TinyBoothApp`].
#[derive(Debug, Clone, Default)]
pub struct CleanseReport {
    /// Number of `Recorded` orphans successfully migrated out.
    pub moved_count: usize,
    /// Per-orphan failures. Surfaced as a multi-line status so the
    /// user can see which file refused to move and why.
    pub failures: Vec<MigrationFailure>,
    /// True when the recordings project's existing rate doesn't match
    /// at least one migrated track. Future Mix on the recordings
    /// project may fail until the user reorganises — flagged so the
    /// status line can mention it.
    pub recordings_rate_mismatch: bool,
}

#[derive(Debug, Clone)]
pub struct MigrationFailure {
    pub display_name: String,
    pub error: String,
}

impl CleanseReport {
    pub fn is_empty(&self) -> bool {
        self.moved_count == 0 && self.failures.is_empty()
    }

    /// Multi-line summary for the status bar / log.
    pub fn summary(&self) -> String {
        let mut s = if self.failures.is_empty() {
            format!(
                "Cleanse: moved {} stray recording(s) out of this Suno project into Recordings.",
                self.moved_count
            )
        } else {
            format!(
                "Cleanse: moved {}, {} failed.",
                self.moved_count,
                self.failures.len()
            )
        };
        if self.recordings_rate_mismatch {
            s.push_str(
                "\n  ⚠ Some migrated takes don't match the Recordings project's existing rate; \
                 Mix on Recordings may need manual reorganisation.",
            );
        }
        for f in &self.failures {
            s.push_str(&format!("\n  ✗ {}: {}", f.display_name, f.error));
        }
        s
    }
}

/// Scan `project` for `TrackSource::Recorded` entries that ended up
/// inside a Suno-shaped project (the pre-v0.4.0 bug). For each, move
/// the WAV file to the recordings filespace, append a fresh Track row
/// to the recordings manifest, and remove the orphan from `project`.
///
/// `project` is mutated in place — the caller is responsible for
/// calling `project.save()` afterward to persist the removal. The
/// recordings project is loaded + saved internally; the disk and
/// in-memory states stay in lockstep.
///
/// Returns an empty report (and leaves `project` unchanged) when:
///   - the project isn't Suno-shaped (no `suno_mixdown_path`); or
///   - the project has no `Recorded` orphans.
///
/// Both paths are O(n) in track count and do no I/O, so this is
/// cheap to call from `Mix-tab::show()` on every visit.
pub fn cleanse_recordings_in_suno_project(project: &mut Project) -> Result<CleanseReport> {
    if project.suno_mixdown_path.is_none() {
        return Ok(CleanseReport::default());
    }

    // Partition: Recorded → orphans, everything else → keep.
    // Walk by index so we can pull out orphans without breaking the
    // remaining indices for the player rebuild that follows.
    let mut orphans = Vec::new();
    let mut keep = Vec::with_capacity(project.tracks.len());
    for t in project.tracks.drain(..) {
        match &t.source {
            TrackSource::Recorded => orphans.push(t),
            _ => keep.push(t),
        }
    }
    project.tracks = keep;

    if orphans.is_empty() {
        return Ok(CleanseReport::default());
    }

    // Load recordings project for the migration target.
    let mut recordings = Project::open_or_create_recordings()
        .context("opening recordings project for cleanse migration")?;
    let recordings_root = recordings.root.clone();
    let project_root = project.root.clone();
    let recordings_first_rate = recordings.tracks.first().map(|t| t.sample_rate);

    let mut report = CleanseReport::default();

    for mut orphan in orphans {
        // Cloned upfront because `orphan` is moved into recordings on
        // the success path and we still need its name for failure
        // reporting along the way.
        let display_name = orphan.name.clone();
        let src_abs = project_root.join(&orphan.file);

        // Mint a fresh id in the recordings project so we never collide
        // with existing recordings even if `track-001` is in use here.
        let (new_id, dest_abs) = recordings.new_track_slot();
        let new_file_rel = format!("{TRACKS_DIR}/{new_id}.wav");

        // Ensure tracks/ exists under the recordings root.
        if let Err(e) = std::fs::create_dir_all(recordings_root.join(TRACKS_DIR)) {
            report.failures.push(MigrationFailure {
                display_name: display_name.clone(),
                error: format!("creating recordings tracks dir: {e}"),
            });
            continue;
        }

        // Move the WAV. Try rename first; fall back to copy+delete on
        // cross-device errors (recordings filespace lives in %APPDATA%
        // which might be on a different drive than the project).
        if let Err(e) = std::fs::rename(&src_abs, &dest_abs) {
            match std::fs::copy(&src_abs, &dest_abs) {
                Ok(_) => {
                    if let Err(e2) = std::fs::remove_file(&src_abs) {
                        // Copy succeeded but original cleanup failed;
                        // the migration is still valid (the canonical
                        // copy is now in recordings) but flag it.
                        report.failures.push(MigrationFailure {
                            display_name: display_name.clone(),
                            error: format!(
                                "moved to {} but could not delete source {}: {e2}",
                                new_file_rel,
                                src_abs.display()
                            ),
                        });
                    }
                }
                Err(e2) => {
                    report.failures.push(MigrationFailure {
                        display_name: display_name.clone(),
                        error: format!(
                            "could not move {} → {}: rename={e}, copy={e2}",
                            src_abs.display(),
                            dest_abs.display()
                        ),
                    });
                    // Critical: put the orphan back so the manifest
                    // stays consistent with the disk. The migration
                    // failed; we shouldn't pretend it succeeded.
                    project.tracks.push(orphan);
                    continue;
                }
            }
        }

        // Rate-mismatch check against the recordings project's existing
        // first-track rate (if any).
        if let Some(r) = recordings_first_rate {
            if r != orphan.sample_rate {
                report.recordings_rate_mismatch = true;
            }
        }

        // Rewrite the orphan's id + file_rel and append to recordings.
        // Every other field carries over verbatim — the user's gain,
        // automation, correction chain, polarity flip, etc.
        orphan.id = new_id;
        orphan.file = new_file_rel;
        recordings.tracks.push(orphan);

        let _ = display_name; // intentionally unused on the success path
        report.moved_count += 1;
    }

    // Save the recordings manifest with the migrated tracks. The
    // active `project` is left for the caller to save (so we don't
    // race with other in-flight changes).
    recordings
        .save()
        .context("saving recordings manifest after cleanse")?;

    Ok(report)
}

/// True when `path` is the recordings root from
/// [`crate::config::Config::recordings_root`]. Used by callers (the
/// app layer) to decide whether the active project IS the recordings
/// project — in which case the cleanse is a no-op (no Suno mixdown
/// in a recordings project).
#[allow(dead_code)] // exposed for future use by app-level guards
pub fn is_recordings_root(path: &std::path::Path) -> bool {
    crate::config::Config::recordings_root()
        .map(|root| root == path)
        .unwrap_or(false)
}

// Re-export for tests.
#[allow(dead_code)]
pub(crate) fn _tracks_dir() -> &'static str {
    TRACKS_DIR
}

// Helper kept for symmetry / future use.
#[allow(dead_code)]
fn _suno_path_helper(project: &Project) -> Option<PathBuf> {
    project
        .suno_mixdown_path
        .as_ref()
        .map(|rel| project.root.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_report_is_empty() {
        let r = CleanseReport::default();
        assert!(r.is_empty());
        assert!(!r.summary().contains("⚠"));
    }

    #[test]
    fn rate_mismatch_flag_appears_in_summary() {
        let r = CleanseReport {
            moved_count: 1,
            failures: vec![],
            recordings_rate_mismatch: true,
        };
        assert!(r.summary().contains("⚠"));
        assert!(r.summary().contains("manual reorganisation"));
    }

    #[test]
    fn failure_lines_render_in_summary() {
        let r = CleanseReport {
            moved_count: 0,
            failures: vec![MigrationFailure {
                display_name: "stray vocal".into(),
                error: "rename across devices not permitted".into(),
            }],
            recordings_rate_mismatch: false,
        };
        let s = r.summary();
        assert!(s.contains("stray vocal"));
        assert!(s.contains("rename across devices"));
    }

    #[test]
    fn non_suno_project_is_no_op() {
        // Project with no suno_mixdown_path → cleanse should return an
        // empty report and leave tracks untouched, even if Recorded
        // entries exist.
        use crate::audio::SourceMode;
        use crate::dsp::Profile;
        let mut p = Project::new("scratch", PathBuf::from("/tmp/test"));
        p.tracks.push(crate::project::Track::recorded(
            "track-001",
            "user take",
            "tracks/track-001.wav",
            48_000,
            SourceMode::Mixdown,
            5.0,
            Profile::raw("Raw"),
        ));
        let report = cleanse_recordings_in_suno_project(&mut p).unwrap();
        assert!(report.is_empty());
        assert_eq!(p.tracks.len(), 1);
    }
}
