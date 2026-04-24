# TBSS-FR-0001 — Suno cleanup mode

| Field | Value |
|---|---|
| **Request ID** | TBSS-FR-0001 |
| **Title** | Suno cleanup mode |
| **Status** | Proposed |
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

Ordered cheapest-first. Each phase is independently shippable after MVP.

| Phase | Feature | Lift | Crates / infra |
|---|---|---|---|
| A | LUFS metering + target normalization | ≈1 day, ~150 LOC | `ebur128` (MIT) |
| B | Match-EQ to user-supplied reference | 3–5 days, ~300 LOC | `rustfft` (in tree) |
| C | Pure-DSP de-reverb | ~1 week, ~400 LOC | `rustfft` |
| D | Stem separation | 2–3 weeks, ~800 LOC | `ort` (ONNX Runtime, Apache-2.0) + Demucs htdemucs_ft ONNX export, ~80 MB model, first-run download to `%APPDATA%\TinyBooth Sound Studio\models\` |
| E | High-frequency regeneration | 3–4 weeks | Same ONNX infra as D, second model; license pick between NVSR (MIT) / BigVGAN+ (MIT+) — AudioSR excluded due to non-commercial S-Lab license |
| F | Pure-DSP de-chirp / transient smoothing | 1–2 weeks + tuning, ~500 LOC | `rustfft` |

**Recommended sequencing:** MVP → A → B → C → D → E → F.

Rationale: A–C give visible user wins without architectural change. D introduces ONNX runtime once, then E borrows it cheaply. F is last because it's the hardest to tune without over-dulling transients.

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
