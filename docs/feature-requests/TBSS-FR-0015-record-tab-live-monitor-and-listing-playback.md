# TBSS-FR-0015 — Record tab: in-listing take playback + live input monitor

| | |
|---|---|
| **ID** | TBSS-FR-0015 |
| **Title** | Recordings play in place (scrubbable, exclusive); ▶-to-mixer retired; live input monitor |
| **Status** | ✅ Implemented |
| **Filed / landed** | 2026-08-24 (same day, v0.4.81) |
| **Requested by** | Carlos (two live-use reports: "lets stop using mix for this" after the recordings ▶ kept failing; "allow all telemetry to fire up while not recording yet, so we can tell base levels") |

## Problem

**Auditioning a take went through the Mix tab**, hijacking the whole
mixer for a one-take listen. The detour failed repeatedly in the field:
an async-build race dropped the solo request (all takes played at once),
the same-length rule skipped takes, positional indices drifted. Each fix
exposed the next because the *shape* was wrong — a mixer is not a
preview button.

**Base levels were unknowable before recording.** The input stream only
opened once ⏺ was pressed, so the first look at levels/waveform/spectrum
cost a throwaway take.

## What shipped

1. **In-listing playback.** Each take row has a ▶/■ toggle playing
   through its own lightweight session (the Crossfade preview transport,
   now with `seek_frac`/`position_frac`). The waveform thumbnail becomes
   a transport while playing: an amber playhead tracks position and
   click/drag **scrubs**; when stopped, drag remains region-selection
   for export. Decode runs on a background thread.
2. **Exclusive audio.** Starting a take silences everything —
   Mix player, Crossfade preview, Album preview — via
   `App::stop_all_playback()`; the other transports symmetrically stop
   the listing preview when they start. One audible thing at a time,
   app-wide.
3. **⇪ Integrate into project.** The "work on this take" gesture: copies
   the take into the currently open project as a real track (folder:
   `tracks/<id>.wav`; `.tib`: stem + track row + `orig` revision — the
   migrate precedent), where corrections/telemetry/trim/mixing apply.
   Disabled (with hover explanation) while the recordings filespace
   itself is open; refuses cross-rate integration with a clear message.
4. **▶-to-mixer retired.** `play_recording_in_mixer`, the autoplay
   solo machinery, and v0.4.80's take-detail view are deleted — the
   listing now owns auditioning end-to-end.
5. **🎙 Live monitor.** A toggle beside ⏺ opens the capture stream with
   **no WAV writer** — same device config path, same recording-tone DSP
   chain, same viz feed — so waveform/spectrum/meters show base levels
   through the exact signal path a take would use. Starting a take
   drops the monitor first (device handoff). Verified on hardware by
   the env-gated probe: frames flow, nothing lands on disk.

## Notes

- `audio::start_recording` now takes `Option<&Path>`; `None` is monitor
  mode. One config-selection path (incl. the v0.4.78 format ranking)
  serves both — no duplicated device logic.
- Deferred: loose-WAV rows get ▶/⇪ parity (FR-0008's remaining item);
  level *numbers* (peak dB readout) alongside the meters while
  monitoring; input-clip indicator distinct from post-DSP peak.
