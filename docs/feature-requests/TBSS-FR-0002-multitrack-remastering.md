# TBSS-FR-0002 — Multitrack remastering

| Field | Value |
|---|---|
| **Request ID** | TBSS-FR-0002 |
| **Title** | Multitrack remastering (Mix tab + player + per-track corrections) |
| **Status** | Proposed |
| **Date filed** | 2026-04-25 |
| **Author** | Claude (authoring assistant) on behalf of project owner |
| **Session serial** | `CLAUDE-2026-04-25-MULTITRACK-RFC-0002` |
| **Depends on** | TBSS-FR-0001 (stem ingestion — landed in v0.1.4) |
| **Breaking changes** | None to users; `Profile` and `Track` schemas extended backward-compatibly |

---

## 1. Executive summary

Make TinyBooth a remastering tool for imported Suno bundles, not just a recorder. Add a **Mix tab** with multitrack waveform lanes, transport controls (play / pause / stop), a synchronized playhead, per-track correction chains, and an A/B bypass per track for original-vs-corrected comparison. Per-track correction settings are persisted in the `.tinybooth` manifest so a project survives close/reopen with its remastering state intact.

The DSP work piggy-backs on the existing `FilterChain` / `FilterChainStereo` — they get parametric EQ + de-esser additions (already designed in TBSS-FR-0001) and otherwise stay untouched. The recording hot path is not affected.

## 2. Problem

After v0.1.4, importing a Suno stem bundle creates a TinyBooth project with one Track per stem. From there, the user has no path to:

- See those stems as waveforms.
- Play them back as a mix.
- Apply per-stem cleanup (each stem has different artefact profiles — vocal sibilance, drum mud, etc.).
- A/B compare original vs. cleaned within the app.
- Export the result as a mastered stereo file.

The Export tab does already mix-and-render, but it processes nothing per-track (no correction chain at mixdown), and there's no playback to validate before commit. Today the user's only remastering option is to export raw, take the WAV into a third-party DAW, and round-trip — which defeats the whole "TinyBooth understands Suno stems" pitch.

## 3. Proposal

Add a **Mix tab**, fourth top-level tab alongside Record / Project / Export. Tab is enabled when the active project has at least one track.

The tab presents:

- **Top transport bar** — Play (▶) / Pause (⏸) / Stop (⏹) buttons, time display (`MM:SS / MM:SS`), output device picker (defaults to system default).
- **Multitrack lane list** — one horizontal lane per Track:
  - Lane label (track name + role badge for Suno stems).
  - Mini-controls strip: Mute toggle, A/B bypass toggle, Gain slider, "Correction…" button opening the editor.
  - Waveform display — pre-computed peak table, drawn against the project's longest track's duration.
- **Synchronized playhead** — a vertical line crossing every lane at the current playback position. Sample-accurate atomic, read by the UI each frame.
- **Correction editor** — modal/sidebar revealed by the per-track "Correction…" button. Edits the track's `correction: Option<Profile>` chain.

## 4. Architecture

### 4.1 New module: `src/player.rs`

Owns a cpal output stream that mixes tracks live.

```text
┌─────────── UI thread ───────────┐    ┌───────── audio thread ──────────┐
│                                 │    │                                 │
│  Mix tab renders waveform       │    │  cpal output callback @ ~256    │
│  lanes + playhead + transport.  │    │  frames:                        │
│                                 │    │   1. read play_state            │
│  User clicks Play →             │    │   2. for each track:            │
│   set play_state = Playing      │───→│       extract sample window     │
│                                 │    │       from pre-loaded buffer    │
│  User clicks Pause →            │    │       at play_pos               │
│   set play_state = Paused       │    │       run track.correction      │
│                                 │    │       (unless bypassed)         │
│  Read play_pos atomic           │←───│   3. sum to stereo bus          │
│  for playhead rendering         │    │   4. soft-limit                 │
│                                 │    │   5. write cpal buffer          │
└─────────────────────────────────┘    │   6. play_pos += frame_count    │
                                       └─────────────────────────────────┘
```

