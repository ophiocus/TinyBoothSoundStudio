//! Project ↔ `.tib` (SQLite) mapping — TBSS-FR-0007 phase 2a.
//!
//! Serialises the in-memory [`Project`] (kept flat: `Vec<Track>`) to the
//! SQLite schema in [`crate::tib`] and back. Each track maps to a
//! single-track stem for now (`stem-<track id>`); multi-track stem
//! *groups* arrive with the UI in a later phase. Sub-structs that the
//! manifest already stores as JSON (`Profile`, `AutomationLane`,
//! `TrackTelemetry`, `TrackSource`, `TelemetryProfile`, `KeyEstimate`)
//! are stored as JSON text columns.
//!
//! [`save_metadata`] writes **only** the project + stem/track rows —
//! never the audio BLOBs. Audio enters the db once (import / record /
//! destructive edit) via [`crate::tib::TibDb`] directly; a routine save
//! is then a handful of small `UPDATE`s, which is the whole point of the
//! SQLite substrate (TBSS-FR-0007). This module is **not yet wired into
//! the app** (phase 2c/2d) — hence the module-level `allow(dead_code)`.
#![allow(dead_code)]

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::PathBuf;

use crate::project::{Project, ProjectKind, Track, TrackSource};
use crate::telemetry::TelemetryProfile;
use crate::tib::TibDb;

// ── JSON column helpers ─────────────────────────────────────────────

fn to_json<T: Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).context("encoding JSON column")
}
fn opt_to_json<T: Serialize>(v: &Option<T>) -> Result<Option<String>> {
    match v {
        Some(x) => Ok(Some(to_json(x)?)),
        None => Ok(None),
    }
}
fn opt_from_json<T: DeserializeOwned>(s: Option<String>) -> Result<Option<T>> {
    match s {
        Some(t) => Ok(Some(
            serde_json::from_str(&t).context("decoding JSON column")?,
        )),
        None => Ok(None),
    }
}

fn kind_str(k: ProjectKind) -> &'static str {
    match k {
        ProjectKind::Standard => "Standard",
        ProjectKind::Recordings => "Recordings",
        ProjectKind::TinyDAW => "TinyDAW",
    }
}
fn kind_from(s: &str) -> ProjectKind {
    match s {
        "Recordings" => ProjectKind::Recordings,
        "TinyDAW" => ProjectKind::TinyDAW,
        _ => ProjectKind::Standard,
    }
}

/// The per-track stem id under the flat (one-stem-per-track) model.
pub fn stem_id_for(track_id: &str) -> String {
    format!("stem-{track_id}")
}

// ── save ────────────────────────────────────────────────────────────

