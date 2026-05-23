# TBSS-FR-0007: The `.tib` container — single-file projects with stem revision history

**Status**: 📝 Proposed
**Author(s)**: ophiocus
**Filed**: 2026-05-23

## Summary

Replace the current folder-based project format (a `project.tinybooth`
JSON manifest plus loose sibling WAVs) with a **single self-contained
`.tib` file**: an ordinary ZIP with opinionated internal scaffolding.
One file per project holds **everything** — every stem, every revision
of every stem (destructive *and* non-destructive history), the bundled
mixdown reference, and all console state (corrections, gain, automation,
master bus).

The on-disk shape is deliberately boring: `.tib` **is** a ZIP, so
renaming it to `.zip` opens it in any archive tool. The app's reader is
**tolerant of "dirt"** — it resolves only the assets the manifest
references and ignores anything else in the archive; the writer
**preserves** unknown entries on rewrite, so a teammate can drop notes
or extra files in via a ZIP app without the app clobbering them.

The recordings filespace becomes a `.tib` too, so the "one file,
everything inside" rule is uniform across project kinds.

## Motivation / Problem

The folder format (documented in `src/project.rs`) has served since
v0.1, but it has three structural gaps:

1. **Projects are not portable as one artefact.** A project is a
   directory of loose files; moving, backing up, or sharing it means
   zipping it by hand and hoping the relative paths survive. Renaming
   or relocating the folder is fine (paths are relative), but the
   project is never *one thing*.

2. **There is no history.** The project-wide Trim
   (`src/trim.rs`) is **destructive** — it crops every WAV in place
   (`.tmp` + rename) and keeps nothing. A bad trim is unrecoverable
   without an external backup. Non-destructive edits (corrections,
   gain, automation) live only as the current value in the manifest;
   there is no way to step back to an earlier console state.

3. **Stems have no grouping or alternates.** Every track is a flat
   peer in `Project.tracks`. There is no notion of a stem that holds
   several alternate takes/versions, nor of revision history per stem.

We want: one file per project; full revision history for stems (both
the audio bytes *and* the console config over time); stems organised as
named groups that each load into the Mix tab carrying their own
history; and an archive that any ZIP tool can crack open.

## Proposal

### 1. The container

A `.tib` file is a ZIP archive (we already depend on `zip` v2 for
read-only Suno import; this RFC adds the write path).

- **Audio entries (`*.wav`) are STORE'd (uncompressed).** PCM barely
  deflates, and a STORE entry is a contiguous byte range in the file —
  so a stem can be read by seeking to its offset and streaming it into
  `hound` with **no full-archive decompression and near-zero extra
  RAM**. This is what lets a ~700 MB container behave like loose files
  on the player's hot path.
- **`manifest.json` is DEFLATE'd** (small, compressible).
- **Tolerant reader, preserving writer.** Load reads `manifest.json`,
  then resolves only the entry paths it references. Everything else is
  "dirt": ignored on read, copied through on rewrite.

`.tib` ⇄ `.zip` is a pure rename; the format carries no proprietary
framing.

### 2. Internal layout

```
project.tib                         (a ZIP)
├── manifest.json                   index + all console state (source of truth)
├── stems/
│   └── <stem_named>/               a stem = a named GROUP
│       └── <Track Name>/           a track in the stem — loads into the Mix tab
│           ├── orig.wav            pristine import — immutable, never pruned
│           ├── latest.wav          the CURRENT audio — player/seek always reads THIS
│           ├── rev-001.wav         destructive snapshot (binary), FIFO depth 5
│           └── rev-002.wav
├── mixdown/
│   └── <name>/{orig,latest,rev-NNN}.wav   bundled Suno mixdown (same scheme)
├── console/                        (optional) bulky gesture recordings if moved
│                                   out of the manifest
└── <anything else>                 dirt: reader ignores, writer preserves
```

Three levels under `stems/`: **stem (group) → track → revisions.**

- A **stem** is a named group (`stems/<stem_named>/`). It groups one or
  more tracks.