**Buffer strategy:** each track's WAV pre-loaded into a `Vec<i16>` (interleaved if stereo) at project load. f32 conversion happens per-buffer in the audio callback. For typical Suno output (3 minutes, 12 stems, 48 kHz, 2 ch, 16-bit) this is ~140 MB resident — acceptable on any modern machine, well below the cost of streaming from disk in the audio callback.

**Sync primitives:**
- `play_state: Arc<AtomicU8>` — `0=Stopped`, `1=Playing`, `2=Paused`. UI sets, audio reads.
- `play_pos_frames: Arc<AtomicU64>` — audio writes, UI reads each frame for playhead position.
- `tracks_state: Arc<Mutex<Vec<TrackPlayState>>>` — UI mutates (mute, gain, bypass, correction profile), audio takes a brief lock at the top of each callback. Locks are short (microseconds); for ≤16 tracks this is fine. If profiling later shows contention, swap to per-track atomics + a "rebuild chain" generation counter.

### 4.2 DSP additions (Phase 1, this RFC's first shipped slice)

Extend `Profile` (in `src/dsp.rs`):

```rust
pub struct Profile {
    // ... existing fields preserved ...

    pub eq_bands: [EqBand; 4],
    pub deess_enabled: bool,
    pub deess_hz: f32,
    pub deess_threshold_db: f32,
    pub deess_ratio: f32,
}

pub struct EqBand {
    pub kind: EqBandKind,   // Bypass / Peak / LowShelf / HighShelf
    pub hz: f32,
    pub gain_db: f32,
    pub q: f32,
}
```

All new fields marked `#[serde(default)]` — every existing `profiles.json` and every existing `.tinybooth` manifest still loads cleanly, with defaults that disable the new blocks.

Both `FilterChain` (mono) and `FilterChainStereo` gain `apply_eq()` and `apply_deess()` methods, slotted into the per-sample chain after the high-pass filter and before the existing compressor:

```text
input → input_gain → HPF → EQ (4 bands) → de-esser → gate → compressor → makeup → output
```

EQ is implemented as four `biquad::DirectForm2Transposed<f32>` per channel; types `Type::PeakingEQ` / `Type::HighShelf` / `Type::LowShelf` per band.

De-esser is one band-pass biquad (centred on `deess_hz`, Q ~2.0) feeding an envelope follower that side-chains a downward-only gain on the dry signal when the band-pass envelope exceeds threshold.

### 4.3 Track-level correction (Phase 2)

Track schema gains a new optional field:

```rust
pub struct Track {
    // ... existing fields ...

    /// Correction chain applied at playback and at export mixdown.
    /// `None` = pass-through (track is mixed unprocessed).
    /// `Some(Profile)` = chain runs in this order: input gain, HPF,
    /// EQ, de-esser, gate, compressor, makeup.
    #[serde(default)]
    pub correction: Option<Profile>,
}
```

Existing manifests load with `correction: None` — no behavioural change. Imported Suno stems start with `correction: None`; user assigns a preset (Suno-Clean) or custom chain via the Mix tab's Correction editor.

### 4.4 Built-in `Suno-Clean` preset

Added to `builtin_profiles()` in `dsp.rs`. Exact parameter values from TBSS-FR-0001 §5:

- HPF 30 Hz
- EQ band 1: −3 dB Peak @ 300 Hz Q=1.0 (mud cut)
- EQ band 2: +2 dB HighShelf @ 10 kHz Q=0.7 (air lift)
- EQ band 3: −2 dB Peak @ 13 kHz Q=2.0 (shimmer tame)
- EQ band 4: bypass
- De-esser @ 6.5 kHz, threshold −18 dB, ratio 3:1
- Compressor: threshold −12 dB, ratio 2.0, attack 30 ms, release 200 ms, makeup +1.5 dB
- No gate

These values are consensus-derived (per FR-0001 §5 / §8 risk table) and **not empirically calibrated**. A Phase-0 listening study before locking defaults is still desirable; in the meantime the preset is offered as a starting point users can clone and tweak.

