# TBSS-FR-0007: The `.tib` container — single-file SQLite projects with stem revision history

**Status**: 📝 Proposed
**Author(s)**: ophiocus
**Filed**: 2026-05-23 · **Revised**: 2026-05-23 (substrate **ZIP → SQLite**)

## Summary

Replace the folder-based project format (a `project.tinybooth` JSON
manifest plus loose sibling WAVs) with a **single self-contained `.tib`
file that is a SQLite database**. One file per project holds
**everything** — every stem, the full revision history of every stem
(destructive binary snapshots *and* non-destructive config snapshots),
the bundled mixdown reference, and all console state — with **atomic
transactional saves, native rollback, and WAL crash-safety**.

> **Substrate note.** This RFC was first specced as a ZIP container
> (rename → `.zip`, browse anywhere). Phase-1 prototyping plus a survey
> of how real apps use ZIP for live storage killed that direction — see
> §"Why SQLite, not ZIP". The browsability goal is deliberately dropped;
> the live working file is a database, inspected with a SQLite tool.

## Motivation / Problem

The folder format (`src/project.rs`) has three structural gaps:

1. **Not portable as one artefact.** A project is a directory of loose
   files; moving/sharing/backing-up means zipping by hand.
2. **No history.** Project-wide Trim (`src/trim.rs`) crops WAVs in place
   and keeps nothing; a bad trim is unrecoverable. Non-destructive edits
   (corrections/gain/automation) keep only the current value.
3. **No grouping or alternates.** Every track is a flat peer; no stem
   that holds alternate takes, no per-stem history.

We want one file per project; full revision history (audio bytes *and*
console config over time); stems organised as named groups each loadable
into the Mix tab with their own history; and crash-safe, cheap saves.

## Why SQLite, not ZIP

Phase 1 built a ZIP container (`src/tib.rs`, since removed) and hit a
structural wall, corroborated by a survey of shipping apps:

- **ZIP has no in-place update.** Changing or deleting any entry forces a
  full-archive rewrite; only *appending new* entries is cheap. The ZIP
  writer also **rejects duplicate filenames**, so you cannot cheaply
  "overwrite" a fixed-name entry (e.g. `latest.wav`, `manifest.json`).
- **Every ZIP-based app format proves this.** OOXML (`.docx/.xlsx`),
  OpenDocument, Krita `.kra`, Sketch, iWork all **full-rewrite on save**;
  EPUB, KMZ, 3MF, USDZ are **read-mostly / write-once**. None mutates a
  live file in place. (USDZ's uncompressed, 64-byte-aligned ZIP exists
  only for memory-mapped *read* speed, not updates.)
- **Our profile is the textbook SQLite case.** A 700 MB file with
  *frequent tiny metadata saves* (corrections/automation on nearly every
  action) **+ per-stem revision snapshots + rollback + crash-safety** is
  exactly what sqlite.org's "application file format" essay contrasts
  against formats that "rewrite the entire document to change a single
  byte." **Audacity 3.0 is the on-point precedent**: an audio app that
  migrated off a multi-file project format to single-file SQLite
  (`.aup3`) so a project can't be broken by moving files, and so saves
  touch only changed pages.

Cost accepted: we lose "rename → `.zip`, open in Explorer." Browse a
`.tib` with the `sqlite3` CLI or DB Browser for SQLite instead.

Dependency added: **`rusqlite`** (bundled SQLite, the `bundled` feature
— no system SQLite needed).

## Schema (SQL)

