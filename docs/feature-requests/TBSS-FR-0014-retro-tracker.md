# TBSS-FR-0014 — Retro tracker (MadTracker-referenced) with sample-configured instruments

| | |
|---|---|
| **ID** | TBSS-FR-0014 |
| **Title** | Tracker tab in the MadTracker 2 lineage: vertical pattern editor, sample-configured instruments, drum patterns |
| **Status** | 📝 Proposed (rev 2 — reshaped around MadTracker per user direction) |
| **Filed** | 2026-08-24 · rev 2 same day |
| **Requested by** | Carlos ("a new tab for a retro tracker/loop/sampler with instrument lanes that can be sample configured"; rev 2: "use references from madtracker") |
| **Reference** | **MadTracker 2** (Yannick Delwiche) — feature set per [madtracker.org/features](https://www.madtracker.org/features.php) and [KVR's product page](https://www.kvraudio.com/product/madtracker_by_yannick_delwiche) |
| **Depends on** | `audiodecode` (canonical decode), `crossfade_player` (session precedent), `dsp` biquads (instrument filter), FR-0009 (bake-as-stem), FR-0015 (exclusive-audio contract) |

## Executive summary

A **Tracker** tab modeled on MadTracker 2's editor conventions rather than
a generic step sequencer: a **vertical pattern editor** (time flows down,
tracks as columns) whose cells carry **note · instrument · volume ·
panning · effect command**, driven by FastTracker-style QWERTY piano
input; an **instrument list** where each instrument is a user-configured
sample (recording take, `.tib` stem, or file) with loop points, base
note, gain, and a **resonant filter** (MT2's signature per-instrument
touch — we already own the biquads); plus MT2's **drum patterns** idea as
a first-class alternate view — a horizontal step grid over the same data
for percussion lanes. Output flows back into TinyBooth: loop audition
under the app-wide one-audible-thing contract (FR-0015), **⤓ bake as a
project stem** (FR-0009 path), and WAV export.

## Problem

TinyBooth records, cleans, mixes, and composes existing audio but cannot
*originate* rhythmic/melodic material. Takes and stems are dead ends as
instruments. A tracker closes the loop — and the user has a specific
dialect in mind: MadTracker, not an abstract grid.

## Proposal

### What we take from MadTracker 2 (and what we consciously shrink)

| MadTracker 2 | TinyBooth Tracker v1 |
|---|---|
| Vertical pattern editor; note/instr/**volume col**/**panning col**/effect, 4-digit hex params | Same five columns. v1 implements a small effect subset (below); unknown commands are preserved, displayed, and ignored by playback |
| 64 tracks × 4 polyphony channels per track (NNA) | 8 tracks × 2 voices per track — enough for NNA "continue" so releases ring out; cut is the default |
| Instruments: sample + **resonant filter per instrument** + envelopes + NNA | Sample (any source, trimmed) + base note + gain + loop (off/forward/ping-pong) + resonant low-pass (cutoff/Q via `dsp` biquads) + NNA cut/continue. Envelopes deferred |
| **Drum patterns** as a dedicated feature | The same pattern data rendered as a horizontal step grid for lanes flagged "drum" — toggleable per track, one underlying model |
| Patterns + order list (song) | v1: patterns + a minimal order list (play one pattern looped, or the order chain) |
| ProTracker/FastTracker keyboard shortcuts | FT2-style QWERTY piano (two octave rows), octave +/-, edit step, insert/delete row, Del clears cell |
| Speed/BPM tempo model | Classic ticks-per-row (`speed`) + BPM; effects operate per tick |
| VST 2.3 effects/instruments, ReWire, track FX/EQ, automation envelopes | **Out of scope.** TinyBooth's correction chain applies later, on the baked stem — that's the house's separation of concerns |
| Synchronized / keep-on-disk samples | Out of scope (all samples decode to RAM via `audiodecode`, like the player) |

### v1 effect-command subset (per-tick, classic semantics)

`0xy` arpeggio · `1xx`/`2xx` pitch slide up/down · `4xy` vibrato ·
`9xx` sample offset · `Axy` volume slide · `Cxx` set volume ·
`Dxx` pattern break · `Fxx` set speed/BPM · `ECx` note cut ·
`EDx` note delay. Everything else parses, round-trips, and no-ops.

### Data model (E1 — pure, serde)

```rust
struct TrackerCell { note: Option<Note>, instr: Option<u8>, vol: Option<u8>,
                     pan: Option<u8>, fx: Option<(u8, u16)> } // 4-digit hex param, MT2-style
struct TrackerPattern { rows: u16 /* 1..=256, 64 default */, tracks: Vec<Vec<TrackerCell>> }
struct TrackerInstrument { name, source: SampleSource, trim: (u64, Option<u64>),
                           base_note: Note, gain_db: f32,
                           loop_mode: Off|Forward|PingPong, loop_pts: (u64, u64),
                           filter: Option<{ cutoff_hz: f32, q: f32 }>,
                           nna: Cut|Continue }
struct TrackerSong { bpm: f32, speed: u8 /* ticks/row */,
                     instruments: Vec<TrackerInstrument>,
                     patterns: Vec<TrackerPattern>, order: Vec<u8>,
                     drum_view_tracks: Vec<bool> }
```

Persisted with the project (manifest field / `.tib` `config_revs`).

### Engine (E2)

Tick-based renderer: `speed` ticks per row at `BPM` → tick length in
frames; notes trigger voices (variable-rate playback for pitch —
authentic aliasing, no resampler); per-tick effect processing; voice
pool per track honoring NNA. Pure function `render_song(song, from_row,
rows) -> Vec<f32>` → unit-testable against hand-computed frame counts.
Playback = pre-rendered buffer through the `CrossfadePreviewSession`
loop transport (seek/position already exist since v0.4.81), re-rendered
on edit and swapped at a loop boundary. Obeys `App::stop_all_playback`
exclusivity in both directions.

### UI (E3/E4)

- **Pattern editor**: monospace grid, row numbers hex (MT2 style),
  current-row highlight, track headers with mute/solo; FT2 keyboard
  entry; the panning/volume columns render compactly (`--`/value).
- **Drum view**: tracks flagged drum render as step-grid rows above the
  note editor (one model, two projections).
- **Instrument rail**: list + editor (sample picker via recordings /
  stems / file — reusing FR-0015's decode plumbing; trim, loop, base
  note, gain, filter, NNA); ▶ auditions the instrument.
- **Transport**: BPM, speed, pattern/order selector, loop ▶/■,
  **⤓ Bake as stem**, **Export WAV…**.

## Implementation notes

Epics: **E1** model + serde + render math → **E2** engine + loop
transport → **E3** pattern editor + keyboard entry → **E4** instrument
rail + drum view → **E5** bake/export + order list. E1/E2 fully
testable headless (same wave discipline as FR-0013). Background rules
apply: decode + render off the UI thread.

## Risks

- **Keyboard entry vs egui focus** — FT2-style entry means the grid owns
  most keys while focused; needs a real focus model (first tab to want
  one). Prototype early in E3.
- **Effect semantics** are folklore-precise; implement against the
  OpenMPT wiki's documented command behaviors and test each with
  frame-exact fixtures.
- Tick renderer complexity creep — the v1 command subset is the fence.

## Open questions

1. Order-list in v1 or ship pattern-loop first? (Lean: pattern-loop
   first, order list in the same FR's final epic.)
2. Panning column in v1 playback (stereo voice pan) or display-only
   until the mixer story matures? (Lean: implement — it's cheap.)
3. `.mt2`/`.xm`/`.mod` import? (Lean: defer; note-for-note import is a
   separate FR if wanted.)

## Success criteria

- A 2-pattern song (order A A B) with 4 instruments — two from
  recording takes — plays gapless, edits mid-loop swap cleanly, NNA
  continue audibly rings a release, `9xx`/`Cxx`/`Fxx`/`ECx` behave per
  reference, bake lands a stem the Mix tab plays, export duration is
  frame-exact.
- All engine math covered by pure tests; zero new decode paths; no
  UI-thread blocking.

Sources: [MadTracker — About/Features](https://www.madtracker.org/features.php) · [MadTracker on KVR Audio](https://www.kvraudio.com/product/madtracker_by_yannick_delwiche)