- A **track** (`stems/<stem_named>/<Track Name>/`) is the thing that
  loads into the Mix tab — an alternate take/version of the stem. Most
  stems will have exactly one track; the level exists so a stem can
  hold alternates (take A / take B / a re-import) without flattening
  them into unrelated peers.

Each track folder holds three kinds of WAV (the §3 scheme):

- **`orig.wav`** — the pristine import. **Immutable, never pruned.** The
  ultimate "restore to factory" source.
- **`latest.wav`** — the current working audio. **The player and every
  seek operation read this one, stable filename** — no "which revision
  is current?" lookup, ever. Overwritten in place whenever the current
  audio changes.
- **`rev-NNN.wav`** — committed destructive snapshots, the rollback
  history. A bounded FIFO (depth 5; §6).

Folder and file names are **human-readable on purpose** (zip-app
browsing). The `manifest.json` is the index that maps logical refs to
exact entry paths, so names can stay readable *and* survive renames
(the manifest path updates; the tolerant reader never guesses).

### 3. Revision model — `orig` / `latest` / `rev-NNN`

The read path and the history are deliberately separated:

- **`latest.wav` is always the current audio.** The player resolves a
  track straight to `…/<Track Name>/latest.wav` — a single, stable path
  — so seeks and loads never walk a revision list. This is the speed
  win: one known filename per track, always.
- **`orig.wav` is the pristine import, kept forever** (never pruned).
  Always a valid "restore to original" target.
- **`rev-NNN.wav` are committed binary snapshots** — the rollback
  history, bounded to the last 5 (§6).

Edits split by *kind* and by *when* they write:

- **Destructive op, on _execute_** (Trim today; future normalize-bake,
  re-import-replace) → immediately **write a new `rev-NNN.wav`** (the
  committed snapshot) **and overwrite `latest.wav`** with the new audio.
  History grows by one binary snapshot; the player keeps reading
  `latest.wav`. **Rolling back = copy a chosen `rev-NNN.wav` (or
  `orig.wav`) over `latest.wav`** — a single, consistent operation per
  track.
- **Non-destructive op (correction / gain / polarity / automation), on
  _save_** → append a **manifest-only revision** (a config snapshot).
  No audio is written; `latest.wav` is untouched. This is the
  "non-destructive history alike" capability at ~zero storage cost.

So there are two history streams: **binary** snapshots (`rev-NNN.wav`,
written on destructive execute, FIFO-5) and **config** snapshots
(manifest revisions, written on save). A manifest revision records the
config *and* which binary it applied to, so restoring it restores both
the console state and the audio it belonged with.

On **import** the app always writes `orig.wav` **and** `latest.wav` (a
copy) up front — every track starts with a pristine baseline and a live
working copy from frame one.

### 4. Manifest schema (indicative)

The manifest is the existing `Project`/`Track` model, restructured into
a **library** (stems/tracks/revisions) plus the **mix** (what's loaded
+ live console state). Field placement below is indicative, not final.

```jsonc
{
  "version": 2,                 // schema bump; v1 = folder format
  "name": "...", "created": "...", "kind": "Standard",
  // project-level (unchanged): master_gain_db, master_gain_automation,
  // corrections_disabled, default_correction, suno_mixdown_*,
  // song_key_estimate, next_suno_ordinal

  "stems": [                    // the library (groups)
    {
      "id": "stem-001", "name": "Lead Vocal",
      "tracks": [
        {
          "id": "trk-001", "name": "Take 1",
          "source": { "kind": "SunoStem", "role": "Vocals", ... },
          "sample_rate": 48000, "stereo": true, "duration_secs": 200.8,

          // AUDIO — the player ALWAYS reads `latest`; `orig` + the
          // binary revs are restore sources only.
          "orig":   "stems/Lead Vocal/Take 1/orig.wav",
          "latest": "stems/Lead Vocal/Take 1/latest.wav",
          "binary_revs": [          // destructive snapshots, FIFO depth 5
            { "rev": 1, "file": "stems/Lead Vocal/Take 1/rev-001.wav",
              "created": "...", "label": "trim 4.0–198.2s", "pinned": false }
          ],

          // LIVE console state (the head):
          "correction": { ...Profile... }, "gain_db": -2.0,
          "polarity_inverted": false, "gain_automation": { ... },
          "telemetry": { ... },

          // NON-DESTRUCTIVE history (config snapshots, appended on save):
          "config_revs": [
            { "created": "...", "label": "import",
              "correction": null, "gain_db": 0.0,
              "polarity_inverted": false, "gain_automation": null },
            { "created": "...", "label": "Suno-Clean -2 dB",
              "correction": { ...Profile... }, "gain_db": -2.0,
              "polarity_inverted": false, "gain_automation": { ... } }
          ]
        }
      ]
    }
  ],

  "mix": [                      // what's loaded into the Mix tab
    { "stem_id": "stem-001", "track_id": "trk-001", "mute": false }
  ]
}
```

