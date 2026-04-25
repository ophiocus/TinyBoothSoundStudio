# TBSS-FR-0001 — Suno cleanup mode

| Field | Value |
|---|---|
| **Request ID** | TBSS-FR-0001 |
| **Title** | Suno cleanup mode |
| **Status** | Proposed (revised 2026-04-24, this session) |
| **Date filed** | 2026-04-24 |
| **Author** | Claude (authoring assistant) on behalf of project owner |
| **Session serial** | `CLAUDE-2026-04-24-SUNO-CLEAN-RFC-0001` |
| **Session scope** | Cross-project research + design, sourced from `I:/Suno_prompting/docs/postproduction-analysis.md` and `I:/Suno_prompting/docs/official-suno-guidance.md` |
| **Supersedes** | `suno-cleanup.md` (draft in same folder) |
| **Depends on** | None (additive feature) |
| **Breaking changes** | None to users; `Profile` schema extended backward-compatibly |

---

## 1. Executive summary

Add a second top-level mode — **Clean** — to TinyBooth Sound Studio, alongside Record / Project / Export. Clean mode loads a stereo WAV (typically a Suno export), applies a new Suno-tuned DSP chain, and writes a cleaned stereo WAV.

The feature reuses tinybooth's existing abstractions:

- `Profile` — extended with EQ and de-esser fields (backward-compat via `#[serde(default)]`).
- `FilterChain` — parallel `FilterChainStereo` for offline stereo processing; mono recording chain untouched.
- `hound` I/O, `biquad` filters, `rustfft` spectral work — all already in `Cargo.toml`.

Ships a built-in `Suno-Clean` profile that targets the six documented Suno artifacts (shimmer, metallic vocals, hiss, 200–500 Hz mud, weak transients, dull highs) with consensus-derived defaults.

The MVP is pure-DSP and self-contained. A phased lift plan (§7) lays out how to add ML-grade restoration (stem separation, HF regen, de-reverb, de-chirp, match-EQ) incrementally, without disturbing the recording workflow.

## 2. Problem

Suno exports arrive with six characteristic artifacts:

1. AI shimmer (10–16 kHz residue of diffusion-from-noise)
2. Metallic / hollow vocals
3. Background hiss floor
4. Muddy low-mids (200–500 Hz)
5. Weak transients + boxed midrange
6. Dull highs

tinybooth's current recording-oriented profiles (Guitar, Vocals, Wind/Brass, Drums, Raw) address none of these — they're tuned for capturing a live mic, not remediating a generated file. Users who record acoustic takes over a Suno bed currently have no in-app path to clean the bed before or after the session.

## 3. Proposal

Add **Clean mode**:

- New top-level tab `Tab::Clean` (keep Record / Project / Export as-is).
- File picker → WAV in (reject MP3 with clear error).
- Profile dropdown (defaults to `Suno-Clean`).
- Progress bar during processing.
- Output-path picker → cleaned WAV out.

The processing runs offline (file → file) through `FilterChainStereo`. No real-time audio thread involvement.

## 4. DSP additions

Added to the existing chain (input gain → HPF → gate → compressor → makeup):

1. **Parametric EQ block** — 4 bands, each `Peak` / `LowShelf` / `HighShelf` / `Bypass`. Uses `biquad::Type::PeakingEQ` / `HighShelf` / `LowShelf`. Four biquads per channel.
2. **De-esser block** — sidechain-compressed band-pass at a user-configurable frequency (default 6.5 kHz). One band-pass biquad + one envelope follower per channel.
3. **Stereo-linked envelopes** — gate and compressor detect from `max(|L|, |R|)`, apply identical gain to both channels. Preserves stereo image.

Mono `FilterChain` kept intact for the recording hot path. Stereo path is a parallel struct (`FilterChainStereo`) used only by Clean mode.

## 5. The `Suno-Clean` built-in profile

```rust
Profile {
    name: "Suno-Clean",
    description: "Post-process a Suno export: trim mud, tame shimmer, \
                  add air, gentle glue. Stereo only.",
    input_gain_db: 0.0,
    hpf_enabled: true,
    hpf_hz: 30.0,
    gate_enabled: false,
    eq_bands: [
        Band { hz: 300.0,    gain_db: -3.0, q: 1.0, kind: Peak       },
        Band { hz: 10_000.0, gain_db: +2.0, q: 0.7, kind: HighShelf  },
        Band { hz: 13_000.0, gain_db: -2.0, q: 2.0, kind: Peak       },
        Band::bypass(),
    ],
    deess_enabled: true,
    deess_hz: 6500.0,
    deess_threshold_db: -18.0,
    deess_ratio: 3.0,
    compressor_enabled: true,
    compressor_threshold_db: -12.0,
    compressor_ratio: 2.0,
    compressor_attack_ms: 30.0,
    compressor_release_ms: 200.0,
    compressor_makeup_db: 1.5,
}
```

