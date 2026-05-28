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
//! never the audio BLOBs. Audio enters the db once (import / destructive
//! edit / hot-swap) via [`crate::tib::TibDb`] directly; a routine save is
//! then a handful of small `UPDATE`s, which is the whole point of the
//! SQLite substrate (TBSS-FR-0007). Wired into the live load/save path
//! as of phase 2c.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use crate::project::{Project, ProjectKind, Track, TrackSource};
use crate::telemetry::TelemetryProfile;
use crate::tib::{RevKind, TibDb};

/// Reserved track id for a migrated Suno mixdown — stored as a
/// not-in-mix track (so it never shows as a Mix lane) whose `orig`
/// revision holds the reference audio. `meta.suno_mixdown_track_id`
/// points here.
pub const MIXDOWN_TRACK_ID: &str = "__mixdown__";

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

/// Write the project's metadata (meta row + stem/track rows) over the
/// given SQLite connection. Does **not** touch audio revisions. Upserts
/// existing rows and prunes stems/tracks that were removed from the
/// in-memory project (whose revisions cascade away). Takes a bare
/// `&Connection` so the caller can run the whole save inside one
/// `TibDb::transaction` — the routine save's atomicity guarantee.
pub fn save_metadata(project: &Project, conn: &Connection) -> Result<()> {
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

    prune_removed(conn, &keep_tracks, &keep_stems)?;
    Ok(())
}

