# TBSS-FR-0010: Crossfade tab — two-track preview + export

**Status**: 📝 Proposed
**Author(s)**: ophiocus
**Filed**: 2026-06-04

## Summary

A dedicated **Crossfade** tab alongside Record / Project / Mix /
Export. Load any two WAVs from disk, position track B's start
relative to track A on a shared timeline, visualize the overlap,
preview each track independently and the crossfade mix, and export
the result to any of the existing supported formats (WAV / FLAC /
MP3 / Ogg / Opus / M4A — same `export.rs` ffmpeg pipeline).

In the spirit of the existing Trim panel: a focused single-purpose
surface that doesn't need a project to be useful.

## Motivation

The Mix tab exists for whole-song multitrack work. The Trim panel
exists for cropping. But "blend the end of one track into the start
of another" is its own gesture and doesn't fit either workflow:

- You don't want to add the two tracks to a Mix project just to set
  one to fade out and the other to fade in — that's heavy ceremony.
- You don't want to commit the crossfade to a destructive edit; you
  want to audition different offsets / curves and export the result
  as a standalone WAV.
- Crossfade is a common deliverable on its own (DJ-style transitions,
  before/after demo cuts, comparison reels).

## Proposal

### Sources — any two WAVs from disk

Two "Load…" buttons (one per slot) open a file dialog. The picked
WAV is decoded once into f32 stereo samples and cached on the tab
state. **Constraint:** both tracks must share a sample rate (same
constraint the existing player enforces, for the same reason — no
resampler today). A mismatch surfaces a clear status-bar error.

Mono → stereo by duplication (L=R).

### Timing — slider for now, drag on waveform later

For MVP, a single `b_offset_secs` slider controls where track B
starts relative to track A's frame 0. Range is `[-B_duration,
A_duration]` so B can start before or after A's start. The overlap
region is computed: `overlap = max(0, min(A_end, B_end) - max(A_start,
B_start))`. The crossfade is applied across the full overlap.

A future revision could let the user **drag the track-B waveform
left/right** to set the offset — more direct, but slider gets us
shipping.

### Curve — equal-power default, linear later

For MVP: equal-power (sin²/cos² complementary curves). Inside the
overlap region of length `L` seconds, at offset `t ∈ [0, L]`:

- track A weight: `cos²(π · t / (2L))` — full at `t=0`, zero at `t=L`
- track B weight: `sin²(π · t / (2L))` — zero at `t=0`, full at `t=L`

These sum to 1 in power, so perceived loudness stays constant through
the transition — the right default for unrelated material. Linear
crossfade is the better choice for phase-coherent (e.g. two takes
of the same source) material and gets added later as a picker.

### Visuals

- Two stacked waveform strips (track A on top, B below) on a shared
  time axis. The shared axis spans `[min(0, B_offset), max(A_dur,
  B_offset + B_dur)]` seconds.
- The **overlap region** is highlighted with a translucent fill
  spanning both lanes.
- The fade curve (track A descending, track B ascending) is drawn
  faintly over the overlap.

### Transport

- **▶ A** plays track A start-to-end.
- **▶ B** plays track B start-to-end.
- **▶ Crossfade** plays the full mixed timeline (A's start through
  B's end with the crossfade applied at the overlap).
- **■ Stop** halts playback.

Playback runs on a dedicated cpal output stream owned by a small
`CrossfadePreviewSession` (mirrors the player's owner-thread pattern
in much-simplified form: one Vec<f32> buffer, an AtomicU64 position,
one cpal stream, dropped on stop). The session is recreated per
▶ press; there's no global play state.

### Export

The "Export…" button mixes the full timeline at the project's f32
buffer width, writes a temp WAV via hound (16-bit PCM, same writer
the rest of the app uses), and — if the user picked a non-WAV
format — pipes it through `find_ffmpeg()` in `export.rs`. The format
picker is the same `ExportFormat` enum the Export tab uses, so
WAV / FLAC / MP3 / Ogg / Opus / M4A all work for free.

The output filename defaults to
`<A-stem>_x_<B-stem>_xfade<duration>s.<ext>`.

## Implementation — module breakdown

- **`src/crossfade.rs`** — pure DSP. `CrossfadeSpec { a_samples,
  b_samples, sample_rate, b_offset_frames, fade_frames, curve:
  CrossfadeCurve }`. `compute_mix(&spec) -> Vec<f32>` (stereo,
  interleaved). Unit tests for: silent input → silent output; equal-
  power weights sum to 1; offset arithmetic produces the right total
  frame count.
- **`src/crossfade_player.rs`** — minimal cpal playback session.
  `CrossfadePreviewSession::play(samples, sample_rate, channels) ->
  Self`. Drop stops the stream. No UI state, no project coupling.
- **`src/ui/crossfade.rs`** — `show(app, ui)`. File-load buttons,
  waveform render (reuses the recordings-list `compute_peaks` shape
  in spirit, computed once on load), offset slider, transport, export.
- **`src/app.rs`** — `Tab::Crossfade` variant, `crossfade_state:
  CrossfadeUiState` field, tab dispatch in `update()`.

## Risks

- **Sample-rate mismatch between A and B.** Refuse to load, status
  message. Same posture as the existing player.
- **Long files = big in-memory buffers.** A 5-min stereo 48 kHz WAV
  is ~115 MB of f32. Two of them + the mix = ~350 MB. Fine for
  desktop; document.
- **Playback contention with the Mix-tab player.** Both want the
  default output device. Worst case: playback fights. Resolution:
  drop the Mix-tab player when the Crossfade tab is active (or just
  document — easier for MVP).

## Open questions

1. **Drag-the-waveform timing** instead of (in addition to) the
   slider — natural next iteration; left out of MVP to ship faster.
2. **Linear curve picker** — same reasoning.
3. **Two crossfades at once** (e.g. fade-in at start of A and
   fade-out at end of B, on top of the A↔B crossfade) — out of
   scope; that's a full timeline editor.
4. **Preview position scrubbing** — out of scope; transport is
   just start/stop.

## Success criteria

- Loading two same-rate WAVs displays both waveforms with the
  overlap highlighted.
- The offset slider repositions B and updates the overlap render
  + the fade curve immediately.
- ▶ A, ▶ B, and ▶ Crossfade each play the right audio through the
  default output; ■ Stop halts cleanly.
- Export… produces a WAV that byte-matches the in-app preview at
  the equal-power curve, plus any of the ffmpeg-encoded formats.
