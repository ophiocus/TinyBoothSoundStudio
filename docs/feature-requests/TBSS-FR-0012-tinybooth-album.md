# TBSS-FR-0012 — TinyBooth Album (`.tba`): N-stem composition format

| Field | Value |
|---|---|
| Status | 🔧 In progress (MVP scoped for v0.4.52) |
| Depends on | TBSS-FR-0007 (`.tib` container), TBSS-FR-0011 (`.tib` mix-run cache) |
| Supersedes | The "Master Render tab" idea floated alongside FR-0011 §B |

## Executive summary

Add a second top-level project format, `.tba` (TinyBooth Album), purpose-built for arranging N bounced `.tib` projects into a linear timeline with per-clip gain + fade-in / fade-out + freely-positioned start times. Add an Album tab that edits the `.tba`, renders it (live preview + bounce-to-cache + export), and treats the album's own bounced master the same way `.tib` does — so an album can itself be loaded as a stem in another album. Recursive composition for free.

## Problem

After FR-0011, `.tib` is two things at once:
1. A source project — multitrack tracks + revisions + master-chain config.
2. A buffered stem wrapper — the single-row `mix_run` cache lets a `.tib` be loaded as a stem in the Crossfade tab.

The Crossfade tab handles **two stems**. The use case we keep hitting (and that the user has been describing across multiple feature passes) is bigger: **assemble N finished projects into an album / long-form piece / mix tape**. The Crossfade-tab's transition model and two-track UI can't grow into that without becoming something else. So we add a sibling format whose data model IS "arrangement of N stems," and a sibling tab whose UI IS for editing that.

## Proposal

### File format — `.tba` (TinyBooth Album)

Single SQLite file, same flavor as `.tib` (WAL, 16 KiB page size, foreign keys ON). Schema v1:

```sql
CREATE TABLE meta (
  schema_version INTEGER NOT NULL,
  name TEXT,
  created TEXT
);

CREATE TABLE clips (
  id INTEGER PRIMARY KEY,
  ord INTEGER NOT NULL,           -- display order, dense [0..N)
  source_path TEXT NOT NULL,      -- absolute path to a .tib (or .wav, future)
  start_secs REAL NOT NULL,       -- clip start on the album timeline
  fade_in_secs REAL NOT NULL,
  fade_out_secs REAL NOT NULL,
  gain_db REAL NOT NULL
);

CREATE TABLE mix_run (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  sample_rate INTEGER NOT NULL,
  channels INTEGER NOT NULL,
  frames INTEGER NOT NULL,
  source_signature TEXT NOT NULL,
  created TEXT NOT NULL,
  audio BLOB NOT NULL             -- 16-bit WAV stream, same shape as .tib's mix_run
);
```

The `mix_run` table is **structurally identical** to the one we added to `.tib` in FR-0011, on purpose: the bounced album is itself a stem, and the Crossfade tab + a future Album-loads-Album path consume both via the same WAV-bytes decode path.

### In-memory model

```rust
pub struct Album {
    pub name: String,
    pub clips: Vec<AlbumClip>,
}

pub struct AlbumClip {
    pub source_path: PathBuf,    // .tib (or .wav, later)
    pub start_secs: f32,         // timeline position
    pub fade_in_secs: f32,
    pub fade_out_secs: f32,
    pub gain_db: f32,
}
```

### Render DSP

Linear time-domain sum:

1. Decode each clip's audio. For `.tib` sources: open, read `mix_run` blob, decode through the same `decode_wav_reader_as_stereo` path the Crossfade tab uses. Refuse clips whose source has no bounce yet (clear status message: "<name>.tib has no bounced mix — open and Bounce first").
2. All clips must share a sample rate (no resampling, same constraint the Crossfade tab and `export::mixdown` enforce). First clip wins; mismatches error out.
3. Output buffer length = `max(start_secs + duration_secs)` across all clips, rounded to whole frames.
4. For each clip:
   - Apply equal-power fade-in (`sin²(t·π/2)`) over the first `fade_in_secs * sr` frames.
   - Apply equal-power fade-out (`cos²(t·π/2)`) over the last `fade_out_secs * sr` frames.
   - Multiply by per-clip linear gain.
   - Sum into the output buffer at offset `start_secs * sr`.