These values are consensus-derived from six public guides (see `I:/Suno_prompting/docs/postproduction-analysis.md`). They are **not empirically calibrated**. Phase-0 requires an A/B listening study on ≥20 real Suno exports before locking defaults.

## 6. MVP scope

- [ ] `FilterChainStereo` struct in `dsp.rs` (parallel to `FilterChain`).
- [ ] Parametric EQ block (4 bands).
- [ ] De-esser block.
- [ ] Stereo-linked gate + compressor envelope detection.
- [ ] `offline.rs` — `hound::WavReader` → chunked process loop → `hound::WavWriter`, with `mpsc` progress channel.
- [ ] `ui::clean` + `Tab::Clean`.
- [ ] `Suno-Clean` profile added to `builtin_profiles()`.
- [ ] Phase-0 A/B calibration study on ≥20 Suno tracks; update defaults before release.
- [ ] Existing recording flow verified unchanged (regression guard).

## 7. Phased lift plan — advanced restoration

**Revised 2026-04-24 (this session):** original Phase D proposed local stem separation via Demucs/ONNX. That has been replaced — Suno separates stems server-side and exports them directly. Local separation would re-do work the user already paid for, at meaningful licensing and download-UX cost. The new D ingests Suno's bundle.

Ordered cheapest-first. Each phase is independently shippable after MVP.

| Phase | Feature | Lift | Crates / infra |
|---|---|---|---|
| A | LUFS metering + target normalization | ≈1 day, ~150 LOC | `ebur128` (MIT) |
| B | Match-EQ to user-supplied reference | 3–5 days, ~300 LOC | `rustfft` (in tree) |
| C | Pure-DSP de-reverb | ~1 week, ~400 LOC | `rustfft` |
| **D** | **Suno stem bundle ingestion** (replaces local separation) | **~3 days, ~400 LOC** | `zip` (single new dep) |
| E | High-frequency regeneration | 3–4 weeks | `ort` (ONNX), NVSR or BigVGAN+ (license pick) |
| F | Pure-DSP de-chirp / transient smoothing | 1–2 weeks + tuning, ~500 LOC | `rustfft` |

**Recommended sequencing:** MVP → A → B → C → D → E → F.

