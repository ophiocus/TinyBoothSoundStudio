# Feature request: Suno cleanup mode

**Status:** draft · **Author:** (filed from Suno_prompting research, 2026-04-24)

## Problem

Suno-generated WAVs consistently ship with six characteristic artifacts the recording-room profiles don't address:

1. **AI shimmer** in the 10–16 kHz band (residue of diffusion-from-noise).
2. **Metallic / hollow vocals** — phasey, chorused leads.
3. **Background hiss** — constant `shhh` floor, obvious on headphones.
4. **Muddy low-mids (200–500 Hz)** — masked, congested bed.
5. **Weak transients + boxed midrange.**
6. **Dull highs** — bass-and-mid heavy, treble underpowered.

Research summary and source citations live in the sibling project at `I:/Suno_prompting/docs/postproduction-analysis.md`.

tinybooth's current DSP chain is **HPF → gate → compressor → makeup**, mono, realtime. That chain is built for capture, not restoration — it doesn't address any of artifacts 1–6 directly. Users currently have no path inside tinybooth to clean up a Suno export.

## Proposal

Add a **"Clean existing WAV"** mode that runs a new, Suno-tuned DSP chain over a loaded stereo WAV file and writes a cleaned stereo WAV. The mode reuses tinybooth's `Profile` abstraction and file-export plumbing. It does **not** require any ML, stem separation, or cloud services — it's pure DSP.

## Architectural fit

Three minimal extensions to the existing codebase:

| Area | Change | Size |
|---|---|---|
| `dsp.rs` | Extend `Profile` with EQ bands + de-esser + (optional) multiband comp fields. Extend `FilterChain` to process stereo. | Medium |
| New `offline.rs` | `hound::WavReader` → chunked process loop → `hound::WavWriter`. Progress channel to UI. | Small |
| `ui/clean.rs` + new `Tab::Clean` | File picker, profile selector, process button, progress bar, output path. | Small |

Nothing in the recording path changes. The Record / Project / Export tabs keep working as-is.

## New DSP blocks required

Adding to the existing HPF / gate / compressor, in chain order:

1. **Parametric EQ** (bell + shelf, 4 bands max). Needed for:
   - Bell cut -2 to -4 dB @ 300 Hz, Q≈1.0 (mud)
   - High shelf +1 to +3 dB @ 10 kHz (air)
   - Bell cut -1 to -3 dB @ 13 kHz, Q≈2.0 (shimmer tamer)
   - One spare user slot
   Implementation: four `biquad::DirectForm2Transposed<f32>` per channel, `Type::PeakingEQ` / `Type::HighShelf`. Same crate already in tree.

2. **De-esser** — sidechain-compressed band pass. Simplest form: split band-pass at 5–8 kHz → envelope follower → if over threshold, attenuate that band before summing back. One biquad pair + one envelope follower per channel.

3. **Stereo linking for gate + compressor.** Current `apply_gate` / `apply_compressor` operate on a single sample stream. For stereo, detect envelope as `max(|L|, |R|)` and apply the same gain to both channels. ~20 lines.

4. **(Optional, stretch)** Multiband compressor — split into 3 bands via Linkwitz-Riley biquads, run three instances of the existing compressor, sum. Doubles the DSP code but delivers the bus-glue stage the research calls out.

## Default "Suno-Clean" profile

Proposed built-in profile to add alongside Guitar / Vocals / Wind / Drums / Raw:

```rust
Profile {
    name: "Suno-Clean".into(),
    description: "Post-process a Suno export: trim mud, tame shimmer, \
                  add air, gentle glue. Stereo only.".into(),
    input_gain_db: 0.0,
    hpf_enabled: true,
    hpf_hz: 30.0,
    gate_enabled: false,
    // (new EQ fields)
    eq_bands: [
        Band { hz: 300.0,   gain_db: -3.0, q: 1.0, kind: Peak },
        Band { hz: 10_000.0, gain_db: +2.0, q: 0.7, kind: HighShelf },
        Band { hz: 13_000.0, gain_db: -2.0, q: 2.0, kind: Peak },
        Band::bypass(),
    ],
    // (new de-esser fields)
    deess_enabled: true,
    deess_hz: 6500.0,
    deess_threshold_db: -18.0,
    deess_ratio: 3.0,
    // existing compressor fields — gentle glue
    compressor_enabled: true,
    compressor_threshold_db: -12.0,
    compressor_ratio: 2.0,
    compressor_attack_ms: 30.0,
    compressor_release_ms: 200.0,
    compressor_makeup_db: 1.5,
}
```

**These numbers are consensus-derived, not empirically validated.** Calibrate against ~20 real Suno tracks before locking as defaults (see Open Questions).

## MVP scope

- [ ] Stereo support in `FilterChain` (currently mono).
- [ ] Parametric EQ block (4 bands) in `dsp.rs`.
- [ ] De-esser block in `dsp.rs`.
- [ ] Offline file-to-file processor (`offline.rs`).
- [ ] New `Tab::Clean` UI with: load button, profile dropdown, output-path picker, process button, progress bar.
- [ ] Built-in "Suno-Clean" profile.
- [ ] Preserve existing recording workflow untouched.

## Stretch scope

- [ ] Multiband compressor (3-band, Linkwitz-Riley splits).
- [ ] A/B preview (short-loop playback of before vs. after, no render).
- [ ] Batch mode: folder in → folder out.
- [ ] Loudness normalization to target LUFS (streaming -14, club -9). Requires an ebur128 impl (crate: `ebur128`).
- [ ] Genre sub-presets under Suno-Clean (Pop / Cinematic / Lo-fi / Acoustic).

## Lift analysis: advanced restoration features