```sql
PRAGMA journal_mode = WAL;        -- atomic commits across power loss
PRAGMA synchronous  = NORMAL;     -- durable enough for a desktop app
PRAGMA foreign_keys = ON;
PRAGMA auto_vacuum  = INCREMENTAL;-- reclaim pruned-revision space cheaply
-- page_size = 16384 set once, before the first table is created (big BLOBs)

CREATE TABLE meta (              -- single-row project + app metadata
  schema_version INTEGER NOT NULL,
  name TEXT, created TEXT, kind TEXT,            -- Standard|Recordings|TinyDAW
  master_gain_db REAL,
  master_gain_automation TEXT,                   -- JSON
  corrections_disabled INTEGER,
  default_correction TEXT,                       -- JSON Profile | null
  suno_mixdown_track_id TEXT, suno_mixdown_lufs REAL,
  song_key_estimate TEXT,                        -- JSON
  next_suno_ordinal INTEGER
);

CREATE TABLE stems (            -- a named group
  id TEXT PRIMARY KEY, name TEXT NOT NULL, ord INTEGER
);

CREATE TABLE tracks (           -- a track in a stem; loads into the Mix tab
  id TEXT PRIMARY KEY,
  stem_id TEXT NOT NULL REFERENCES stems(id) ON DELETE CASCADE,
  name TEXT NOT NULL, ord INTEGER,
  source TEXT,                                   -- JSON TrackSource
  sample_rate INTEGER, stereo INTEGER, duration_secs REAL,
  current_rev_id INTEGER REFERENCES revisions(id),   -- the live audio
  -- live console head (what the Mix tab edits right now):
  correction TEXT, gain_db REAL, polarity_inverted INTEGER,
  gain_automation TEXT, telemetry TEXT,          -- JSON
  loaded_in_mix INTEGER, mute INTEGER
);

CREATE TABLE revisions (        -- binary audio snapshots (the history)
  id INTEGER PRIMARY KEY,
  track_id TEXT NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,                            -- 'orig' | 'destructive'
  created TEXT, label TEXT, pinned INTEGER DEFAULT 0,
  sample_rate INTEGER, stereo INTEGER, duration_secs REAL,
  audio BLOB NOT NULL                            -- the WAV bytes
);

CREATE TABLE config_revs (      -- non-destructive snapshots (no audio)
  id INTEGER PRIMARY KEY,
  track_id TEXT NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
  created TEXT, label TEXT,
  correction TEXT, gain_db REAL, polarity_inverted INTEGER, gain_automation TEXT
);
```

`stems` = your named groups; `tracks` = the alternates that load into the
Mix tab; `revisions` = the audio history; `config_revs` = the
non-destructive history. The existing per-track config that lives in the
manifest today moves onto the `tracks` head + JSON columns.

## Revision model (your decisions, SQLite-expressed)

The `orig` / `latest` / `rev-NNN` scheme maps cleanly onto rows — and
gets *better*, because rollback becomes a pointer update instead of a
file copy:

- **`orig`** → a `revisions` row with `kind='orig'`. Pristine import,
  **never pruned**.
