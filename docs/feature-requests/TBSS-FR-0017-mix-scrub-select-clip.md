# TBSS-FR-0017 — Mix window: scrubbing, region highlight, clip-through-the-chain

| | |
|---|---|
| **ID** | TBSS-FR-0017 |
| **Title** | Mix lanes: scrub the playhead; click-drag region highlight (per-track or all-tracks); render the highlight through the full chain to a new soundwave |
| **Status** | 📝 Proposed |
| **Filed** | 2026-08-24 |
| **Requested by** | Carlos ("enable scrubbing; enable clickdrag to highlight (on any track individually or across all tracks); Highlighted can be clipped to a separate soundwave, is rendered with all enabled filters and master values of the mixer") |
| **Depends on** | `player` (seek), `export::mixdown` (the chain renderer), FR-0015 (selection UX precedent + ⇪ integrate), FR-0016 (keys for selection nudging later) |

## Executive summary

Three connected Mix-tab gestures. **Scrub**: the lane playhead becomes
draggable — click/drag anywhere on a lane's timeline moves the transport
(today the Mix playhead is display-only). **Highlight**: click-drag on a
lane selects a time region — on that track alone, or across all tracks
via the shared timeline ruler — using the same drag-selection language
the recordings list already speaks (FR-0008/0015). **Clip**: one button
renders the highlighted range **through the exact playback chain** —
per-track corrections (only where enabled), fader gains, automation,
polarity, master fader/automation — into a separate soundwave: saved as
a WAV and offered for integration as a new track (the FR-0015 ⇪ path).

## Grounding in the current code

- **Seek already half-exists**: `PlayerState::seek_frames`
  (player.rs, `#[allow(dead_code)]` "Phase 3") stores the position
  atomic the audio callback re-reads every buffer — the same mechanism
  v0.4.81 shipped for recording-preview scrubbing. Wiring it to lane
  clicks is the missing 10%; this FR retires that Phase-3 reservation.
- **The chain renderer already exists**: `export::mixdown` renders
  corrections + automation + master exactly as playback does (it's the
  Export tab and Bounce path). It needs two parameters it doesn't have:
  a **frame range** and an optional **track filter** (one track vs all).
  No new DSP.
- **Selection UX exists**: the recordings thumbnail drag-select +
  scrub-vs-select disambiguation (playing = scrub, stopped = select)
  shipped in v0.4.81 — the lanes adopt the same rules, plus lane-drag =
  that track, ruler-drag = all tracks.

## Proposal

### E1 — scrub

Lane and ruler click/drag seek the transport via `seek_frames`
(un-deadening it). Works playing or stopped (stopped: playhead moves,
next ▶ starts there). LUFS note: the integrated meter's history is
positional, not gapless — seeking resets the integrated reading
(honest behavior; document in the tooltip).

### E2 — region highlight

`mix_selection: Option<MixSelection { scope: Track(usize) | AllTracks,
start_secs: f32, end_secs: f32 }>` on the app. Drag on a lane body →
`Track(project_idx)` (uses the FR-0016-era `project_idx`, not lane
position). Drag on the timeline ruler → `AllTracks`. Drawn as a
translucent band (per-lane or full-height). Esc clears (Global-scope
binding? No — selection isn't a modal; a small ✕ in the selection
toolbar plus starting a new drag replaces it). Scrub-vs-select
disambiguation mirrors the recordings list: plain drag = select; the
playhead handle itself = scrub.

### E3 — clip to a separate soundwave

A selection toolbar appears while a highlight exists:
`✂ Clip selection…` renders `[start, end)` through
`export::mixdown(project, range, scope)`:

- `AllTracks` → the master mix slice — all enabled corrections, all
  faders/automation, polarity, master fader + master automation. What
  you hear is what you clip.
- `Track(i)` → the same pipeline with every other track muted — i.e.
  the track's corrected, fader-shaped, automated contribution
  **including master values**, per the request's wording.
- Output: save-dialog WAV (extension fixup per house rule) + a
  follow-up offer to **⇪ integrate** the clip into the project as a new
  track (reuses `integrate_recording_into_project`'s storage arms).
- Runs on a background thread (audit rule; the Export tab's inline
  freeze is the counter-example, not the precedent).

## Epics & tests

**E1** seek wiring (+ unit test: seek clamps to `[0, longest]`) →
**E2** selection state + drawing (pure geometry tests) → **E3**
range/filter parameters on `mixdown` (fixture test: known sine stems,
range render equals full render's slice sample-for-sample; muted-other
test: single-track clip contains only that track's signal) → **E4**
toolbar + save + integrate offer.

## Open questions

1. Should a `Track(i)` clip bypass the **master** chain instead? The
   request says "with … master values of the mixer" — v1 follows that
   literally (master applied). Flag if solo-without-master is wanted
   later.
2. Snap: none in v1 (free-range). Beat-snap once FR-0013's beat grid is
   surfaced in the mixer?
3. Should the clip land in the recordings list too (as a loose WAV) for
   immediate ▶ audition? (Lean: yes — cheap, and FR-0015 already gives
   it playback + integrate affordances.)
