# TBSS-FR-0008: Record tab — recordings browser (full directory listing, waveforms, region export) + repeat-take fix

**Status**: 📝 Proposed
**Author(s)**: ophiocus
**Filed**: 2026-05-28

## Summary

Four changes to the Record tab's "Recent recordings" surface, filed
together because they share a code surface and ship best as one
coherent UX pass:

1. **List every WAV in the recordings filespace**, not just the ones
   tracked in the manifest.
2. **Path label affordance** — add a copy-to-clipboard button and an
   open-in-Explorer button next to the recordings-folder path.
3. **Bug fix** — after a take has been recorded and is listed, hitting
   Record again doesn't start a fresh take. Repro and fix.
4. **Per-take waveform + region selection + export selection** — every
   listed take grows a thumbnail waveform; click-drag picks a region;
   an "Export selection…" button crops that region to a new WAV.

Items (2) and (3) are small and should land first. (1) is a small
behaviour change. (4) is the meaty one and lands last.

## Motivation / Problem

The Record tab today (`src/ui/record.rs::show_recordings_list`) reads
the recordings project's manifest and lists `rec.tracks`. That's three
gaps in practice:

- **Loose WAVs are invisible.** Anything dropped manually into
  `%APPDATA%\TinyBooth Sound Studio\recordings\tracks\` or carried in
  from another machine without manifest reconciliation doesn't appear
  in the list. (The cleanse protocol migrates *Suno-project orphans*
  into the manifest, but that's the only auto-reconciliation path.)
- **The path label is dead text** — you can read where takes land but
  can't act on it. The natural next gestures (open the folder, copy
  the path into a file picker or terminal) require typing.
- **A reported bug** — once a take has been captured and is listed,
  pressing Record again fails to start a fresh take. The user has to
  do something else (restart? change tabs? change input?) to recover.
  This blocks the "record several takes in a row" workflow that the
  Record tab is for.
- **No per-take review.** Listed takes carry metadata (name, duration)
  but no visual. Picking the keeper take across a session of attempts
  means hitting ▶ on each, which round-trips through the Mix tab —
  high friction for what should be an at-a-glance browse. And once a
  good take is identified, exporting *just the good section* of it
  requires opening it in the Mix tab and running project-wide Trim,
  which destructively crops the take instead of producing a clean
  export.

## Proposal

### (2) Path label affordance — `S`

In `show()` (`src/ui/record.rs:220`), replace the `horizontal_wrapped`
with a row that carries:

- The "Each take saves to" label.
- The path in `monospace`.
- A 📋 button that copies the path to the clipboard via
  `egui::Context::output_mut(|o| o.copied_text = ...)`.
- A 📂 button that opens the folder in Explorer via
  `Command::new("explorer").arg(path).spawn()`. (Windows-only; this
  is a Windows desktop app, so no portability layer needed.)

Both buttons get short hover-tooltips. No new app state.

### (3) Bug — Record won't start again after a take is listed — `S–M`

**Repro to capture first**, then fix:

1. Launch the app, pick an input device, hit Record, speak, hit Stop.
2. The take appears in "Recent recordings".
3. Hit Record again. Observe what happens (button disabled? error in
   the status bar? silent no-op? non-fatal panic in logs/panic.log?).

Candidate root causes to check, in priority order, based on a read of
`app::start_new_take` and surrounding state:

- **cpal stream not fully released.** `stop_take` does
  `drop(sess)`, which drops the `RecordingSession` and its cpal
  `Stream`. If the underlying device handle outlives the drop on some
  drivers, the second `audio::start_recording` can't acquire it. Look
  for an error routed through `audio_err_tx` (drained into
  `app.status` each frame).
- **`required_sample_rate` mismatch on the 2nd take.** The first
  take's rate is baked into `rec.tracks.first().sample_rate`; the
  second take must match. If cpal's currently-active config drifted
  (e.g. the user changed the system sample-rate between takes), the
  second `start_recording` hard-fails.
- **A leftover `self.session` or `self.pending_take`.** Both should
  be `None` after `stop_take`, but a panic path through `stop_take`
  could leave one populated. If `app.session.is_some()`, the UI
  hides the Record button (line 142 — `if !recording`) and only
  shows Stop, which would manifest as "Record button gone, not
  broken."

**Fix shape**: identified by the repro. Plumb the actual failure into
the status bar (it should already be there via `audio_err_tx` — verify
the channel is being drained at the moment of the second-Record
click, not deferred a frame). If the root cause is a cpal handle held
across drops, add an explicit short delay or a `Stream::pause`
before drop. If it's a state-leak, fix the state machine.

This step lands its own commit ahead of the feature work — it
unblocks the basic record-multiple-takes workflow regardless of (1)
and (4).

### (1) List every WAV in `tracks/` — `S–M`

Today, `show_recordings_list` calls `Project::open_or_create_recordings()`
and iterates `rec.tracks`. Change to:

1. Read `rec.tracks` as today (manifested takes — full metadata).
2. `read_dir` on `rec.root.join("tracks")` and collect every `*.wav`
   not already covered by a manifest track's `file` field.
3. Render the unmanifested set after the manifested set, in a
   visually-distinct group (label: *"Loose WAVs (not in manifest)"*).
   Each loose entry gets a minimal row: filename, file size,
   modification time, the same ▶ / Export-selection / Delete actions
   that manifested takes get, plus an "Adopt into manifest" action
   that creates a `Track` row pointing at the WAV (using
   `Project::new_track_slot`-style id minting where possible, or the
   WAV's existing filename stem).

Edge cases:

- A loose WAV at a rate that mismatches the manifested rate is
  flagged with the same red "rate mismatch" treatment the cleanse
  protocol uses; Adopt is disabled with a hover-text explanation.
- Files with `.swap-tmp` / `.tmp` extensions (in-flight trim writes)
  are excluded.
- A directory listing failure surfaces as a colored label, not a
  panic (mirror `show_recordings_list`'s existing red-text pattern).

### (4) Per-take waveform + region selection + Export Selection — `M–L`

Each list entry (manifested *and* loose, after (1)) grows:

- **Waveform thumbnail.** Compute a peak table per WAV using the same
  `compute_peaks` shape as `src/player.rs`. Render with the existing
  `viz::draw_waveform` (or a thinner variant — the list shows N rows
  per page so vertical budget is tight; ~40 px works). Cache the
  peak vector keyed by `(abs_path, modified_at)` so we don't recompute
  on every frame. Build the cache lazily (first visit) on a worker
  thread or via the player's owner-thread idiom; UI thread never
  reads the WAV.
- **Region selection.** Click-drag on the thumbnail picks a region
  in seconds. Default is the full file. Persist the latest selection
  per take in the list's UI state so it survives pagination and tab
  switches (recordings page already does this for page index).
- **Export selection** button. Opens a `save_file` dialog seeded with
  `<take_name>-<start>-<end>.wav`. On confirm, crop the WAV to the
  selected range using `trim::crop_wav_bytes` (which already handles
  16/24-bit int + float, lossless) and write the bytes. The original
  take is untouched — pure export.

UI-state additions on `TinyBoothApp`:

- `recordings_peaks_cache: HashMap<PathBuf, (SystemTime, Vec<f32>)>`
- `recordings_selection: HashMap<PathBuf, (f32, f32)>` — start/end
  seconds per take.

Worker-thread plumbing: the existing telemetry/player owner-thread
patterns apply — the cache build doesn't need a long-lived thread;
spawn one decode per WAV on first visit, deliver via mpsc, drain in
`update()`.

## Risks

- **(3) is the highest-information item** — the actual root cause
  may be one we haven't predicted. Don't paper over symptoms (e.g.
  "force-reload selected_device on every Record click") without
  understanding *why* the second Record fails; that's exactly how
  battle-scars accrete.
- **(4) caches one decoded peak table per WAV.** A folder of 100
  takes × 3-min average × 48 kHz mono = ~100 × 8.6 MB raw decoded =
  860 MB if we held audio. We hold only `Vec<f32>` peaks at
  `PEAKS_BIN_SIZE = 256`: 100 × ~33 KB ≈ 3.3 MB. Comfortable.
- **(1) auto-adoption is out of scope.** Loose WAVs are *listed* but
  only adopted on explicit click — silent auto-import would surprise
  users who deliberately staged a WAV without wanting it in the
  manifest.
- **(4) export from a `.tib`-backed recordings filespace** is *not*
  in scope. Recordings are folder-format through phase 2c MVP
  ([TBSS-FR-0007]); when the recordings filespace migrates to
  `.tib`, the Export Selection bytes-source becomes
  `db.read_revision_audio(rev_id)` instead of `fs::read(path)` —
  small touchup, deferred until that migration lands.

## Open questions

1. **Loose-WAV pagination** — interleave with manifested takes in the
   newest-first ordering by file mtime, or keep them in a separate
   bottom section? Bottom section is simpler and less surprising;
   default to that.
2. **Selection persistence on the manifest** — should the per-take
   region selection be saved to disk (e.g. as a JSON field on the
   recordings manifest), or stay UI-state only? UI-state only for
   MVP — a selection is throwaway scaffolding for an export action.
3. **Auto-stop on Record fail** — when (3)'s underlying error is
   surfaced, should the failed attempt be a hard error in the status
   bar, or a toast that decays? Status bar with the existing
   pattern is fine.

## Success criteria

- The recordings list shows every `*.wav` in `tracks/`, manifested or
  not, with the loose ones clearly grouped.
- The recordings-path label has a copy button and an Open-in-Explorer
  button, both with hover-tooltips.
- After Stop on a take, Record starts a fresh take cleanly. If it
  can't (device error), the failure shows in the status bar.
- Each listed take shows a waveform thumbnail and supports
  click-drag region selection.
- "Export selection…" produces a WAV with the selected range,
  byte-identical to a hound `WavReader::open + crop + WavWriter`
  pipeline over the same input + range. The original take is
  untouched.

[TBSS-FR-0007]: TBSS-FR-0007-tib-container-revisions.md
