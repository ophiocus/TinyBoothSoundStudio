//! TinyBooth Album `.tba` container — a single **SQLite** database.
//!
//! Sibling format to [`crate::tib`]. The schema is intentionally smaller:
//! a `.tba` doesn't own any source audio of its own. It owns an
//! arrangement — a list of `clips`, each referencing an external `.tib`
//! (a stem source) by absolute path, plus per-clip start/fade/gain.
//! The album's own bounced master goes into the same `mix_run` table
//! shape used by `.tib` (TBSS-FR-0011), so the bounced album is itself
//! loadable as a stem (in Crossfade, or in another album → recursive
//! composition).
//!
//! See `docs/feature-requests/TBSS-FR-0012-tinybooth-album.md`.
//!
//! Several CRUD methods (`path`, `conn`, `read_mix_run_audio`) are kept
//! for parity with the `.tib` storage API even though the v0.4.52 UI
//! doesn't yet exercise them — the Crossfade tab's future
//! "Album-loaded-as-stem" path will. Module-level allow rather than
//! per-item suppresses the noise without papering over real dead code.
#![allow(dead_code)]

use anyhow::{Context, Result};
use rusqlite::{params, Connection, DatabaseName, OptionalExtension};
use std::io::Read;
use std::path::{Path, PathBuf};

/// On-disk schema version, stored in `meta.schema_version`.
pub const SCHEMA_VERSION: i64 = 1;

/// Page size — matches `.tib` for the same reason (large mix_run BLOB).
const PAGE_SIZE: i64 = 16_384;

const SCHEMA_SQL: &str = "\
CREATE TABLE meta (
  schema_version INTEGER NOT NULL,
  name TEXT,
  created TEXT
);
CREATE TABLE clips (
  id INTEGER PRIMARY KEY,
  ord INTEGER NOT NULL,
  source_path TEXT NOT NULL,
  start_secs REAL NOT NULL,
  fade_in_secs REAL NOT NULL,
  fade_out_secs REAL NOT NULL,
  gain_db REAL NOT NULL
);
CREATE INDEX idx_clips_ord ON clips(ord);
CREATE TABLE mix_run (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  sample_rate INTEGER NOT NULL,
  channels INTEGER NOT NULL,
  frames INTEGER NOT NULL,
  source_signature TEXT NOT NULL,
  created TEXT NOT NULL,
  audio BLOB NOT NULL
);
";

/// One row of the `clips` table — a single arrangement slot.
#[derive(Debug, Clone)]
pub struct ClipRow {
    pub id: i64,
    pub ord: i64,
    pub source_path: String,
    pub start_secs: f32,
    pub fade_in_secs: f32,
    pub fade_out_secs: f32,
    pub gain_db: f32,
}

/// Metadata-only view of the `mix_run` row — same shape as
/// [`crate::tib::MixRunHeader`], copied here so the formats stay
/// independently importable without one pulling the other into scope.
#[derive(Debug, Clone)]
pub struct MixRunHeader {
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
    pub source_signature: String,
    pub created: String,
}

/// An open `.tba` database. Owns one `rusqlite::Connection`.
pub struct TbaDb {
    conn: Connection,
    path: PathBuf,
}