/// Delete tracks/stems no longer present in the in-memory project. Their
/// revisions + config_revs cascade away (ON DELETE CASCADE).
fn prune_removed(conn: &Connection, keep_tracks: &[String], keep_stems: &[String]) -> Result<()> {
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

/// Map of `track_id` → `current_rev_id` for every track in the `.tib`
/// that has a current audio revision. The player snapshot uses this to
/// build `AudioSource::TibRev` entries — one indexed SELECT here, then
/// the owner-thread reopens its own read connection per track.
/// TBSS-FR-0007 phase 2c.
pub fn current_rev_id_map(db: &TibDb) -> Result<HashMap<String, i64>> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare("SELECT id, current_rev_id FROM tracks WHERE current_rev_id IS NOT NULL")
        .context("preparing current_rev_id_map")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .context("querying current_rev_id_map")?;
    let mut out = HashMap::new();
    for row in rows {
        let (id, rev) = row.context("decoding current_rev_id row")?;
        out.insert(id, rev);
    }
    Ok(out)
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
             WHERE t.loaded_in_mix = 1
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

// ── migration (folder format → .tib) ────────────────────────────────

/// Convert a folder-format [`Project`] (JSON manifest + sibling WAVs)
/// into a fresh `.tib` at `tib_path`. Meta + stem/track rows come from
/// [`save_metadata`]; each track's WAV is read from the folder and stored
/// as its `orig` revision (the immutable import baseline). A bundled Suno
/// mixdown, if present, is stored under the reserved [`MIXDOWN_TRACK_ID`]
/// track (not loaded in the mix) with `meta.suno_mixdown_track_id`
/// pointed at it. Lossless — nothing on disk is needed afterwards.
pub fn migrate_folder_to_tib(folder: &Project, tib_path: &Path) -> Result<()> {
    let db = TibDb::create(tib_path)
        .with_context(|| format!("creating .tib at {}", tib_path.display()))?;

    // 1. meta + stem/track rows (current_rev_id NULL until audio lands).
    save_metadata(folder, db.conn())?;

    // 2. each track's WAV → an `orig` revision BLOB.
    for t in &folder.tracks {
        let wav = folder.track_abs_path(t);
        let bytes =
            std::fs::read(&wav).with_context(|| format!("reading track WAV {}", wav.display()))?;
        let rid = db.insert_revision(
            &t.id,
            RevKind::Orig,
            "import (migrated from folder)",
            t.sample_rate,
            t.stereo,
            t.duration_secs,
            &bytes,
        )?;
        db.set_current_rev(&t.id, rid)?;
    }

    // 3. bundled Suno mixdown → reserved, not-in-mix track.
    if let Some(rel) = &folder.suno_mixdown_path {
        let mix_path = folder.root.join(rel);
        if mix_path.is_file() {
            migrate_mixdown(&db, &mix_path)?;
        }
    }
    Ok(())
}

fn migrate_mixdown(db: &TibDb, mix_path: &Path) -> Result<()> {
    let bytes = std::fs::read(mix_path)
        .with_context(|| format!("reading mixdown {}", mix_path.display()))?;
    // Read the spec from the in-memory bytes (avoids a second file read).
    let reader = hound::WavReader::new(Cursor::new(&bytes)).context("parsing mixdown WAV")?;
    let spec = reader.spec();
    let dur = reader.duration() as f32 / spec.sample_rate.max(1) as f32;
    let stereo = spec.channels >= 2;

    let stem_id = stem_id_for(MIXDOWN_TRACK_ID);
    let conn = db.conn();
    conn.execute(
        "INSERT INTO stems (id, name, ord) VALUES (?1, 'Mixdown', 9999)
         ON CONFLICT(id) DO NOTHING",
        params![stem_id],
    )?;
    conn.execute(
        "INSERT INTO tracks
           (id, stem_id, name, ord, sample_rate, stereo, duration_secs,
            loaded_in_mix, mute, gain_db, polarity_inverted)
         VALUES (?1, ?2, 'Mixdown', 9999, ?3, ?4, ?5, 0, 0, 0.0, 0)
         ON CONFLICT(id) DO NOTHING",
        params![
            MIXDOWN_TRACK_ID,
            stem_id,
            spec.sample_rate,
            stereo as i64,
            dur as f64
        ],
    )?;
    let rid = db.insert_revision(
        MIXDOWN_TRACK_ID,
        RevKind::Orig,
        "suno mixdown (migrated)",
        spec.sample_rate,
        stereo,
        dur,
        &bytes,
    )?;
    db.set_current_rev(MIXDOWN_TRACK_ID, rid)?;
    conn.execute(
        "UPDATE meta SET suno_mixdown_track_id = ?1",
        params![MIXDOWN_TRACK_ID],
    )?;
    Ok(())
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
        save_metadata(&original, db.conn()).unwrap();

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
        save_metadata(&p, db.conn()).unwrap();
        // Edit + save again: should UPDATE, not duplicate.
        p.tracks[0].gain_db = -6.0;
        p.name = "Renamed".into();
        save_metadata(&p, db.conn()).unwrap();

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
        save_metadata(&p, db.conn()).unwrap();
        assert_eq!(load_project(&db, path.clone()).unwrap().tracks.len(), 2);

        p.tracks.remove(1); // drop Drums
        save_metadata(&p, db.conn()).unwrap();
        let loaded = load_project(&db, path.clone()).unwrap();
        assert_eq!(loaded.tracks.len(), 1);
        assert_eq!(loaded.tracks[0].id, "track-001");
        cleanup(&path);
    }

    fn write_wav(path: &Path, sr: u32, stereo: bool) {
        let spec = hound::WavSpec {
            channels: if stereo { 2 } else { 1 },
            sample_rate: sr,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        let chans = if stereo { 2 } else { 1 };
        for i in 0..(sr / 10) {
            for _ in 0..chans {
                w.write_sample((i as i16).wrapping_mul(7)).unwrap();
            }
        }
        w.finalize().unwrap();
    }

    fn folder_track(id: &str, file: &str, name: &str) -> Track {
        Track {
            id: id.into(),
            name: name.into(),
            file: file.into(),
            mute: false,
            gain_db: 0.0,
            sample_rate: 48_000,
            channel_source: None,
            duration_secs: 0.1,
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
    fn current_rev_id_map_returns_pointer_per_track() {
        let dir = std::env::temp_dir().join(format!("tbss-revmap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tracks")).unwrap();
        write_wav(&dir.join("tracks/track-001.wav"), 48_000, true);
        write_wav(&dir.join("tracks/track-002.wav"), 48_000, true);

        let mut proj = Project::new("Map", dir.clone());
        proj.tracks
            .push(folder_track("track-001", "tracks/track-001.wav", "Vox"));
        proj.tracks
            .push(folder_track("track-002", "tracks/track-002.wav", "Drums"));
        let tib = dir.join("map.tib");
        migrate_folder_to_tib(&proj, &tib).unwrap();

        let db = TibDb::open(&tib).unwrap();
        let map = current_rev_id_map(&db).unwrap();
        assert_eq!(map.len(), 2, "one rev pointer per track");
        // Both pointers must round-trip into a real audio BLOB.
        for tid in ["track-001", "track-002"] {
            let rev = map.get(tid).copied().expect("track in map");
            let bytes = db.read_revision_audio(rev).unwrap();
            assert!(!bytes.is_empty(), "rev BLOB non-empty for {tid}");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_folder_round_trips_audio_and_mixdown() {
        let dir = std::env::temp_dir().join(format!("tbss-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tracks")).unwrap();
        write_wav(&dir.join("tracks/track-001.wav"), 48_000, true);
        write_wav(&dir.join("tracks/track-002.wav"), 48_000, true);
        write_wav(&dir.join("tracks/mixdown.wav"), 48_000, true);

        let mut proj = Project::new("Migrated", dir.clone());
        proj.tracks
            .push(folder_track("track-001", "tracks/track-001.wav", "Vocals"));
        proj.tracks
            .push(folder_track("track-002", "tracks/track-002.wav", "Drums"));
        proj.suno_mixdown_path = Some("tracks/mixdown.wav".into());

        let tib = dir.join("migrated.tib");
        migrate_folder_to_tib(&proj, &tib).unwrap();

        let db = TibDb::open(&tib).unwrap();
        let loaded = load_project(&db, tib.clone()).unwrap();
        assert_eq!(loaded.tracks.len(), 2, "mixdown is not a mix lane");

        // Track audio preserved byte-for-byte (the orig revision == the file).
        let a1 = db.read_current_audio("track-001").unwrap();
        assert_eq!(a1, std::fs::read(dir.join("tracks/track-001.wav")).unwrap());

        // Mixdown audio preserved + pointer set.
        assert_eq!(loaded.suno_mixdown_path.as_deref(), Some(MIXDOWN_TRACK_ID));
        let mix = db.read_current_audio(MIXDOWN_TRACK_ID).unwrap();
        assert_eq!(mix, std::fs::read(dir.join("tracks/mixdown.wav")).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