5. Soft-limit to `[-1, 1]` if any sample exceeds.

Crossfade between adjacent clips is **emergent**: clip N's fade-out and clip N+1's fade-in overlap on the timeline if their fade durations + positions cause them to overlap; the equal-power curves sum to constant power. No explicit "transition" data structure — it's implicit in the start/fade values.

### UI — Album tab

New `Tab::Album` variant. Tab renders:

- **Top row**: album name input, Open / Save / Save As, ⤓ Bounce, Export.
- **Clip list** (one row per clip): `#`, source filename, `start [_____]s`, `fade-in [_____]s`, `fade-out [_____]s`, `gain [_____]dB`, `▲▼✖` reorder + remove. `+ Add Clip…` button at the bottom — `.tib` file dialog (`.wav` deferred).
- **Timeline strip** (top-down): each clip rendered as a colored band at its `[start, start+duration]` range with fade shading at the edges; nothing draggable in v0.4.52 (timeline editing is v0.4.53 polish).
- **Transport**: ▶ Preview (renders + plays via `CrossfadePreviewSession`), ■ Stop. ⤓ Bounce writes the rendered audio to `mix_run`; pip shows fresh / stale / none like the Mix tab.

### Album ↔ `.tib` symmetry

The two formats share a base: SQLite + WAL + a `mix_run` table holding a WAV-stream blob. Their **distinct surfaces** are:

| | `.tib` | `.tba` |
|---|---|---|
| Owns | multitrack source (`tracks`, `revisions`, `config_revs`) | composition (`clips` referencing other files) |
| Edited in | Mix / Record / Generator / Trim tabs | Album tab |
| Bounce produces | the project's master mix | the album's master mix |
| Loadable as a stem in Crossfade | yes (FR-0011 §B) | yes (same `mix_run` decode path) |
| Loadable as an album clip | yes | yes (recursive composition) |

## Risks

- **Source-path stability.** Storing absolute paths breaks portability when the user moves the album folder. v0.4.52 stores absolute paths only; v0.4.53 will add relative-path resolution (try-relative-then-absolute on load).
- **Sample-rate lock-in.** Same constraint as everywhere else in the app — clips must share a rate. Acceptable; the error message tells the user what to do (re-export the offending source).
- **Mix-run growth.** A bounced album's `mix_run` blob is the same size as a `.tib`'s for the same duration (~28 MiB / 5 minutes stereo 16-bit at 48 kHz). Acceptable.

## Open questions

Deferred past v0.4.52 MVP:

- **Timeline editing.** Drag clips and fade handles on the timeline strip (Crossfade-tab-style). v0.4.53.
- **`.wav` as a clip source.** v0.4.52 takes `.tib` only (forces the user through the bounce step). `.wav` clips are a one-line addition once the `.tib` path proves out.
- **Relative source paths.** v0.4.53.
- **Per-clip correction/EQ.** Clips render with whatever the source `.tib`'s bounce produced. Per-clip post-correction (a filter chain applied to the source's bounced audio at album-render time) is a deferred extension.

## Success criteria

- New `tba::tests::create_and_clips_round_trip` + `tba::tests::mix_run_round_trips_and_upsert_replaces` (mirrors the `tib` tests).
- New `album::tests::render_two_clips_equal_power_overlap_sums_to_one_in_power` (DSP correctness).
- Full suite remains green (≥ 130/130 with the new tests).
- Manual: create an album with two bounced `.tib` clips, set a 2-second overlap with 2-second fades, hear an equal-power crossfade. Bounce, see pip turn green. Export.