- **"latest" / current audio** → the `tracks.current_rev_id` pointer.
  The player resolves it with one indexed `SELECT audio FROM revisions
  WHERE id = current_rev_id` and streams the BLOB. (The original
  "stable read path" win is preserved — a single pointer lookup — and
  there's no duplicate copy on disk.)
- **`rev-NNN`** → `revisions` rows with `kind='destructive'`.

Edits, by kind and timing:

- **Destructive op, on _execute_** (Trim; future normalize-bake,
  re-import-replace): `INSERT` a `revisions` row with the new audio BLOB,
  `UPDATE tracks.current_rev_id` to it, prune to FIFO-5 (§Retention) —
  all in **one transaction**. **Rollback = `UPDATE current_rev_id` to a
  chosen revision** (no byte copy).
- **Non-destructive op, on _save_**: `UPDATE` the `tracks` head columns
  (correction/gain/…) and `INSERT` a `config_revs` snapshot. Tiny
  transaction; SQLite writes only the changed pages — **no audio
  touched, no 700 MB rewrite**.

On **import**: in one transaction, insert the stem, the track, the
`kind='orig'` revision (the WAV BLOB), and set `current_rev_id` to it.

## Save protocol — transactions, not rewrites

Every save is a SQLite transaction. A metadata save is an `UPDATE` of a
few small rows → SQLite rewrites only the touched pages (kilobytes). A
destructive save adds one BLOB row. **No append/compaction machinery,
no central directory, no temp-file-rewrite of the whole project** — the
thing that made ZIP unworkable here. WAL lets a save commit atomically
and durably; a crash leaves the last committed transaction intact.

## Retention / auto-prune

Per track: keep the `orig` revision + the **last 5 `destructive`
revisions** (FIFO) + any `pinned`. Creating a 6th destructive revision
`DELETE`s the oldest unpinned one; `PRAGMA incremental_vacuum` reclaims
its pages (no full `VACUUM` needed). `config_revs` are tiny (no audio)
and kept liberally. Net: roll back through the last 5 destructive edits,
or all the way to `orig`.

## BLOBs

Stems are ~55–80 MB BLOBs — past SQLite's ~100 KB "internal blob" sweet
spot, but the single-file mandate overrides that guidance:

- `page_size = 16384` (set before first write) for large-BLOB throughput.
- Read via **incremental BLOB I/O** (`rusqlite::blob::Blob` /
  `sqlite3_blob_open`) so the owner-thread streams a stem into `hound`
  without a second full-size buffer. (The player still ends up holding
  the decoded `Vec<i16>` in RAM, as today.)
- Default per-BLOB limit is ~1 GB (`SQLITE_MAX_LENGTH`); 80 MB stems are
  comfortable. Whole-DB limit is effectively unbounded for us (281 TB).

## Crash-safety

WAL + `synchronous=NORMAL` gives atomic, durable commits without
rewriting the file. **Honest caveat:** while a `.tib` is *open*, WAL
creates `-wal` and `-shm` sidecar files; they're checkpointed and removed
on a clean close, so the project is a single file *at rest*. (If strict
single-file-even-while-open ever matters, `journal_mode=TRUNCATE` trades
some concurrency for it — not needed for a single-user desktop app.)

## Recordings filespace as `.tib`

The app-owned recordings filespace becomes a SQLite `.tib` too. Recording
still streams to a temp loose WAV (the writer needs a seekable header
patch); on stop, the take is inserted as a stem/track/`orig` revision in
one transaction. Append-heavy and growing, so retention + incremental
vacuum matter most here.

## Implementation — the phased lift (SQLite)

| Phase | Work | Size |
|---|---|---|
| 1 | `TibDb` module over `rusqlite` (bundled): open/create, PRAGMAs/WAL, schema + migrations, BLOB read (incremental) / write, transaction helpers. Replaces the removed ZIP `tib.rs`. + tests. | M |
| 2 | Project save/load over SQLite; rewrite every asset-path site (`project.rs`, `player.rs`, `trim.rs`, `export.rs`, `suno_import.rs`): `Track.file`→`current_rev_id` BLOB, `Project.root`→`db_path`, `track_abs_path`→`read_current_audio`. | **L** |
| 3 | Owner-thread reads the current-rev BLOB (the v0.4.40 two-phase build snapshot carries `(db_path, rev_id)` instead of a `PathBuf`). | M |
| 4 | Stem/track/revision/config schema + **folder-project migration** (each existing track → a stem with one track + one `orig` revision from its WAV). | L |
| 5 | Destructive ops in a transaction: insert revision + repoint `current_rev_id` + FIFO-5 prune + incremental_vacuum. | M |
| 6 | UI: stem-group browser + per-track revision/config history + restore (repoint) / pin. | **L** (design-heavy) |
| 7 | Recordings filespace → `.tib`. | M |
| 8 | WAL/pragmas/retention/checkpoint-on-close tuning; crash-safety tests. | M |
| 9 | Export / import / file-dialog (`*.tib`) / `config.last_project_path` + `recent_projects`. | M |

**MVP that proves it:** phases 1–3 + 5 + the migration slice of 4 —
SQLite container, load/save, player reads the current BLOB, destructive
Trim writes a revision + rollback, legacy folder import. UI browser (6),
recordings-as-`.tib` (7), retention tuning (8) follow.

## Risks

- **Large BLOBs in SQLite.** Past the recommended internal-blob size;
  validate read/write throughput + memory in phase 1 (incremental BLOB
  I/O, page_size 16384) before committing the breadth-y phase 2.
- **DB growth & vacuum cost.** Full `VACUUM` on a multi-GB DB is
  expensive; rely on `auto_vacuum=INCREMENTAL` + FIFO-5 so we never need
  a full vacuum on the hot path.
- **WAL sidecars during a session** (`-wal`/`-shm`): "one file" holds at
  rest, not mid-edit. Documented above.
- **Refactor breadth (phase 2)** — same as before: every track→file site
  changes shape once; needs the `TibDb` abstraction solid first.
- **New dependency** (`rusqlite` bundled): build time + binary size
  increase; mitigated by it being a single well-maintained crate.
- **No zip-app browsability** — accepted trade per the substrate
  decision.

## Open questions

1. **page_size / auto_vacuum** final values — confirm 16384 + INCREMENTAL
   against real multi-stem throughput in phase 1.
2. **Stream vs load BLOBs** — incremental BLOB I/O into `hound` vs a
   single `read` of the whole BLOB (the player buffers the decoded
   samples either way).
3. **WAL checkpoint cadence** — on close only, or periodic.
4. **Mixdown** — model as a special stem/track row vs dedicated `meta`
   columns (`suno_mixdown_track_id`).
5. **Schema migrations** — `schema_version` in `meta`; how aggressively
   to migrate older `.tib` schemas in place.

## Success criteria

- A project is one `.tib` SQLite file (single file at rest).
- A metadata save touches only changed pages — **no 700 MB rewrite** —
  and is atomic/crash-safe (WAL).
- The player reads current audio via the `current_rev_id` pointer.
- Destructive Trim is reversible: roll back through the last **5**
  destructive edits, or to `orig`, by repointing `current_rev_id` (no
  byte copy).
- A non-destructive console change is restorable from a `config_revs`
  snapshot without storing a second copy of the audio.
- A legacy folder project migrates into a `.tib` losslessly on open.
- A power-loss mid-save leaves the last committed state intact.