/// Write the project's metadata (meta row + stem/track rows) to the
/// `.tib`. Does **not** touch audio revisions. Upserts existing rows and
/// prunes stems/tracks that were removed from the in-memory project
/// (whose revisions cascade away). Wrap the call in a `TibDb`
/// transaction at the call site if atomicity across the whole save is
/// wanted.
pub fn save_metadata(project: &Project, db: &TibDb) -> Result<()> {
    let conn = db.conn();

    conn.execute(
        "UPDATE meta SET
           name = ?1, created = ?2, kind = ?3, master_gain_db = ?4,
           master_gain_automation = ?5, corrections_disabled = ?6,
           default_correction = ?7, suno_mixdown_track_id = ?8,
           suno_mixdown_lufs = ?9, song_key_estimate = ?10,
           next_suno_ordinal = ?11",
        params![
            project.name,
            project.created.to_rfc3339(),
            kind_str(project.kind),
            project.master_gain_db as f64,
            opt_to_json(&project.master_gain_automation)?,
            project.corrections_disabled as i64,
            opt_to_json(&project.default_correction)?,
            project.suno_mixdown_path,
            project.suno_mixdown_lufs.map(|x| x as f64),
            opt_to_json(&project.song_key_estimate)?,
            project.next_suno_ordinal,
        ],
    )
    .context("updating meta")?;

    let mut keep_tracks: Vec<String> = Vec::with_capacity(project.tracks.len());
    let mut keep_stems: Vec<String> = Vec::with_capacity(project.tracks.len());
    for (i, t) in project.tracks.iter().enumerate() {
        let stem_id = stem_id_for(&t.id);
        keep_tracks.push(t.id.clone());
        keep_stems.push(stem_id.clone());

        conn.execute(
            "INSERT INTO stems (id, name, ord) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, ord = excluded.ord",
            params![stem_id, t.name, i as i64],
        )
        .context("upserting stem")?;

        conn.execute(
            "INSERT INTO tracks
               (id, stem_id, name, ord, source, sample_rate, stereo, duration_secs,
                channel_source, correction, gain_db, polarity_inverted, gain_automation,
                telemetry, rec_profile, telemetry_profile, loaded_in_mix, mute)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,1,?17)
             ON CONFLICT(id) DO UPDATE SET
               stem_id = excluded.stem_id, name = excluded.name, ord = excluded.ord,
               source = excluded.source, sample_rate = excluded.sample_rate,
               stereo = excluded.stereo, duration_secs = excluded.duration_secs,
               channel_source = excluded.channel_source, correction = excluded.correction,
               gain_db = excluded.gain_db, polarity_inverted = excluded.polarity_inverted,
               gain_automation = excluded.gain_automation, telemetry = excluded.telemetry,
               rec_profile = excluded.rec_profile, telemetry_profile = excluded.telemetry_profile,
               mute = excluded.mute
               -- current_rev_id intentionally NOT updated here (audio-managed)",
            params![
                t.id,
                stem_id,
                t.name,
                i as i64,
                to_json(&t.source)?,
                t.sample_rate,
                t.stereo as i64,
                t.duration_secs as f64,
                t.channel_source.map(|c| c as i64),
                opt_to_json(&t.correction)?,
                t.gain_db as f64,
                t.polarity_inverted as i64,
                opt_to_json(&t.gain_automation)?,
                opt_to_json(&t.telemetry)?,
                opt_to_json(&t.profile)?,
                to_json(&t.telemetry_profile)?,
                t.mute as i64,
            ],
        )
        .context("upserting track")?;
    }

    prune_removed(db, &keep_tracks, &keep_stems)?;
    Ok(())
}

