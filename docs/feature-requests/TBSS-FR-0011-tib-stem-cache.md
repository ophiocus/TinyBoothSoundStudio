# TBSS-FR-0011 — `.tib` as buffered stem (mix-run cache) + Crossfade-of-.tib

| Field | Value |
|---|---|
| Status | ✅ Landed (MVP) |
| Landed in | v0.4.50 |
| Depends on | TBSS-FR-0007 (`.tib` container), TBSS-FR-0010 (Crossfade tab) |
| Replaces | — |

## Executive summary

Extend each `.tib` project with a single-row **mix-run cache** holding a buffered render of that project's master mix, and teach the Crossfade tab to load a `.tib` as a single stem by reading that cache. This turns finished TinyBooth projects into composable units — you can drop two `.tib` files into the Crossfade tab and stitch them, without re-running the source DSP or staging an intermediate `.wav`.

## Problem

Until v0.4.49, a `.tib` was a multitrack source — you could open it, edit it, and Export it to a separate `.wav`. To use a finished project as a stem in another tab (the Crossfade tab in particular), you had to:

1. Open the source `.tib`.
2. Export to a sibling `.wav`.
3. Load the `.wav` into the Crossfade tab.
4. Repeat for every other source.
5. Keep the `.wav` files in sync with the `.tib` if you ever go back and edit.

That's a fragile manual loop with two copies of each rendered mix on disk. The unit a user thinks in — "this finished piece" — was not a unit the app could pass around.

## Proposal

### Phase A — Embed the buffered mix-run inside the `.tib`

Add a single-row `mix_run` table to the `.tib` schema:

```sql
CREATE TABLE mix_run (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  sample_rate INTEGER NOT NULL,
  channels INTEGER NOT NULL,
  frames INTEGER NOT NULL,
  source_signature TEXT NOT NULL,
  created TEXT NOT NULL,
  audio BLOB NOT NULL          -- complete 16-bit WAV stream
);
```

- **Audio format.** The blob is a complete WAV byte stream (header + PCM). Same format as the existing per-track revision BLOBs, so consumers can decode via `hound::WavReader::new(Cursor::new(bytes))` exactly like the .wav path.
- **Source signature.** A stable hash over the project state that influences the rendered bytes — each track's id + `current_rev_id` + mute/gain/polarity/automation/correction + project master gain & automation & `corrections_disabled`. Anything that changes the mix bytes invalidates the cache. UI-only state (selected tab, zoom) is excluded.
- **Schema migration.** `SCHEMA_VERSION` bumps from 1 to 2. On open, v1 files are migrated in-place with `CREATE TABLE IF NOT EXISTS mix_run …` + `UPDATE meta SET schema_version = 2`. Idempotent.
- **Bounce trigger.** Explicit Mix-tab toolbar button labelled `⤓ Bounce`. State pip:
  - no cache → `⤓ Bounce`
  - cache present + fresh → `⤓ Bounce  ✓`
  - cache present + stale → `⤓ Bounce  ⚠ stale`
- **Folder-backed projects.** Bounce is disabled with a tooltip telling the user to Save → As `.tib` first. The cache lives inside `.tib`; folder projects don't get it.

### Phase B — Crossfade tab reads `.tib`

The Load… dialog's filter widens to `WAV or TinyBooth stem (.tib)` (with separate WAV-only and .tib-only filters as alternatives). When the user picks a `.tib`, the tab opens the database, reads the `mix_run` blob, and feeds it through the same `decode_wav_reader_as_stereo` path the .wav loader uses — so the rest of the tab (zoom, click-seek, fade handles, transport, export) needs zero changes.

If the picked `.tib` has no `mix_run` row, the tab refuses with a clear status: `"<file> has no bounced mix yet — open the project in TinyBooth and click Bounce first"`.

## Implementation notes

- `TibDb` gets four new methods: `read_mix_run_header`, `read_mix_run_audio`, `write_mix_run` (upsert keyed on `id = 1`), `delete_mix_run`. Header reads are cheap (metadata only, no BLOB I/O); the audio is pulled via incremental BLOB I/O so rusqlite doesn't materialise a second copy.
- `export.rs` exposes three new functions: `render_master_mix` (in-memory `(Vec<f32>, sr, ch)`), `render_master_mix_to_wav_bytes` (the same plus an in-memory WAV encode), and `compute_mixrun_signature`. The existing `mixdown` stays private — the new entry points just stop at different points in the existing pipeline.
- `TinyBoothApp::bounce_master_mix_to_tib` is the orchestration glue: gate on `.tib` backing, compute the signature, render, encode, write. `mix_run_status() -> (present, fresh)` powers the Mix-tab pip.
- The Mix tab adds the button in `transport_bar`, after the bulk-correction strip, separated by `ui.separator()`.
- The Crossfade tab adds `load_tib_mix_run_as_stereo` next to `load_wav_as_stereo` and dispatches in `handle_load` by file extension. The two loaders share a `decode_wav_reader_as_stereo<R: Read>` helper.

## Risks

- **Stale cache served as fresh.** The signature must include everything that affects the mix bytes. The implementation lists each relevant field explicitly via a serde-derived `MixSig` struct — a future field added to `Track` or `Project` that affects rendering will need to be added to `compute_mixrun_signature` too. Documented in the function's doc comment.
- **Size growth.** A 5-minute stereo 16-bit mix at 48 kHz is ~28 MiB per `.tib`. Acceptable for the project-as-stem use case; users who don't want the cache simply don't click Bounce.
- **Schema-version forward-compatibility.** A v2 .tib loaded by a v1-aware build of TinyBooth would fail at the existing "newer than this app supports" check — that's the right behaviour, not a regression.

## Open questions

None at landing. Deferred:

- **Auto-bounce on Export.** Optional: every WAV/FLAC/etc. export also stamps the .tib's mix_run with the same render. Currently explicit-only.
- **Multiple mix_run slots.** If multiple "approved" mixdowns per .tib become useful (e.g. one for sequencing, one for a "loudness-normalised release master"), the `id = 1` constraint becomes `id INTEGER PRIMARY KEY` with a `slot` text column. Not needed for v1.

## Success criteria

- New `mix_run` round-trip test in `tib::tests` passes — write, read header, read audio, upsert overwrites cleanly, delete drops the row.
- Full test suite remains green (127/127 with the new test).
- Manual: bounce a .tib project, load it into the Crossfade tab as Track A, load a second bounced .tib as Track B, hear them crossfade.
