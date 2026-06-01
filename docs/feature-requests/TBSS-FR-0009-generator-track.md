# TBSS-FR-0009: Generator track — binaural / isochronic / layered focus music as a synthesized stem

**Status**: 📝 Proposed
**Author(s)**: ophiocus
**Filed**: 2026-05-29

## Summary

Add a new kind of track to the Mix tab — a **Generator track** — whose
audio is *synthesized* from parameters rather than recorded or imported.
It carries one of three generator modes (binaural beats, isochronic
tones, layered focus music), is **non-editable through Trim / hot-swap**
(its bytes are the deterministic output of the parameters), and is
**baked on demand**: the user clicks Bake, the audio is rendered at the
current project settings, the track becomes playable like any other
stem. Editing any parameter — or any setting the bake depends on —
marks the track *dirty*; the next bake clears that.

Every bake also writes a **timestamped export** to an
`exports/generator-bakes/` subfolder, so the user accumulates a versioned
library of generator renders rather than just the live one.

## Motivation / Problem

TinyBooth Sound Studio is a multitrack mixer for stem-based songs. The
user wants to layer **brain-entrainment audio** (binaural beats /
isochronic tones / focus-music pads) into their projects as a first-
class stem — to mix focus music with their stems, route it through the
project's master chain, export the combined mix, etc. — without:

- recording the synthesized audio in some other tool and importing
  the WAV (lossy round-trip, no parameter introspection, no
  regeneration if you want to tweak the carrier frequency by 2 Hz);
- juggling external generator apps every time you want to try a
  different beat rate against the project.

The audio itself is straightforward DSP. The interesting design is the
**data model + bake protocol** that integrates a procedural source
cleanly into a project format built around captured audio.

## Proposal

### Modes — modular, scope all three

A `GeneratorMode` enum carries the per-mode parameters. The bake
pipeline dispatches on it. MVP implements Binaural + Isochronic; the
Layered mode is the bigger DSP scope and lands in a follow-up.

```rust
pub enum GeneratorMode {
    /// Binaural beats — sine carrier with a slight L/R freq offset.
    /// Requires stereo output; needs headphones for the effect.
    Binaural {
        carrier_hz: f32,   // e.g. 200.0
        beat_hz: f32,      // e.g. 10.0 (alpha)
        amplitude: f32,    // 0..1, peak per channel
    },
    /// Isochronic tones — sine carrier modulated by a pulse envelope
    /// at the entrainment rate. Works over speakers.
    Isochronic {
        tone_hz: f32,      // e.g. 200.0
        pulse_hz: f32,     // e.g. 10.0
        duty_cycle: f32,   // 0..1
        amplitude: f32,
    },
    /// Layered focus music — background drone / ambient pad layered
    /// with an entrainment carrier. Future variant; design TBD,
    /// flagged here as the third architectural slot so the rest of
    /// the system (data model, dispatch, UI) doesn't have to be
    /// reworked when it lands.
    Layered { /* deferred */ },
}
```

The DSP for binaural is trivial: two independent sines at
`carrier_hz ± beat_hz/2` for L/R. Isochronic is a single sine
multiplied by a unipolar pulse envelope (square or smooth) at
`pulse_hz` with the given `duty_cycle`. Both are stable at any sample
rate; both write 16-bit PCM WAV via hound, same encode as recording.

### Data model — Generator as a TrackSource variant

The generator track lives in `Project.tracks` like any other stem, so
the Mix tab, exporter, telemetry, and `.tib` storage all see it
through the existing surfaces. New `TrackSource` variant:

```rust
pub enum TrackSource {
    SunoStem { … },
    Recorded { … },
    Imported { … },
    Generator {
        mode: GeneratorMode,
        last_bake_at: Option<DateTime<Utc>>,
        last_bake_master_signature: Option<MasterSignature>,
    },
}
```

`MasterSignature` is a small hash/struct of master settings the bake
depends on (see §"Dirty semantics"). Cheap to compute and compare.