Rationale: A–C give visible user wins without architectural change. D used to introduce the ONNX runtime; with the ingestion redesign, D ships in days with no ML and no model downloads. E now bears the ONNX integration cost on its own (it's the only remaining ML feature); whether E ships at all becomes a separate decision based on whether HF regen on a Suno master is worth the lift. F unchanged.

### 7.D Suno stem bundle ingestion (revised)

**Problem:** Suno's web UI lets Pro/Premier users export per-track stems as a zip archive ("Download All") or per-stem WAV/MP3 files. Today TinyBooth has no path to consume these — users would have to manually create a project, drop each WAV in, name the tracks, and tag the roles by hand.

**Proposal:** add a `File → Import Suno stems…` action with two entry points (folder, zip). Detected stems become `Track` rows in a fresh `.tinybooth` project, each with a `TrackSource::SunoStem { role, original_filename }` so downstream tooling (the cleanup chain in MVP, future per-stem profile selection, etc.) can dispatch on stem identity rather than guessing from filename at every read.

**File-format expectations** — sourced from the project's research doc (`I:/TinyBoothSoundStudio/docs/research/suno-stems.md`, separately filed) and surfaced here as constraints:

- Filenames are **advisory**, lowercase, simple labels (`vocals.wav`, `drums.wav`, `bass.wav`, …). Schema not officially published. **Match by case-insensitive substring**, never exact equality. Handle numeric suffixes (`drums_1.wav`, `drums_2.wav`).
- Format may be WAV or MP3. **Read WAV headers for sample rate, bit depth, channel count** — don't trust filename or assume 44.1 kHz / 16-bit. (Subset shipped: WAV only for v1; MP3 deferred.)
- Tempo-Locked WAV variants exist and are time-stretched. They will NOT sum to the rendered master and **must be skipped** by the ingester — detect by filename hint (`tempo*locked` substring) and exclude.
- Stems are nominally time-aligned, but **community reports occasional few-sample offsets**. The ingester does not attempt to correct these; user can nudge per-track gain/offset in the existing Project tab.
- Stems do NOT sum bit-exactly to the master (mastering chain runs on the rendered master only). Don't surface "null-test" as a quality gate; it is a "did Suno's separator misbehave on this track" diagnostic at most.

**Stem-role enum (`StemRole`)** — covers the documented 12-stem set plus the legacy 2-stem mode:

```
Vocals, BackingVocals, Drums, Bass, ElectricGuitar, AcousticGuitar,
Keys, Synth, Pads, Strings, Brass, Percussion, FxOther,
Instrumental,  // for the legacy 2-stem export
Master,        // when the bundle includes the rendered master
Unknown        // catch-all for anything the matcher can't classify
```

**Filename → role mapping** (initial, conservative):

| Substring (case-insensitive) | StemRole |
|---|---|
| `vocal` AND `back` | BackingVocals |
| `vocal` (else) | Vocals |
| `drum` | Drums |
| `bass` | Bass |
| `electric` AND `guitar` | ElectricGuitar |
| `acoustic` AND `guitar` | AcousticGuitar |
| `key` or `piano` | Keys |
| `synth` or `lead` | Synth |
| `pad` or `chord` | Pads |
| `string` | Strings |
| `brass` or `wood` | Brass |
| `perc` | Percussion |
| `fx` or `other` | FxOther |
| `instrumental` | Instrumental |
| `master` or `mix` | Master |
| (anything else) | Unknown |

**Project-format extension:**

```rust
#[serde(tag = "kind")]
pub enum TrackSource {
    Recorded,                                                    // default
    SunoStem { role: StemRole, original_filename: String },
}

// Track gains:
#[serde(default)]
pub source: TrackSource,
```

`#[serde(default)]` on the new field keeps every existing `.tinybooth` manifest deserialising cleanly.

**Ingestion flow:**

1. User picks a folder OR a zip via File menu.
2. Ingester walks the source: zips are streamed via the `zip` crate; for each entry that ends in `.wav` and isn't a Tempo-Locked variant, the file is extracted into `<project>/tracks/`.
3. Each retained file is hound-opened to read sample rate / bit depth / channel count / duration.
4. Filename → `StemRole` matcher tags the file.
5. A new `Project` is created with one `Track` per detected stem; the project root is a sibling of the source (or, for zips, named after the zip). Manifest written to disk.
6. App opens the new project; user is now in the Project tab with everything ready.

**Out of scope for v1:**

- MP3 ingestion (defer until users ask — Suno's WAV path is the recommended one for any quality-sensitive workflow).
- Null-test against an included master (a debugging convenience, not core).
- Per-stem profile auto-assignment (lives in the Clean tab work; this PR just lands the data + import).
- Online stem fetch via unofficial APIs (research notes that those break whenever Suno rotates auth; not worth the maintenance).

**Lift recap:** ~3 days, ~400 LOC, one new crate (`zip`). All other Phase D scope (ONNX runtime, model downloads, license audit) is gone.

## 8. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Stereo refactor regresses mono recording | Parallel `FilterChainStereo`; mono hot path untouched. Add regression test covering `start_new_take` → `stop_take` round-trip. |
| Default values are guesses | Phase-0 A/B study on ≥20 Suno tracks with blind-preference protocol before locking defaults. |
| Compressor stacking pumps on already-loud Suno output | Conservative defaults (ratio 2:1, makeup 1.5 dB). Optional "bypass compressor if integrated LUFS > -10" check in Phase A. |
| Profile schema migration breaks old `profiles.json` | `#[serde(default)]` on all new fields. Covered by an explicit test. |
| Model download UX (Phase D) | First-run modal dialog with checksum-verified download, resumable, clear error if offline. |
| License contamination (Phase E) | Explicit license audit in Phase E; AudioSR pre-excluded; choose between NVSR and BigVGAN+ via quality A/B. |

## 9. Open questions

1. Should "Clean existing WAV" also accept project-internal tracks as input (apply the chain to a recorded take)? Would unify modes conceptually and cost little.
2. Does the project want LUFS metering in the Record tab as well (metering during capture), or is it Clean-only?
3. Shimmer-tamer bell at 13 kHz: static default, or an adaptive heuristic that scans the file for shimmer-band energy and sets depth dynamically?

## 10. Success criteria

- **Subjective:** blind A/B preference for cleaned vs. raw Suno export ≥70% across a 20-track panel.
- **Objective:** shimmer-band (10–16 kHz) RMS reduced by ≥3 dB without shifting spectral centroid by more than ±5%.
- **Performance:** offline processing ≤1× track duration on a mid-tier laptop CPU (pre-Phase-D; no ML in MVP).
- **Regression:** zero changes to mono recording timing, drift, or file-format output.

## 11. References

- Research synthesis: `I:/Suno_prompting/docs/postproduction-analysis.md`
- Official Suno guidance: `I:/Suno_prompting/docs/official-suno-guidance.md`
- Draft predecessor (retained for process record): `suno-cleanup.md` in this folder

---

*Session serial `CLAUDE-2026-04-24-SUNO-CLEAN-RFC-0001` identifies the authoring conversation. Quote this serial when requesting revisions that should build on, rather than diverge from, the research and reasoning behind this document.*
