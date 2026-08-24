# TBSS-FR-0014 — Retro tracker / loop sampler tab

| | |
|---|---|
| **ID** | TBSS-FR-0014 |
| **Title** | Retro tracker: pattern-based loop sampler with sample-configurable instrument lanes |
| **Status** | 📝 Proposed |
| **Filed** | 2026-08-24 |
| **Requested by** | Carlos (verbatim: "a new tab for a retro tracker/loop/sampler with instrument lanes that can be sample configured") |
| **Depends on** | `audiodecode` (canonical WAV/i16 decode), `crossfade_player` (one-shot preview transport precedent), FR-0009 (bake-as-stem precedent), FR-0013 E2 (tab-wiring + background-job idiom) |

## Executive summary

A new **Tracker** tab in the ProTracker/FastTracker lineage: a step grid
where rows are time steps and columns are **instrument lanes**, each lane
bound to a **user-configured sample** (a recording take, a `.tib` stem's
current revision, or any WAV/MP3 off disk). Patterns loop at a set BPM;
steps trigger their lane's sample, optionally pitched in semitones and
scaled in volume. The result can be auditioned live, looped, and **baked
into the project as a stem** (the FR-0009 Generator precedent) or exported
as a WAV — so a beat sketched in the tracker becomes first-class material
for the Mix, Crossfade, Album, and Chords features.

## Problem

TinyBooth can record, correct, mix, and compose *existing* audio, and can
synthesize entrainment tones — but it has no way to **make rhythmic
material**. The user's takes (Record tab) and stems are dead ends as
percussion/loop sources: there is no way to chop a take into an
instrument and sequence it. Every sibling feature would benefit from a
loop source: albums need interstitials, mixes need scratch beats, and the
recordings browser is already a natural sample bin.

## Proposal

### Data model (E1 — pure, serde, no UI)

```rust
struct TrackerInstrument {
    name: String,
    /// Where the sample comes from. Decoded once at load; cached.
    source: SampleSource,          // File(PathBuf) | TibStem { path, track_id } | RecordingTake { track_id }
    /// Trim window into the source (samples), so a take can be chopped.
    start: u64, len: Option<u64>,
    gain_db: f32,
    /// Base pitch reference: steps play at 2^(semitones/12) speed.
    root_semitone_offset: i8,
}

struct TrackerStep { on: bool, semitone: i8, velocity: u8 /* 0-127 */ }

struct TrackerPattern {
    steps_per_bar: u8,             // 16 default (4/4 sixteenths)
    bars: u8,                      // 1-4 → 16..64 steps
    lanes: Vec<Vec<TrackerStep>>,  // lanes[i].len() == step count
}

struct TrackerSong {
    bpm: f32, swing: f32,
    instruments: Vec<TrackerInstrument>,   // parallel to pattern.lanes
    pattern: TrackerPattern,               // v1: ONE pattern (loop); song-arrangement deferred
}
```

Persisted as a JSON column on the project (folder manifest field /
`.tib` `config_revs` entry) so a tracker sketch travels with its project.

### Playback (E2)

Pitch is done the authentic retro way: **variable-rate sample playback**
(nearest/linear interpolation, `rate = 2^(semi/12)`) — no resampler
dependency, and the aliasing *is* the aesthetic. Rendering a pattern is
pure math over decoded i16 buffers (via `audiodecode::decode_wav_i16`),
so the loop is **pre-rendered to a stereo f32 buffer whenever the
pattern/instruments change** (a 2-bar 16-step pattern renders in
milliseconds) and played through a dedicated looping transport modeled on
`CrossfadePreviewSession` — with the loop flag added, and reusing the
user's configured output device (fixing that session's known default-
device gap while we're in there). No per-step realtime scheduling in v1;
edit-during-playback re-renders and hot-swaps the buffer at the loop
boundary.

### UI (E3 — the Tracker tab)

- Standard tab wiring (`Tab::Tracker`, `TrackerUiState` per the
  FR-0013 recipe; state struct lives in `ui/tracker.rs` per the audit's
  TrimState convention).
- **Instrument rail** (left): lane list; each lane = name, sample picker
  (Recordings takes / project stems / file dialog), trim range, gain,
  root pitch. "▶" auditions the lane's sample once.
- **Step grid** (right): rows = lanes, columns = steps, bar-grouped
  shading; click toggles, drag paints; per-step semitone/velocity via
  scroll or a small popover. Playhead column highlights while looping.
- **Transport strip**: BPM drag, swing, bars/steps-per-bar, ▶ loop /
  ■ stop, and the two sinks: **⤓ Bake as stem** (FR-0009 path: lands as
  a locked `TrackSource::Tracker` track / `.tib` revision) and
  **Export WAV…**.

### Non-goals (v1)

Multiple patterns + order list (the "song" editor), per-step effects
(retrigger, slide, vibrato), MIDI in/out, sample recording directly into
an instrument (use the Record tab), time-stretching (pitch is speed,
period).

## Implementation notes

- Epics: **E1** model + pure pattern-render (tests: step placement
  sample-accurate at BPM; pitch = rate math; velocity scaling; swing
  offsets) → **E2** looping transport (reuse/extend `crossfade_player`;
  honor configured output device) → **E3** tab UI → **E4** bake/export
  sinks. E1/E2 are pure and testable before any UI exists — same wave
  discipline as FR-0013.
- Decode through `audiodecode` only (the audit's canonical path — no new
  decode ladders). `.tib` sources read the current revision BLOB via
  `Cursor`, same as the player.
- Background rules from the audit apply from day one: sample decode and
  bake run off the UI thread (`ChordJobMsg`-style mpsc poll; or the
  `JobHandle` helper if the cohesion tranche lands first).
- The step grid is the first UI with hold-and-paint interaction —
  pointer-capture semantics need the same care the crossfade fade
  handles got.

## Risks

- **Loop-boundary hot-swap** must be click-free: swap on the boundary
  sample with a 2–5 ms crossfade if needed. Prove with a unit test on
  the rendered buffers, not by ear.
- **Sample-rate mixing**: instruments at 44.1k vs a 48k project — v1
  renders at the project rate and variable-rate playback absorbs the
  ratio (a 44.1k sample at "0 semitones" plays at 44.1/48 rate). Must be
  explicit in the render math or everything is subtly flat.
- Scope creep toward a DAW: the non-goals list is the fence; v1 is one
  looping pattern done well.

## Open questions

1. Should ⤓ Bake write a *loop* long enough to fill the project's
   longest stem (FR-0009 anchors generator bakes that way), or exactly
   one pattern length? (Lean: fill-to-longest, matching Generator.)
2. Per-lane choke groups (closed hat chokes open hat) — v1 or defer?
   (Lean: defer; it's the first "per-step effect".)
3. Does the Recordings browser grow a "send to Tracker as instrument"
   affordance (FR-0008 adjacency), or does the Tracker's picker pull
   from recordings only? (Lean: picker-only in v1.)

## Success criteria

- A 2-bar, 4-lane beat built from two recording takes and two WAVs
  loops gapless at 90–180 BPM, survives app restart with the project,
  bakes into a stem that plays in the Mix tab, and exports a WAV whose
  duration is exactly `bars × steps × step_secs` at the project rate.
- All pattern-render math covered by pure tests; zero new WAV decode
  implementations; no UI-thread blocking on decode or bake.