Two history streams, no `current` pointer needed: `latest.wav` *is* the
current audio. **Binary** history is the `binary_revs` files (restore =
copy one over `latest.wav`); **config** history is `config_revs`,
manifest-only (restore = re-apply the snapshot to the head — no audio
written). A config change costs no WAV bytes at all.

### 5. Save protocol — append-on-save

This is the load-bearing constraint: the app marks the project dirty on
nearly every action, and today a save is cheap (rewrite a small JSON;
WAVs untouched). A `.tib` save must stay cheap, or the format is a
non-starter.

- **Append-on-save.** A config save appends a new `manifest.json` entry
  and rewrites only the (small) ZIP central directory + EOCD — kilobytes,
  not megabytes. Audio entries are touched **only** by destructive ops
  *on execute*, which append a new `rev-NNN.wav` **and** a new
  `latest.wav` (overwriting the old `latest` — in ZIP terms, appending a
  fresh `latest.wav` entry that shadows the prior one, whose bytes
  become dead until compaction). Two WAV writes per destructive edit;
  zero on a config save.
- **Debounce.** Coalesce rapid dirty actions into one manifest append;
  always flush on tab-switch and on close, so we don't append hundreds
  of manifest copies between compactions.
- **Compaction** is the only full rewrite. It drops every entry not
  referenced by the live manifest (superseded manifests, pruned
  revisions, orphaned blobs) and copies the survivors — including
  "dirt" — into a fresh archive via temp-file + atomic rename. Runs on
  explicit command and (optionally) on close.
- **Crash safety.** Append order is: write entry data → fsync → write
  new central directory + EOCD → fsync, with the previous EOCD kept
  recoverable so a torn append degrades to "last good state," never a
  corrupt archive. Compaction's temp+rename is atomic by construction.

### 6. Retention / auto-prune

Binary history is a **bounded FIFO of the last 5 destructive snapshots**
per track. Creating a 6th `rev-NNN.wav` enqueues it and **pops the
oldest for deletion** (its bytes become dead, reclaimed at the next
compaction). `orig.wav` and `latest.wav` are **exempt** — `orig` is the
permanent factory baseline, `latest` is the live audio. Pinning a `rev`
protects it from FIFO eviction. Config snapshots (`config_revs`) are
manifest-only and effectively free, so they're kept liberally (and can
be bounded later if they ever grow large).

Net effect per track: you can always roll back through the **last 5
destructive edits**, *or* all the way to `orig.wav`.

### 7. Recordings filespace as `.tib`

The app-owned recordings filespace folds into its own `.tib`. Recording
is a streaming write that needs a seekable file to patch the RIFF header
on stop, so the hot path is unchanged: **record to a temp loose WAV,
then append it into the recordings `.tib` as a new stem/track/rev on
stop.** The recordings `.tib` is append-heavy and grows with every take,
so auto-prune + compaction matter more there than for song projects.

## Implementation notes — the phased lift