## 5. Phasing

Independently shippable; each is a separate version tag.

| Phase | Tag | What ships | Lift |
|---|---|---|---|
| **1 — DSP substrate** | v0.1.6 | Profile schema extensions, EQ + de-esser in FilterChain(Stereo), Suno-Clean preset, Admin window editors for the new fields. **No player. No Mix tab.** Existing recording-tone profiles can already use EQ + de-esser at recording time; export still works unchanged. | ~1 day |
| **2 — Player + Mix tab** | v0.2.0 | `Track.correction` field, `src/player.rs` (cpal output, pre-load, transport state, atomic playhead), peak-table precomputation, new `Tab::Mix` with lanes + transport + per-track A/B + Correction editor, mixdown honours `track.correction` at export. | ~5 days |
| **3 — Polish** | v0.2.x | Click-to-seek on lanes, loop region selection, Solo button, optional master limiter on the bus, per-stem correction-preset library (save / load named correction chains). | ~3 days |

## 6. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Audio-thread Mutex on `tracks_state` causes glitches under contention | For ≤16 tracks the lock window is tens of microseconds; if profiling shows xruns, swap to per-track atomics with a rebuild generation counter. |
| Pre-loaded buffers blow memory for very long projects | 12 stems × 5 min × 48 kHz × 2 ch × 2 bytes = ~230 MB worst case for raw Suno output — acceptable. Document the limit; if a user complains, add streamed playback via a producer thread + ring buffer in Phase 3. |
| Suno-Clean defaults sound bad | Phase-0 calibration on real Suno tracks before v0.2.0 lock. Mark as "starting point — clone and tweak" in the Mix tab description. |
| Profile schema migration breaks `profiles.json` | `#[serde(default)]` on every new field. Verified at Phase-1 release time by loading a pre-Phase-1 `profiles.json`. |
| User confusion: "recording tone" vs "correction chain" — both are `Profile` | Same struct, two different uses (recording-time freeze vs. playback-time live edit). The Admin window stays focused on recording-tone profiles. The Correction editor (Mix tab) operates on `track.correction` directly. Manual chapter explains the distinction. |

## 7. Open questions

1. Should the Mix tab's playback respect Mute (skip the track entirely) or play it through with the mute-icon visible? (Default: skip — matches existing Project tab semantics.)
2. Should A/B bypass apply at export too, or only at playback? (Default: only playback. Export always honours the persisted `correction`. Bypass is for listening, not for committing.)
3. Output device — system default only, or expose a picker like the input picker on Record? (Default: picker, parallel to input.)
4. When a track's correction is changed during playback, rebuild the chain mid-stream or wait until the next play cycle? (Default: rebuild mid-stream — the audio thread polls a generation counter.)

## 8. Success criteria

- Open a Suno-imported project → click Play → hear all stems mixed in stereo.
- Toggle a track's A/B button → audibly switch between original and corrected on the fly.
- Adjust the Suno-Clean preset's de-ess threshold on the Vocals stem → hear the change next play cycle without re-export.
- Export → resulting WAV is bit-equal (within rounding) to a manual stem-by-stem render through the same chain.
- Existing recording flow (Record tab → take → Project tab → Export) is identical to v0.1.5 — zero regressions.

## 9. References

- TBSS-FR-0001 (Suno cleanup mode) — §4 specifies the EQ + de-esser blocks reused here; §5 specifies the Suno-Clean preset values.
- `src/dsp.rs` — current `FilterChain` / `FilterChainStereo` implementations.
- `src/audio.rs` — current cpal input model; output model in Phase 2 mirrors its structure.
- Suno-Trimmer (`I:/SoundTrimmer/`) — reference for cpal output + peak-table waveform rendering patterns.

---

*Session serial `CLAUDE-2026-04-25-MULTITRACK-RFC-0002`. Quote when requesting revisions that should build on, rather than diverge from, the design above.*
