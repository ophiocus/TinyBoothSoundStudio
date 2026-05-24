//! TinyBooth `.tib` container — a single **SQLite** database.
//!
//! See `docs/feature-requests/TBSS-FR-0007`. A `.tib` is one SQLite file
//! holding the whole project: stems (named groups), tracks (the things
//! that load into the Mix tab), each track's revision history (binary
//! audio snapshots as BLOBs + non-destructive config snapshots), and the
//! console state. Saves are atomic SQLite transactions that touch only
//! changed pages (no whole-file rewrite); rollback is a pointer update;
//! WAL gives crash-safe commits. This replaces the abandoned ZIP
//! prototype — see the RFC's §"Why SQLite, not ZIP".
//!
//! Revision model (TBSS-FR-0007 §"Revision model"):
//!   * `revisions(kind='orig')` — the pristine import, never pruned.
//!   * `tracks.current_rev_id` — the live audio; the player reads the
//!     BLOB this points at (one indexed lookup). Rollback = repoint it.
//!   * `revisions(kind='destructive')` — committed snapshots, FIFO-5.
//!   * `config_revs` — non-destructive history (no audio bytes).
//!
//! NOTE: this is TBSS-FR-0007 **phase 1** — the storage layer, exercised
//! by its own tests but not yet wired into the app (phase 2 replaces the
//! folder format's path resolution with this). The module-level
//! `allow(dead_code)` is removed when phase 2 lands.
#![allow(dead_code)]

use anyhow::{Context, Result};
use rusqlite::{params, Connection, DatabaseName, OptionalExtension};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Current on-disk schema version, stored in `meta.schema_version`.
pub const SCHEMA_VERSION: i64 = 1;

/// Page size for the database. 16 KiB suits the large WAV BLOBs we store
/// (TBSS-FR-0007 §BLOBs). Must be set before the first write.
const PAGE_SIZE: i64 = 16_384;

const SCHEMA_SQL: &str = "\
CREATE TABLE meta (
  schema_version INTEGER NOT NULL,
  name TEXT, created TEXT, kind TEXT,
  master_gain_db REAL,
  master_gain_automation TEXT,
  corrections_disabled INTEGER,
  default_correction TEXT,
  suno_mixdown_track_id TEXT,
  suno_mixdown_lufs REAL,
  song_key_estimate TEXT,
  next_suno_ordinal INTEGER
);
CREATE TABLE stems (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  ord INTEGER
);
CREATE TABLE tracks (
  id TEXT PRIMARY KEY,
  stem_id TEXT NOT NULL REFERENCES stems(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  ord INTEGER,
  source TEXT,                                   -- JSON TrackSource
  sample_rate INTEGER, stereo INTEGER, duration_secs REAL,
  channel_source INTEGER,                        -- mono take's hardware channel
  current_rev_id INTEGER REFERENCES revisions(id),
  correction TEXT, gain_db REAL, polarity_inverted INTEGER,
  gain_automation TEXT, telemetry TEXT,          -- JSON
  rec_profile TEXT,                              -- JSON recording-time snapshot
  telemetry_profile TEXT,                        -- JSON analyzer profile
  loaded_in_mix INTEGER, mute INTEGER
);
CREATE TABLE revisions (
  id INTEGER PRIMARY KEY,
  track_id TEXT NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  created TEXT, label TEXT, pinned INTEGER DEFAULT 0,
  sample_rate INTEGER, stereo INTEGER, duration_secs REAL,
  audio BLOB NOT NULL
);
CREATE INDEX idx_revisions_track ON revisions(track_id);
CREATE TABLE config_revs (
  id INTEGER PRIMARY KEY,
  track_id TEXT NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
  created TEXT, label TEXT,
  correction TEXT, gain_db REAL, polarity_inverted INTEGER, gain_automation TEXT
);
CREATE INDEX idx_config_revs_track ON config_revs(track_id);
";

/// Revision kind — `Orig` (pristine import, never pruned) or
/// `Destructive` (a committed snapshot, FIFO-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevKind {
    Orig,
    Destructive,
}

impl RevKind {
    fn as_str(self) -> &'static str {
        match self {
            RevKind::Orig => "orig",
            RevKind::Destructive => "destructive",
        }
    }
}

/// An open `.tib` database. Owns one `rusqlite::Connection`. The audio
/// owner-thread opens its own read connection (phase 3); SQLite WAL
/// allows concurrent readers alongside the writer.
pub struct TibDb {
    conn: Connection,
    path: PathBuf,
}