| Phase | Work | Size |
|---|---|---|
| 1 | `TibContainer` abstraction over `zip`: open / windowed-read-entry / append-entry / compact / dirt-passthrough; STORE audio, DEFLATE json. Validate per-stem random-access read cost early. | M |
| 2 | Manifest as library (stems/tracks/revisions) + mix; rewrite **every** asset-path site (`project.rs`, `player.rs`, `trim.rs`, `export.rs`, `suno_import.rs`): `Track.file`→rev refs, `Project.root`→`container_path`, `track_abs_path`→`read_revision_bytes`. | **L** |
| 3 | Owner-thread reads revisions from the `.tib` (the v0.4.40 build snapshot carries entry refs, not `PathBuf`; `build_state` reads entries). | M |
| 4 | Stem/track/revision + group schema; manifest version bump v1→v2; **folder-project migration** (each existing track → a stem with one track; existing WAV becomes `orig.wav` + a `latest.wav` copy). | L |
| 5 | Destructive ops (Trim first): on execute, write a new `rev-NNN.wav` + overwrite `latest.wav`; FIFO-5 prune. The player keeps reading `latest.wav`. | M |
| 6 | UI: stem-group browser + per-track revision/layer picker + "load into Mix" + restore / pin / compact. | **L** (design-heavy) |
| 7 | Recordings filespace → `.tib` (record-to-temp → append). | M |
| 8 | Save cadence: append-on-save + debounce + compaction + crash-safety. | **L** |
| 9 | Export / import / file-dialog (`pick_file("*.tib")`) / `config.last_project_path` + `recent_projects` path changes. | M |

**MVP that proves the format:** phases 1–3 + 5 + the migration slice of
4 — container, library/mix load+save, player reads from the `.tib`,
destructive Trim keeps revisions, legacy folder import. The stem-group
browser UI (6), recordings-as-`.tib` (7), and the full
append/compaction polish (8) are fast-follows once the container is real.

## Risks

- **Save cadence (highest).** If a save ever rewrites the whole
  container, the format is dead. Append-on-save + debounce is therefore
  a requirement, not an optimisation, and it shapes phases 1/2/8.
- **Append crash-safety.** A half-written central directory/EOCD can
  confuse readers (they scan backward for the EOCD). The fsync ordering
  and recoverable-previous-EOCD scheme must be designed and tested
  deliberately; risky ops (compaction) go through temp+rename.
- **Random access / memory.** Must never load the whole ZIP to read one
  stem. STORE + the `zip` crate's windowed entry reader (or mmap +
  offset) keeps per-stem reads as cheap as loose files — validate in
  phase 1 before committing.
- **Refactor breadth.** Phase 2 touches every code path that resolves a
  track to a file. High surface area; needs the `TibContainer`
  abstraction solid first so call sites change shape once.
- **Single-writer assumption.** Editing the `.tib` in an external ZIP
  tool while the app holds it open is unsupported; document it. The
  tolerant reader/preserving writer makes *offline* external edits
  safe.

## Open questions

1. ~~`N` for auto-prune.~~ **Resolved:** depth **5** per track, FIFO,
   `orig`/`latest` exempt, pins protected. Open sub-question only:
   whether 5 is later exposed as a config knob.
2. **Readable folder names vs renames.** Names are readable and the
   manifest indexes by path; confirm the rename flow updates the
   manifest path (and whether we ever rename the in-zip folder or just
   the manifest's display name with a stable path).
3. **Console gesture recordings** — keep automation in `manifest.json`
   (current) or move bulky lanes to `console/` entries? Only matters if
   automation grows large.
4. **Compaction trigger** — on close always, on command only, or a
   size-threshold prompt.
5. **Mixdown grouping** — model the Suno mixdown as a special stem under
   `mixdown/` (revisioned like any other) vs a dedicated manifest field
   (current `suno_mixdown_path`).

## Success criteria

- A project is one `.tib` file; renaming to `.zip` opens it in any
  archive tool and shows human-readable
  `stems/<stem>/<track>/{orig,latest,rev-NNN}.wav`.
- The player always reads `latest.wav` (no revision lookup on the hot
  path).
- Destructive Trim is reversible: you can roll back through the **last 5**
  destructive edits, or all the way to `orig.wav`, from the UI.
- A non-destructive console change is restorable without storing a
  second copy of the audio (config snapshot only).
- Opening / saving a multi-stem project is **not slower** than the
  folder format on the common path (config save stays sub-100 ms; lanes
  render as soon as audio decodes per v0.4.40).
- A legacy folder project migrates to `.tib` losslessly on open.
- Dropping an extra file into the `.tib` via a ZIP tool survives an
  app save (dirt preserved).