A new `Track.locked: bool` is added (default `false`). When a track is
generator-backed, `locked = true` and the Trim, hot-swap, and any
future destructive ops short-circuit with a status-bar message
("Generator tracks are baked, not edited — change the params and
re-bake instead"). Already-existing per-track gain / correction /
mute work normally — they apply at playback, not at bake.

### Bake protocol — on demand only

A generator track is created in **dirty** state (no audio yet). The
user opens the Generator-params editor, dials in their mode + numbers,
clicks **Bake**. The bake does:

1. **Resolve duration.** `duration_secs = longest other track's
   duration_secs at this moment.` Re-resolved every bake — if you
   add stems later and re-bake, the generator track grows to match.
2. **Render the raw audio.** Dispatch on `GeneratorMode`. Output is
   stereo for Binaural (mandatory), stereo-duplicated for Isochronic
   (cheap), at the project's sample rate (or first track's rate, the
   project convention).
3. **Encode WAV** via hound (16-bit PCM, same writer as Trim's
   `crop_wav_bytes`).
4. **Store** as the track's audio — for `.tib` projects, commit as a
   new `revisions` row via `TibDb::commit_destructive_revision`
   (which gives free FIFO-5 history of past bakes — natural reuse of
   the FR-0007 phase 2c primitives); for folder projects, write the
   WAV at `tracks/<id>.wav`.
5. **Write a timestamped export** to
   `<project_root>/exports/generator-bakes/<track_id>-<ISO8601>.wav`
   — same WAV bytes, second copy on disk. Gives the user a versioned
   library of bakes outside the project's live track.
6. **Stamp** `last_bake_at = now()`, `last_bake_master_signature =
   current MasterSignature`, clear the in-memory dirty flag.

The bake can take a fraction of a second to a couple of seconds for
the longer modes; runs on the UI thread for MVP (existing pattern for
small in-memory WAV work) with a progress-bar follow-up if length is
ever a problem.

### Dirty semantics — what marks a generator stale

A generator track is dirty when its baked audio no longer matches what
a fresh bake would produce. Triggers:

- **Any generator parameter changes** (carrier_hz, beat_hz, mode
  switch, etc.) — obvious.
- **Project longest-track duration changes** (a stem was added,
  imported, or trimmed shorter / longer than the generator).
- **`MasterSignature` changes** — see the next subsection.

The Mix-tab lane for a dirty generator shows a distinct **dirty
indicator** (a small ✱ icon or amber tinge on the lane header).
Clicking it opens a **Bake confirmation modal**: "Generator track
'<name>' is out of date. Re-bake now? <Bake> <Keep stale>". Bake runs
the protocol above.

### "Meld with master chain" — the open interpretation

The user-stated requirement: *"Content of generator track is always
expected to meld with master chain on all settings as set at the
moment of bake."* Two plausible readings; the RFC proposes the
**first** and flags it for confirmation.

**Reading A (proposed):** The bake **does not pre-apply** the master
chain to the audio bytes (so playback / export still routes the
generator through the master chain like any other track, no
double-apply). But the bake **snapshots the master chain settings**
into `last_bake_master_signature`; if those settings change after the
bake, the track is marked dirty, and the timestamped export
filename records the master-settings signature so each saved
generator-bake file is a coherent point-in-time render against a
known master.

**Reading B:** The bake **does pre-apply** the master chain to the
audio bytes (so the baked WAV is the post-master signal). At
playback, generator tracks bypass the master chain (otherwise it
double-applies). This is unusual: it makes the generator track sit at
a different relative level than other tracks if you adjust the master
afterward, which is exactly what "marks dirty" handles — but it's
strange behaviour at playback for the period between dirty and
re-bake.

Reading A is cleaner and matches every other track's behaviour. The
"meld" guarantee is preserved by the dirty-on-master-change rule plus
the timestamped export carrying a master-signature stamp. **If you
meant Reading B, say so and the RFC + implementation flip.**

### `MasterSignature` shape

Just enough of `Project`'s master state to detect "would re-baking
produce different bytes." Cheap:

```rust
struct MasterSignature {
    master_gain_db_bits: u32,                  // f32 bits for exact compare
    master_automation_hash: u64,               // hash of Option<AutomationLane>
    corrections_disabled: bool,
    longest_other_duration_centisecs: u32,     // .duration_secs * 100, rounded
}
```

Stored as a tiny serde JSON on the track row (`.tib` schema's existing
JSON-text-column convention). Generation: walk the project state at
bake time, hash/derive, emit.