impl TbaDb {
    /// Create a fresh `.tba` at `path` (overwriting any existing file +
    /// its WAL sidecars), apply pragmas + schema, seed the meta row.
    pub fn create(path: impl Into<PathBuf>, name: &str) -> Result<Self> {
        let path = path.into();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).context("creating .tba parent dir")?;
        }
        for suffix in ["", "-wal", "-shm"] {
            let mut p = path.as_os_str().to_os_string();
            p.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(p));
        }
        let conn = Connection::open(&path)
            .with_context(|| format!("creating .tba: {}", path.display()))?;
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
            "INSERT INTO meta (schema_version, name, created)
             VALUES (?1, ?2, datetime('now'))",
            params![SCHEMA_VERSION, name],
        )
        .context("seeding meta row")?;
        Ok(Self { conn, path })
    }

    /// Open an existing `.tba`. Refuses missing files (same footgun
    /// guard as `TibDb::open`).
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if !path.is_file() {
            anyhow::bail!("not a .tba file: {}", path.display());
        }
        let conn =
            Connection::open(&path).with_context(|| format!("opening .tba: {}", path.display()))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")
            .context("applying connection pragmas")?;
        let db = Self { conn, path };
        let v = db.schema_version()?;
        if v > SCHEMA_VERSION {
            anyhow::bail!(
                "this .tba is schema v{v}, newer than this app supports (v{SCHEMA_VERSION}) — update TinyBooth"
            );
        }
        Ok(db)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT schema_version FROM meta", [], |r| r.get(0))
            .context("reading schema_version")
    }

    /// Project name — comes from `meta.name`, empty string when NULL.
    pub fn name(&self) -> Result<String> {
        self.conn
            .query_row("SELECT COALESCE(name, '') FROM meta", [], |r| r.get(0))
            .context("reading meta.name")
    }

    pub fn set_name(&self, name: &str) -> Result<()> {
        self.conn
            .execute("UPDATE meta SET name = ?1", params![name])
            .context("updating meta.name")?;
        Ok(())
    }

    // ── clips ────────────────────────────────────────────────────────

    /// Read all clip rows in `ord` order.
    pub fn list_clips(&self) -> Result<Vec<ClipRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, ord, source_path, start_secs, fade_in_secs, fade_out_secs, gain_db
                 FROM clips ORDER BY ord",
            )
            .context("preparing clip select")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ClipRow {
                    id: r.get(0)?,
                    ord: r.get(1)?,
                    source_path: r.get(2)?,
                    start_secs: r.get::<_, f64>(3)? as f32,
                    fade_in_secs: r.get::<_, f64>(4)? as f32,
                    fade_out_secs: r.get::<_, f64>(5)? as f32,
                    gain_db: r.get::<_, f64>(6)? as f32,
                })
            })
            .context("querying clips")?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Replace every clip row in one transaction (delete-all-then-insert).
    /// Simpler than diffing for the v1 UI surface — the clip count is
    /// small (album-sized) and the BLOB-free clip rows are tiny.
    pub fn replace_clips(&mut self, clips: &[ClipRow]) -> Result<()> {
        let tx = self.conn.transaction().context("begin replace-clips txn")?;
        tx.execute("DELETE FROM clips", [])
            .context("clearing clips")?;
        for c in clips {
            tx.execute(
                "INSERT INTO clips (ord, source_path, start_secs, fade_in_secs, fade_out_secs, gain_db)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    c.ord,
                    c.source_path,
                    c.start_secs as f64,
                    c.fade_in_secs as f64,
                    c.fade_out_secs as f64,
                    c.gain_db as f64,
                ],
            )
            .context("inserting clip")?;
        }
        tx.commit().context("commit replace-clips txn")?;
        Ok(())
    }

    // ── mix_run (mirrors the .tib API) ───────────────────────────────

    pub fn read_mix_run_header(&self) -> Result<Option<MixRunHeader>> {
        self.conn
            .query_row(
                "SELECT sample_rate, channels, frames, source_signature, created
                 FROM mix_run WHERE id = 1",
                [],
                |r| {
                    Ok(MixRunHeader {
                        sample_rate: r.get::<_, i64>(0)? as u32,
                        channels: r.get::<_, i64>(1)? as u16,
                        frames: r.get::<_, i64>(2)? as u64,
                        source_signature: r.get(3)?,
                        created: r.get(4)?,
                    })
                },
            )
            .optional()
            .context("reading mix_run header")
    }

    pub fn read_mix_run_audio(&self) -> Result<Option<Vec<u8>>> {
        let exists = self
            .conn
            .query_row("SELECT 1 FROM mix_run WHERE id = 1", [], |_| Ok(()))
            .optional()
            .context("checking mix_run presence")?;
        if exists.is_none() {
            return Ok(None);
        }
        let blob = self
            .conn
            .blob_open(DatabaseName::Main, "mix_run", "audio", 1, true)
            .context("opening mix_run BLOB")?;
        let mut buf = Vec::with_capacity(blob.len());
        let mut blob = blob;
        blob.read_to_end(&mut buf).context("reading mix_run BLOB")?;
        Ok(Some(buf))
    }

    pub fn write_mix_run(
        &mut self,
        sample_rate: u32,
        channels: u16,
        frames: u64,
        source_signature: &str,
        audio_wav_bytes: &[u8],
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO mix_run (id, sample_rate, channels, frames, source_signature, created, audio)
                 VALUES (1, ?1, ?2, ?3, ?4, datetime('now'), ?5)
                 ON CONFLICT(id) DO UPDATE SET
                   sample_rate = excluded.sample_rate,
                   channels    = excluded.channels,
                   frames      = excluded.frames,
                   source_signature = excluded.source_signature,
                   created     = excluded.created,
                   audio       = excluded.audio",
                params![
                    sample_rate as i64,
                    channels as i64,
                    frames as i64,
                    source_signature,
                    audio_wav_bytes,
                ],
            )
            .context("writing mix_run row")?;
        Ok(())
    }

    #[allow(dead_code)] // parity with the .tib CRUD; UI consumer is deferred
    pub fn delete_mix_run(&mut self) -> Result<()> {
        self.conn
            .execute("DELETE FROM mix_run WHERE id = 1", [])
            .context("deleting mix_run row")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("tbss-tba-{}-{}.tba", name, std::process::id()));
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

    #[test]
    fn create_and_clips_round_trip() {
        let p = scratch("clips");
        let mut db = TbaDb::create(&p, "Test Album").unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(db.name().unwrap(), "Test Album");
        assert!(db.list_clips().unwrap().is_empty());

        let clips = vec![
            ClipRow {
                id: 0,
                ord: 0,
                source_path: "C:/stems/intro.tib".into(),
                start_secs: 0.0,
                fade_in_secs: 0.5,
                fade_out_secs: 2.0,
                gain_db: -1.0,
            },
            ClipRow {
                id: 0,
                ord: 1,
                source_path: "C:/stems/verse.tib".into(),
                start_secs: 30.0,
                fade_in_secs: 2.0,
                fade_out_secs: 2.0,
                gain_db: 0.0,
            },
        ];
        db.replace_clips(&clips).unwrap();
        let back = db.list_clips().unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].source_path, "C:/stems/intro.tib");
        assert_eq!(back[1].start_secs, 30.0);
        assert_eq!(back[1].ord, 1);

        // Set name persists.
        db.set_name("Renamed").unwrap();
        assert_eq!(db.name().unwrap(), "Renamed");
        cleanup(&p);
    }

    #[test]
    fn mix_run_round_trips_and_upsert_replaces() {
        let p = scratch("mixrun");
        let mut db = TbaDb::create(&p, "Mix").unwrap();
        assert!(db.read_mix_run_header().unwrap().is_none());

        let wav1: Vec<u8> = (0..1024).map(|i| (i % 251) as u8).collect();
        db.write_mix_run(48_000, 2, 256, "sig-1", &wav1).unwrap();
        let h1 = db.read_mix_run_header().unwrap().unwrap();
        assert_eq!(h1.sample_rate, 48_000);
        assert_eq!(h1.frames, 256);
        assert_eq!(db.read_mix_run_audio().unwrap().unwrap(), wav1);

        let wav2: Vec<u8> = (0..2048).map(|i| ((i * 7) % 251) as u8).collect();
        db.write_mix_run(44_100, 1, 1024, "sig-2", &wav2).unwrap();
        let h2 = db.read_mix_run_header().unwrap().unwrap();
        assert_eq!(h2.sample_rate, 44_100);
        assert_eq!(h2.channels, 1);
        assert_eq!(db.read_mix_run_audio().unwrap().unwrap(), wav2);

        db.delete_mix_run().unwrap();
        assert!(db.read_mix_run_header().unwrap().is_none());
        cleanup(&p);
    }

    #[test]
    fn reopen_round_trips() {
        let p = scratch("reopen");
        {
            let _db = TbaDb::create(&p, "Reopen").unwrap();
        }
        let db = TbaDb::open(&p).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(db.pragma_string_journal(), "wal");
        cleanup(&p);
    }

    impl TbaDb {
        fn pragma_string_journal(&self) -> String {
            self.conn
                .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
                .unwrap_or_default()
        }
    }
}