/// Delete tracks/stems no longer present in the in-memory project. Their
/// revisions + config_revs cascade away (ON DELETE CASCADE).
fn prune_removed(db: &TibDb, keep_tracks: &[String], keep_stems: &[String]) -> Result<()> {
    let conn = db.conn();
    let existing_tracks: Vec<String> = {
        let mut s = conn.prepare("SELECT id FROM tracks")?;
        let rows = s.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for id in existing_tracks {
        if !keep_tracks.contains(&id) {
            conn.execute("DELETE FROM tracks WHERE id = ?1", params![id])?;
        }
    }
    let existing_stems: Vec<String> = {
        let mut s = conn.prepare("SELECT id FROM stems")?;
        let rows = s.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for id in existing_stems {
        if !keep_stems.contains(&id) {
            conn.execute("DELETE FROM stems WHERE id = ?1", params![id])?;
        }
    }
    Ok(())
}

// ── load ────────────────────────────────────────────────────────────

/// Raw track columns, read inside the rusqlite closure (rusqlite types
/// only); JSON decoding happens afterwards where `anyhow::?` works.
struct RawTrack {
    id: String,
    name: String,
    source: Option<String>,
    sample_rate: i64,
    stereo: i64,
    duration_secs: Option<f64>,
    channel_source: Option<i64>,
    correction: Option<String>,
    gain_db: Option<f64>,
    polarity_inverted: i64,
    gain_automation: Option<String>,
    telemetry: Option<String>,
    rec_profile: Option<String>,
    telemetry_profile: Option<String>,
    mute: i64,
}

/// Load a [`Project`] from a `.tib`. `db_path` is stamped into
/// `project.root` (the open-file handle the rest of the app keys on).
pub fn load_project(db: &TibDb, db_path: PathBuf) -> Result<Project> {
    let conn = db.conn();

    struct RawMeta {
        name: String,
        created: String,
        kind: String,
        master_gain_db: Option<f64>,
        mga: Option<String>,
        corr_disabled: i64,
        def_corr: Option<String>,
        mixdown_id: Option<String>,
        mixdown_lufs: Option<f64>,
        key: Option<String>,
        ordinal: i64,
    }
    let m: RawMeta = conn
        .query_row(
            "SELECT name, created, kind, master_gain_db, master_gain_automation,
                    corrections_disabled, default_correction, suno_mixdown_track_id,
                    suno_mixdown_lufs, song_key_estimate, next_suno_ordinal
             FROM meta",
            [],
            |r| {
                Ok(RawMeta {
                    name: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    created: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    kind: r
                        .get::<_, Option<String>>(2)?
                        .unwrap_or_else(|| "Standard".into()),
                    master_gain_db: r.get(3)?,
                    mga: r.get(4)?,
                    corr_disabled: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
                    def_corr: r.get(6)?,
                    mixdown_id: r.get(7)?,
                    mixdown_lufs: r.get(8)?,
                    key: r.get(9)?,
                    ordinal: r.get::<_, Option<i64>>(10)?.unwrap_or(1),
                })
            },
        )
        .context("reading meta")?;

    let created = DateTime::parse_from_rfc3339(&m.created)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    let raws: Vec<RawTrack> = {
        let mut stmt = conn.prepare(
            "SELECT t.id, t.name, t.source, t.sample_rate, t.stereo, t.duration_secs,
                    t.channel_source, t.correction, t.gain_db, t.polarity_inverted,
                    t.gain_automation, t.telemetry, t.rec_profile, t.telemetry_profile, t.mute
             FROM tracks t JOIN stems s ON t.stem_id = s.id
             ORDER BY s.ord, t.ord",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(RawTrack {
                id: r.get(0)?,
                name: r.get(1)?,
                source: r.get(2)?,
                sample_rate: r.get(3)?,
                stereo: r.get(4)?,
                duration_secs: r.get(5)?,
                channel_source: r.get(6)?,
                correction: r.get(7)?,
                gain_db: r.get(8)?,
                polarity_inverted: r.get(9)?,
                gain_automation: r.get(10)?,
                telemetry: r.get(11)?,
                rec_profile: r.get(12)?,
                telemetry_profile: r.get(13)?,
                mute: r.get(14)?,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };

    let mut tracks = Vec::with_capacity(raws.len());
    for raw in raws {
        tracks.push(build_track(raw)?);
    }

    Ok(Project {
        version: 1,
        name: m.name,
        created,
        tracks,
        kind: kind_from(&m.kind),
        master_gain_db: m.master_gain_db.unwrap_or(0.0) as f32,
        master_gain_automation: opt_from_json(m.mga)?,
        next_suno_ordinal: m.ordinal as u32,
        corrections_disabled: m.corr_disabled != 0,
        default_correction: opt_from_json(m.def_corr)?,
        suno_mixdown_path: m.mixdown_id,
        suno_mixdown_lufs: m.mixdown_lufs.map(|x| x as f32),
        song_key_estimate: opt_from_json(m.key)?,
        root: db_path,
    })
}

fn build_track(raw: RawTrack) -> Result<Track> {
    Ok(Track {
        id: raw.id,
        name: raw.name,
        file: String::new(), // audio lives in the db now, not on disk
        mute: raw.mute != 0,
        gain_db: raw.gain_db.unwrap_or(0.0) as f32,
        sample_rate: raw.sample_rate as u32,
        channel_source: raw.channel_source.map(|c| c as u16),
        duration_secs: raw.duration_secs.unwrap_or(0.0) as f32,
        profile: opt_from_json(raw.rec_profile)?,
        stereo: raw.stereo != 0,
        source: match raw.source {
            Some(s) => serde_json::from_str(&s).context("decoding TrackSource")?,
            None => TrackSource::default(),
        },
        correction: opt_from_json(raw.correction)?,
        gain_automation: opt_from_json(raw.gain_automation)?,
        polarity_inverted: raw.polarity_inverted != 0,
        telemetry: opt_from_json(raw.telemetry)?,
        telemetry_profile: match raw.telemetry_profile {
            Some(s) => serde_json::from_str(&s).context("decoding TelemetryProfile")?,
            None => TelemetryProfile::default(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::{AutomationLane, AutomationPoint};
    use crate::project::StemRole;

    fn scratch(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("tbss-tibproj-{name}-{}.tib", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let mut q = p.as_os_str().to_os_string();
            q.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(q));
        }
        p
    }
    fn cleanup(p: &std::path::Path) {
        for suffix in ["", "-wal", "-shm"] {
            let mut q = p.as_os_str().to_os_string();
            q.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(q));
        }
    }

    fn fixture() -> Project {
        let mut p = Project::new("My Song", PathBuf::from("ignored"));
        p.master_gain_db = -1.5;
        p.corrections_disabled = true;
        p.next_suno_ordinal = 4;
        p.suno_mixdown_lufs = Some(-14.2);
        p.tracks.push(Track {
            id: "track-001".into(),
            name: "Lead Vocal".into(),
            file: "tracks/track-001.wav".into(),
            mute: true,
            gain_db: -3.0,
            sample_rate: 48_000,
            channel_source: Some(1),
            duration_secs: 200.8,
            profile: None,
            stereo: true,
            source: TrackSource::SunoStem {
                role: StemRole::Vocals,
                original_filename: "vocals.wav".into(),
                session_epoch: Some(1_777_095_097),
                session_ordinal: Some(3),
                provenance: Some("made with suno studio".into()),
            },
            correction: None,
            gain_automation: Some(AutomationLane {
                points: vec![
                    AutomationPoint {
                        time_secs: 0.0,
                        gain_db: -3.0,
                    },
                    AutomationPoint {
                        time_secs: 5.0,
                        gain_db: -1.0,
                    },
                ],
            }),
            polarity_inverted: true,
            telemetry: None,
            telemetry_profile: TelemetryProfile::default(),
        });
        p
    }

    #[test]
    fn project_round_trips_through_tib() {
        let path = scratch("roundtrip");
        let db = TibDb::create(&path).unwrap();
        let original = fixture();
        save_metadata(&original, &db).unwrap();

        let loaded = load_project(&db, path.clone()).unwrap();
        assert_eq!(loaded.name, "My Song");
        assert_eq!(loaded.master_gain_db, -1.5);
        assert!(loaded.corrections_disabled);
        assert_eq!(loaded.next_suno_ordinal, 4);
        assert_eq!(loaded.suno_mixdown_lufs, Some(-14.2));
        assert_eq!(loaded.tracks.len(), 1);

        let t = &loaded.tracks[0];
        assert_eq!(t.id, "track-001");
        assert_eq!(t.name, "Lead Vocal");
        assert!(t.mute);
        assert_eq!(t.gain_db, -3.0);
        assert!(t.stereo);
        assert!(t.polarity_inverted);
        assert_eq!(t.channel_source, Some(1));
        assert_eq!(t.gain_automation.as_ref().unwrap().points.len(), 2);
        match &t.source {
            TrackSource::SunoStem {
                role,
                session_ordinal,
                ..
            } => {
                assert_eq!(*role, StemRole::Vocals);
                assert_eq!(*session_ordinal, Some(3));
            }
            _ => panic!("source should round-trip as SunoStem"),
        }
        cleanup(&path);
    }

    #[test]
    fn save_is_idempotent_and_updates_in_place() {
        let path = scratch("idempotent");
        let db = TibDb::create(&path).unwrap();
        let mut p = fixture();
        save_metadata(&p, &db).unwrap();
        // Edit + save again: should UPDATE, not duplicate.
        p.tracks[0].gain_db = -6.0;
        p.name = "Renamed".into();
        save_metadata(&p, &db).unwrap();

        let loaded = load_project(&db, path.clone()).unwrap();
        assert_eq!(loaded.name, "Renamed");
        assert_eq!(loaded.tracks.len(), 1, "no duplicate track rows");
        assert_eq!(loaded.tracks[0].gain_db, -6.0);
        cleanup(&path);
    }

    #[test]
    fn removing_a_track_prunes_its_rows() {
        let path = scratch("prune");
        let db = TibDb::create(&path).unwrap();
        let mut p = fixture();
        p.tracks.push(Track {
            id: "track-002".into(),
            name: "Drums".into(),
            file: String::new(),
            mute: false,
            gain_db: 0.0,
            sample_rate: 48_000,
            channel_source: None,
            duration_secs: 200.8,
            profile: None,
            stereo: true,
            source: TrackSource::default(),
            correction: None,
            gain_automation: None,
            polarity_inverted: false,
            telemetry: None,
            telemetry_profile: TelemetryProfile::default(),
        });
        save_metadata(&p, &db).unwrap();
        assert_eq!(load_project(&db, path.clone()).unwrap().tracks.len(), 2);

        p.tracks.remove(1); // drop Drums
        save_metadata(&p, &db).unwrap();
        let loaded = load_project(&db, path.clone()).unwrap();
        assert_eq!(loaded.tracks.len(), 1);
        assert_eq!(loaded.tracks[0].id, "track-001");
        cleanup(&path);
    }
}