### UI

- **Add Generator Track** action in the Mix tab's "+" menu (or a new
  context menu on the tracks list). Opens a small modal: mode picker
  (Binaural / Isochronic / Layered-disabled), per-mode parameter
  fields with reasonable defaults (200 Hz carrier + 10 Hz alpha beat,
  amplitude 0.3), Bake button.
- **Edit Generator Track**: clicking the dirty indicator (or a small
  "✎ Params" button on the lane header) re-opens the same modal
  pre-filled with the track's current params.
- **Bake action**: produces a status-bar message
  `"Baked <track_name> → <track audio path>; exports/generator-bakes/<file>.wav"`.

## Risks

- **Locked-track surface.** Every existing destructive op needs the
  `Track.locked` check or it'll happily Trim / hot-swap the generator
  away. Currently: `trim::trim_project*`, `app::hot_load_swap*`,
  `ui::project.rs` delete-track. Audit before MVP ships.
- **`.tib` revision history under repeated bakes.** Each bake commits
  a new destructive revision (FIFO-5). After ~5 bakes the oldest
  drops. Acceptable — that's exactly the FIFO semantic — and the
  timestamped exports cover the case where the user wants longer
  history. Document.
- **The "meld with master" interpretation.** See above. The RFC is
  designed around Reading A. A misread here means a real
  architectural rework, so confirm before code lands.
- **Layered mode is real DSP work.** Voicing, scale/key sources,
  amplitude balancing against the entrainment carrier — not MVP.
  Scoped here so the data model + UI don't need rework later.

## Open questions

1. **Reading A vs B for "meld with master chain"** — see §"Meld with
   master chain — the open interpretation."
2. **Where does Add Generator Track live in the UI?** Mix-tab
   add-track menu (most natural — generator IS a stem), or Project
   tab? Lean Mix-tab.
3. **Cap on bake duration.** A 60-minute generator at 48 kHz stereo
   is ~660 MB of WAV. Reasonable for a focus session, big for a
   project. Soft warn over 30 min? Or trust the user?
4. **Layered focus music scope.** When this becomes real, what's the
   musical material — a drone tuned to the carrier? A randomized
   pad? Sampled bed loops? Out of scope for the MVP RFC; future
   amendment.
5. **Dirty-on-stem-added behaviour.** If the user imports new stems
   that are *shorter* than the generator, does that mark the
   generator dirty (because its longest-other-duration signature
   changed but the generator would still cover the new content)?
   Easiest default: yes, mark dirty; the cost is one bake.

## Success criteria

- "Add Generator Track" creates a new track in `Project.tracks` with
  `TrackSource::Generator` and `locked = true`. The new track shows
  up in the Mix lanes immediately, marked dirty.
- Clicking Bake renders the audio, stores it (BLOB or WAV per
  backing), writes a timestamped export, and clears dirty.
- After bake, the generator plays alongside other stems with normal
  per-track gain / correction / mute, and routes through the master
  chain at playback like any other track (Reading A).
- Changing any generator parameter, the master settings, or another
  track's length marks the generator dirty and surfaces the bake
  prompt on the next Mix-tab visit.
- Trim / hot-swap on a locked track no-ops with a clear status
  message; the underlying baked audio is never destructively edited.

## Landing order (suggested)

1. Data model — `GeneratorMode` enum, `TrackSource::Generator`
   variant, `Track.locked` field, serde compat for both `.tib` and
   folder. Tests for round-trip serialisation.
2. DSP — `bake_binaural(params, sr, secs) -> Vec<u8>` and
   `bake_isochronic(...)`. Pure functions, unit-tested.
3. Bake plumbing — `app::bake_generator(track_idx) -> Result<…>`
   that resolves duration, runs the DSP, stores audio per backing,
   writes the timestamped export, stamps the signature, clears
   dirty. Reuses `TibDb::commit_destructive_revision` /
   folder-WAV-write paths.
4. Locked-track guards — Trim / hot-swap / delete-track audit.
5. UI — Add Generator Track action + params modal + dirty
   indicator + Bake-confirm modal.
6. Layered mode — separate follow-up. Stub variant + UI greyed.