impl TibDb {
    /// Create a fresh `.tib` at `path` (overwriting any existing file +
    /// its WAL sidecars), apply pragmas + schema, and stamp the meta row.
    pub fn create(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).context("creating .tib parent dir")?;
        }
        for suffix in ["", "-wal", "-shm"] {
            let mut p = path.as_os_str().to_os_string();
            p.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(p));
        }

        let conn = Connection::open(&path)
            .with_context(|| format!("creating .tib: {}", path.display()))?;
        // Order matters: page_size + auto_vacuum must be set before the
        // first write (the WAL switch) locks them into the file header,
        // and before any table exists. execute_batch tolerates the row
        // that `journal_mode` returns.
        conn.execute_batch(&format!(
            "PRAGMA page_size = {PAGE_SIZE};
             PRAGMA auto_vacuum = INCREMENTAL;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;"
        ))
        .context("applying pragmas")?;
        conn.execute_batch(SCHEMA_SQL).context("creating schema")?;
        conn.execute(
            "INSERT INTO meta (schema_version, kind, next_suno_ordinal, corrections_disabled)
             VALUES (?1, 'Standard', 1, 0)",
            params![SCHEMA_VERSION],
        )
        .context("seeding meta row")?;
        Ok(Self { conn, path })
    }

    /// Open an existing `.tib`. Re-asserts the runtime pragmas (WAL +
    /// foreign_keys persist in the file header, but foreign_keys must be
    /// re-enabled per connection) and checks the schema version.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let conn =
            Connection::open(&path).with_context(|| format!("opening .tib: {}", path.display()))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")
            .context("applying connection pragmas")?;
        let db = Self { conn, path };
        let v = db.schema_version()?;
        if v > SCHEMA_VERSION {
            anyhow::bail!(
                "this .tib is schema v{v}, newer than this app supports (v{SCHEMA_VERSION}) — update TinyBooth"
            );
        }
        Ok(db)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Borrow the underlying connection. Used by the project-mapping
    /// layer (`tib_project`) to run domain-specific upsert/select SQL;
    /// `TibDb` itself stays free of `Project`/`Track` knowledge.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT schema_version FROM meta", [], |r| r.get(0))
            .context("reading schema_version")
    }

    /// Read a string PRAGMA (test/diagnostic helper), e.g. `journal_mode`.
    pub fn pragma_string(&self, name: &str) -> Result<String> {
        self.conn
            .query_row(&format!("PRAGMA {name}"), [], |r| r.get::<_, String>(0))
            .with_context(|| format!("reading pragma {name}"))
    }

    pub fn pragma_i64(&self, name: &str) -> Result<i64> {
        self.conn
            .query_row(&format!("PRAGMA {name}"), [], |r| r.get::<_, i64>(0))
            .with_context(|| format!("reading pragma {name}"))
    }

    /// Run `f` inside a transaction; commit on `Ok`, roll back on `Err`.
    pub fn transaction<T>(&mut self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let tx = self.conn.transaction().context("begin transaction")?;
        let out = f(&tx)?;
        tx.commit().context("commit transaction")?;
        Ok(out)
    }

    // ── stems / tracks ────────────────────────────────────────────────

    pub fn insert_stem(&self, id: &str, name: &str, ord: i64) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO stems (id, name, ord) VALUES (?1, ?2, ?3)",
                params![id, name, ord],
            )
            .context("inserting stem")?;
        Ok(())
    }

    /// Insert a track with no audio yet (`current_rev_id` NULL); set it
    /// via [`Self::set_current_rev`] after inserting the `orig` revision.
    pub fn insert_track(&self, id: &str, stem_id: &str, name: &str, ord: i64) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO tracks (id, stem_id, name, ord, loaded_in_mix, mute, gain_db, polarity_inverted)
                 VALUES (?1, ?2, ?3, ?4, 1, 0, 0.0, 0)",
                params![id, stem_id, name, ord],
            )
            .context("inserting track")?;
        Ok(())
    }

    // ── revisions (binary audio history) ──────────────────────────────

    /// Insert a revision carrying `audio` bytes; returns its row id.
    /// (The arg list mirrors the columns; routing it through a struct
    /// would just move the fanout to the caller — same call as the
    /// existing `Track::from_suno_stem`.)
    #[allow(clippy::too_many_arguments)]
    pub fn insert_revision(
        &self,
        track_id: &str,
        kind: RevKind,
        label: &str,
        sample_rate: u32,
        stereo: bool,
        duration_secs: f32,
        audio: &[u8],
    ) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO revisions
                   (track_id, kind, created, label, pinned, sample_rate, stereo, duration_secs, audio)
                 VALUES (?1, ?2, datetime('now'), ?3, 0, ?4, ?5, ?6, ?7)",
                params![
                    track_id,
                    kind.as_str(),
                    label,
                    sample_rate,
                    stereo as i64,
                    duration_secs as f64,
                    audio
                ],
            )
            .context("inserting revision")?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_current_rev(&self, track_id: &str, rev_id: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE tracks SET current_rev_id = ?2 WHERE id = ?1",
                params![track_id, rev_id],
            )
            .context("setting current_rev_id")?;
        Ok(())
    }

    pub fn current_rev_id(&self, track_id: &str) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT current_rev_id FROM tracks WHERE id = ?1",
                params![track_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()
            .context("reading current_rev_id")
            .map(Option::flatten)
    }

    /// Read a revision's audio BLOB via **incremental BLOB I/O** — streams
    /// the bytes out without rusqlite materialising a second copy
    /// internally. (The caller still owns the returned `Vec`.)
    pub fn read_revision_audio(&self, rev_id: i64) -> Result<Vec<u8>> {
        let blob = self
            .conn
            .blob_open(DatabaseName::Main, "revisions", "audio", rev_id, true)
            .context("opening revision BLOB")?;
        let mut buf = Vec::with_capacity(blob.len());
        let mut blob = blob;
        blob.read_to_end(&mut buf)
            .context("reading revision BLOB")?;
        Ok(buf)
    }

    /// Read the audio the track currently plays (the `current_rev_id`
    /// pointer → BLOB). Errors if the track has no current revision.
    pub fn read_current_audio(&self, track_id: &str) -> Result<Vec<u8>> {
        let rev = self
            .current_rev_id(track_id)?
            .ok_or_else(|| anyhow::anyhow!("track {track_id} has no current revision"))?;
        self.read_revision_audio(rev)
    }

    /// FIFO-prune destructive revisions: keep `keep` newest destructive
    /// revisions per track, plus `orig`, plus any pinned, plus the
    /// current one. Returns how many rows were deleted. Caller runs
    /// `incremental_vacuum` afterwards to reclaim pages.
    pub fn prune_destructive(&self, track_id: &str, keep: usize) -> Result<usize> {
        let n = self
            .conn
            .execute(
                "DELETE FROM revisions
                 WHERE track_id = ?1
                   AND kind = 'destructive'
                   AND pinned = 0
                   AND id <> COALESCE((SELECT current_rev_id FROM tracks WHERE id = ?1), -1)
                   AND id NOT IN (
                     SELECT id FROM revisions
                     WHERE track_id = ?1 AND kind = 'destructive'
                     ORDER BY id DESC LIMIT ?2
                   )",
                params![track_id, keep as i64],
            )
            .context("pruning destructive revisions")?;
        Ok(n)
    }

    /// Reclaim free pages left by deletes (cheap, incremental — the
    /// auto-`VACUUM` equivalent of ZIP compaction).
    pub fn incremental_vacuum(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA incremental_vacuum;")
            .context("incremental_vacuum")?;
        Ok(())
    }

    /// Count revisions of a given kind for a track (test/diagnostic).
    pub fn revision_count(&self, track_id: &str, kind: RevKind) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM revisions WHERE track_id = ?1 AND kind = ?2",
                params![track_id, kind.as_str()],
                |r| r.get(0),
            )
            .context("counting revisions")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("tbss-tib-{}-{}.tib", name, std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let mut q = p.as_os_str().to_os_string();
            q.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(q));
        }
        p
    }

    fn cleanup(p: &Path) {
        for suffix in ["", "-wal", "-shm"] {
            let mut q = p.as_os_str().to_os_string();
            q.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(q));
        }
    }

    /// Seed one stem + track + an `orig` revision; returns (track_id, rev_id).
    fn seed_track(db: &TibDb) -> (String, i64) {
        db.insert_stem("stem-1", "Lead Vocal", 0).unwrap();
        db.insert_track("trk-1", "stem-1", "Take 1", 0).unwrap();
        let rid = db
            .insert_revision(
                "trk-1",
                RevKind::Orig,
                "import",
                48_000,
                true,
                1.0,
                &[1u8; 64],
            )
            .unwrap();
        db.set_current_rev("trk-1", rid).unwrap();
        ("trk-1".to_string(), rid)
    }

    #[test]
    fn create_sets_wal_page_size_and_schema() {
        let p = scratch("create");
        let db = TibDb::create(&p).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(db.pragma_string("journal_mode").unwrap(), "wal");
        assert_eq!(db.pragma_i64("page_size").unwrap(), PAGE_SIZE);
        cleanup(&p);
    }

    #[test]
    fn reopen_round_trips() {
        let p = scratch("reopen");
        {
            let _db = TibDb::create(&p).unwrap();
        }
        let db = TibDb::open(&p).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        // WAL persists in the file header across reopen.
        assert_eq!(db.pragma_string("journal_mode").unwrap(), "wal");
        cleanup(&p);
    }

    #[test]
    fn large_blob_round_trips_via_incremental_io() {
        let p = scratch("blob");
        let db = TibDb::create(&p).unwrap();
        db.insert_stem("s", "Drums", 0).unwrap();
        db.insert_track("t", "s", "Kit", 0).unwrap();
        // ~4 MiB stand-in for stem audio, with a recognisable pattern.
        let audio: Vec<u8> = (0..4 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        let rid = db
            .insert_revision("t", RevKind::Orig, "import", 48_000, true, 30.0, &audio)
            .unwrap();
        db.set_current_rev("t", rid).unwrap();
        let back = db.read_current_audio("t").unwrap();
        assert_eq!(back.len(), audio.len());
        assert_eq!(back, audio);
        cleanup(&p);
    }

    #[test]
    fn current_rev_pointer_selects_latest_audio() {
        let p = scratch("pointer");
        let db = TibDb::create(&p).unwrap();
        let (track, _orig) = seed_track(&db);
        let d = db
            .insert_revision(
                &track,
                RevKind::Destructive,
                "trim",
                48_000,
                true,
                0.9,
                &[2u8; 64],
            )
            .unwrap();
        db.set_current_rev(&track, d).unwrap();
        assert_eq!(db.read_current_audio(&track).unwrap(), vec![2u8; 64]);

        // Rollback is a pointer update — no byte copy.
        let orig = db.current_rev_id(&track).unwrap();
        assert_eq!(orig, Some(d));
        cleanup(&p);
    }

    #[test]
    fn prune_keeps_orig_current_and_last_five() {
        let p = scratch("prune");
        let db = TibDb::create(&p).unwrap();
        let (track, _orig) = seed_track(&db);
        // Seven destructive revisions; current = the newest.
        let mut last = 0;
        for i in 0..7 {
            last = db
                .insert_revision(
                    &track,
                    RevKind::Destructive,
                    &format!("edit {i}"),
                    48_000,
                    true,
                    1.0,
                    &[i as u8; 64],
                )
                .unwrap();
        }
        db.set_current_rev(&track, last).unwrap();

        let deleted = db.prune_destructive(&track, 5).unwrap();
        db.incremental_vacuum().unwrap();
        assert_eq!(deleted, 2, "two oldest destructive revs pruned");
        assert_eq!(db.revision_count(&track, RevKind::Destructive).unwrap(), 5);
        assert_eq!(
            db.revision_count(&track, RevKind::Orig).unwrap(),
            1,
            "orig kept"
        );
        // Current still resolves.
        assert_eq!(db.read_current_audio(&track).unwrap(), vec![6u8; 64]);
        cleanup(&p);
    }

    #[test]
    fn transaction_rolls_back_on_error() {
        let p = scratch("txn");
        let mut db = TibDb::create(&p).unwrap();
        let r: Result<()> = db.transaction(|tx| {
            tx.execute("INSERT INTO stems (id, name, ord) VALUES ('x', 'X', 0)", [])?;
            anyhow::bail!("boom"); // force rollback
        });
        assert!(r.is_err());
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM stems", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "failed transaction must leave no rows");
        cleanup(&p);
    }

    #[test]
    fn foreign_key_cascade_deletes_revisions_with_track() {
        let p = scratch("cascade");
        let db = TibDb::create(&p).unwrap();
        let (track, _) = seed_track(&db);
        assert_eq!(db.revision_count(&track, RevKind::Orig).unwrap(), 1);
        // Null the pointer first (tracks.current_rev_id has no cascade),
        // then delete the track → its revisions cascade away.
        db.conn
            .execute(
                "UPDATE tracks SET current_rev_id = NULL WHERE id = ?1",
                params![track],
            )
            .unwrap();
        db.conn
            .execute("DELETE FROM tracks WHERE id = ?1", params![track])
            .unwrap();
        assert_eq!(db.revision_count(&track, RevKind::Orig).unwrap(), 0);
        cleanup(&p);
    }
}
