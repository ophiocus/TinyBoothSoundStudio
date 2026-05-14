# TBSS-FR-0006: The machine-learning boundary

**Status**: accepted (policy)
**Author(s)**: ophiocus
**Filed**: 2026-05-13

## Summary

A standing architectural policy, not a feature: **machine learning
is never permitted in the core analyzer or any deterministic path
of TinyBooth Sound Studio.** It is allowed *only* as a strictly
quarantined, opt-in, separately-downloaded sidecar, and *only*
where pure-DSP has a demonstrable ceiling that ML genuinely
crosses.

This RFC exists so the boundary is written down before anyone — a
future contributor, or a future version of the author — is tempted
to cross it casually because "the model would be more accurate
here."

## Motivation

TBSS-FR-0005 (track telemetry) was specced "no ML/LLM yet — pure
code." The "yet" was load-bearing: it left the door open. v0.4.13
through v0.4.35 walked through that door with pure-DSP
implementations of onset detection, YIN pitch tracking,
Krumhansl-Schmuckler key detection, multi-band drum classification,
and cross-band coherence (the AI-audio fingerprint diagnostic).

Every one of those could, in isolation, be "improved" with a
trained model. The question this RFC settles is not *can* they be —
it's *should the door stay open at all, and if so, how far*.

The answer: the door stays open a crack, fenced on all sides.

## The decision

### ML is forbidden in the core analyzer

The "core analyzer" is everything reachable from `analyze_wav`
today: spectral features, onset detection, sustain ratio, mood
proxies, drum classification, YIN pitch, K-S key, cross-band
coherence. None of it may be replaced or augmented by a learned
model. The reasons below are specific to *this* app, not generic
ML-skepticism.

**1. ML breaks the determinism the telemetry schema depends on.**
`analyze_wav` is a pure function — same WAV in, byte-identical
telemetry out. That is *why* `ANALYZER_VERSION` works as a contract:
a bump means "the algorithm changed, here is the diff," and stale
manifests can be detected and re-analyzed with confidence. A
learned model makes the version field unauditable — "we retrained,
outputs shifted, we cannot tell you exactly how." The entire
schema-versioning discipline rests on the analyzer being a pure
function.

**2. Every number is inspectable, and that is the brand.**
`compute_cross_band_coherence` is ~60 readable lines. A user — or
the author, two years on — can trace exactly why a stem scored
0.42. The README sells "the bedroom-studio opposite of a corporate
DAW"; the docs lean on pure-DSP transparency throughout. A
black-box "0.42 because the weights said so" contradicts the thing
the app is built to be.

**3. Using AI to de-AI-ify audio is close to self-defeating.**
The premise of the cross-band coherence work is that AI audio has a
*measurable, mechanistic* defect — decorrelated octave bands. The
planned phase-3 fix re-correlates them with a deterministic filter
whose behaviour can be fully explained. Replacing that with a model
trained to "make it sound natural" gives the de-AI tool the same
black-box, non-reproducible, hallucination-prone character as the
thing it is cleaning. Fighting a generative model with another
generative model erodes the entire premise.

**4. Binary and dependency cost is concrete.**
The MSI is ~67 MB, of which ~12 MB is the actual app and the rest
is the bundled LGPL ffmpeg. ONNX Runtime, `tch` (libtorch), or
`candle` with a real model adds 50–500 MB and a heavy native
dependency tree — much of it C++ that must cross-compile on the
Windows CI runner. `docs/rust-survival-guide.md` has a whole
chapter on dependency hygiene; this would be its single largest
violation.

**5. CPU-only, offline, no-GPU is a designed-around constraint.**
The analyzer runs on a background thread, ~1–3 s per stem, on
whatever hardware the user has. Audio-ML inference wants AVX-512 or
a GPU. A bedroom user on a five-year-old laptop would see the
analyzer go from 2 s to minutes, or be told to install CUDA. The
current cost is modest and predictable.

**6. It undoes the CI work and adds cross-compile fragility.**
Release builds are ~9 minutes after the v0.4.28 speedup pass. A
C++-linking ML dependency balloons that and introduces a class of
"works locally, breaks on the runner" failures.

### Reasons deliberately NOT relied on

For honesty, the following are *weak* arguments and must not be
cited as if they were strong ones:

- "ML is hard to debug" — true but generic; says nothing about
  *this* app.
- "Training-data licensing" — real but solvable (Basic Pitch is
  Apache-2.0; permissive audio datasets exist).
- "Models go stale" — DSP heuristics drift too.

The case against core-analyzer ML stands on reasons 1–6 alone. If
those ever stop being true, this RFC should be revisited.

### Where pure-DSP has a real ceiling

This RFC does not pretend DSP wins everywhere:

- **Polyphonic transcription.** YIN is monophonic-only. The guitar
  analyzer *detects* a strum and gives up — it cannot transcribe
  the constituent notes. Real chord/note extraction from
  polyphonic content is a learned-pattern task; DSP cannot cross
  that line.
- **Drum-class accuracy.** The multi-band heuristic is serviceable,
  but a small CNN trained on drum samples would be meaningfully
  better at kick-vs-tom and snare-vs-rimshot.
- **Genre / style classification.** Inherently a learned task; pure
  DSP essentially cannot do it.

These are the *only* places ML may ever be considered, and only
under the fence below.

## The fence — rules for any future ML sidecar

If ML is ever added, it must obey every one of these. A proposal
that violates any of them is rejected without further discussion.

1. **Separate download.** It never bloats the base MSI. The user
   who wants polyphonic transcription opts in; everyone else's
   install stays ~67 MB. The base app must remain fully functional
   — every feature shipped through v0.4.x — with the sidecar
   absent.

2. **Strictly additive.** It produces *new* fields (e.g.
   `polyphonic_notes`), never replaces a deterministic one. The
   coherence score, key estimate, onset detection, drum
   classification, and everything else in the core analyzer stay
   pure-DSP forever.

3. **Versioned as a dependency, not as the analyzer.** Its outputs
   live behind a clearly distinct `ml_model_version` field so they
   are never confused with — or compared against — the
   deterministic `analyzer_version` telemetry.

4. **Visibly ML-derived in the UI.** A distinct chip style (or
   explicit badge) so the user always knows which numbers are
   inspectable pure-DSP and which are inferred.

5. **No network at inference time.** If a model is bundled, it runs
   locally. TBSS makes no inference calls to a remote service. The
   only network traffic the app ever generates remains the
   self-update check.

6. **Offline-trainable provenance.** Any bundled model ships with a
   documented, reproducible training recipe and a permissively
   licensed dataset, committed to the repo's docs. No opaque
   weights of unknown origin.

## Non-goals

- This RFC does not schedule any ML work. It is a *boundary*, not a
  roadmap item. ML sidecar features, if they ever happen, get their
  own RFCs and must pass the fence above.
- This RFC does not forbid using ML *tooling* during development
  (e.g. an LLM to draft code, or an offline model to label a test
  corpus). It governs what ships *inside the binary* and what
  touches the *deterministic analysis path*.

## Decision record

**Accepted as standing policy, 2026-05-13.** The core analyzer is
pure-DSP forever. ML is fenced to an opt-in sidecar for the
transcription/classification frontier, and only there. Any PR or
RFC that wants ML in a deterministic path is the signal to push
back hard and point here.