The artifact list has items the MVP doesn't touch. None are off-limits — each has a real lift in hours, crates, and architectural surface area. Ordered cheapest-first so the project can land them incrementally.

### 1. LUFS metering / normalization — **Trivial (≈1 day)**

Add the `ebur128` crate (Rust port of libebur128, MIT). Meter the offline processing loop, optionally apply a single gain trim to hit a target integrated LUFS (-14 streaming / -9 club / user-specified). ~150 LOC. No architectural impact. Already listed under Stretch above — pulling it here for completeness.

### 2. Match-EQ to a reference track — **Low (≈3–5 days)**

Pure DSP, fits the existing crate set. Algorithm: STFT the reference (avg magnitude spectrum across the track), STFT the source, compute per-band difference, apply as a 31-band static EQ curve. `rustfft` already in tree. ~300 LOC. Needs one new UI element (reference-file picker). Worth doing — it's the cleanest way to borrow the tonal balance of a commercial track.

### 3. Pure-DSP de-reverb — **Low-medium (≈1 week)**

Not RX-grade, but useful for mild Suno tails. Approach: spectral gating with per-band noise-profile estimation in short silences + envelope-following tail suppression. `rustfft` again. ~400 LOC. Won't handle dense cathedral reverbs but works on the "metallic shimmer tail" characteristic of Suno. Ships as an optional block in the offline chain.

### 4. Pure-DSP de-chirp / transient smoothing — **Medium (≈1–2 weeks)**

Compression-artifact cleanup. STFT → detect outlier time-frequency bins (energy spikes inconsistent with neighbors) → interpolate from neighbors. Non-trivial to tune — too aggressive destroys percussive transients. Unchirp (proprietary) is the benchmark. A first pass is doable; expect iteration. ~500 LOC + tuning time.

### 5. Stem separation — **Medium-high (≈2–3 weeks)**

Needs ML inference, but doesn't require Python. Export Demucs htdemucs_ft to ONNX, use the `ort` crate (ONNX Runtime, Apache-2.0, already Windows/Linux/Mac on CPU and CUDA). Ship the model as a one-time download on first use (~80 MB) with progress + cache in `%APPDATA%\TinyBooth Sound Studio\models\`. ~800 LOC for inference wrapper + download manager + new "split stems" UI action. Runtime: 30–60s per 3-minute track on CPU, 3–5s on CUDA.

Once this lands, it unlocks the "per-stem processing" half of the research consensus — each stem gets its own chain run rather than bussing the whole mix.

### 6. High-frequency regeneration — **High (≈3–4 weeks + license resolution)**

Same ONNX plumbing as stem separation, different model. The catch is licensing:

- **AudioSR** — best results, S-Lab non-commercial license. Blocks any paid tinybooth distribution.
- **NVSR** — MIT, narrower bandwidth, acceptable quality.
- **BigVGAN+** — MIT, larger model, slower.

Pick one after a quality A/B. Once the ONNX runtime from #5 exists, this is mostly model-wrangling. Lift includes a second model download, quality-evaluation harness, and a "regenerate highs above X kHz" UI toggle.

### Dependency order

Items 1, 2, 3 are independent and can ship any time. Item 4 is independent but hardest to tune. Items 5 and 6 share infrastructure — build the ONNX runtime layer once (as part of #5) and #6 piggybacks for a fraction of the cost.

Suggested sequencing:

```
MVP → #1 (LUFS) → #2 (Match-EQ) → #3 (de-reverb) → #5 (stems + ONNX infra) → #6 (HF regen) → #4 (de-chirp)
```

That gives users meaningful wins every milestone, keeps the biggest architectural change (ONNX) for the middle of the roadmap when the app has proven out its cleanup workflow, and defers the trickiest-to-tune block (#4) until last.

### What stays external

- **MP3 encoding** — already handled by `export.rs` + ffmpeg shell-out. No need to bring libmp3lame in-tree.

## Risks

1. **Stereo refactor touches the hot path.** `FilterChain::process` currently takes one `f32` and returns one `f32`. Switching to `(f32, f32)` means changes in `audio.rs` recording loop as well — easy to break mono recording. Mitigation: introduce `FilterChainStereo` as a parallel struct, only used by offline mode; keep mono chain unchanged.

2. **Default values are guesses.** The research consensus gives direction, not calibration. Shipping bad defaults will make users think the feature is broken. Mitigation: phase-0 A/B study, 20+ Suno tracks, blind preference test before cutting a release.

3. **Compression on already-compressed source.** Suno output is often already loudness-limited. Stacking tinybooth's compressor on top can pump audibly. Mitigation: default makeup to 0 dB and ratio to 2:1; consider adding a "bypass when already loud" LUFS check.

4. **Profile schema migration.** Adding `eq_bands` + `deess_*` fields to `Profile` breaks existing `profiles.json` deserialization. Mitigation: use `#[serde(default)]` on all new fields so old JSON loads cleanly.

## Open questions

1. Does tinybooth want to own LUFS metering, or is that also a sibling-tool concern?
2. Should "Clean existing WAV" also accept existing tinybooth project tracks as input (apply the Suno-Clean profile to a take that's already been recorded)? Would unify the two modes conceptually.
3. Is there any appetite for a compiled-in shimmer-detection heuristic that adapts the 13 kHz cut dynamically, or do we keep it static?

## Success criteria

- Blind A/B preference: cleaned output preferred vs. raw Suno export on ≥70% of trials.
- Objective: shimmer-band (10–16 kHz) RMS reduced by ≥3 dB without shifting spectral centroid by more than ±5%.
- Runtime: ≤1× track duration on a mid-tier laptop CPU (no ML = trivially fast).
- Zero regressions in existing mono recording workflow.
