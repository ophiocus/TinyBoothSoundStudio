# TBSS-FR-0013 — Chord-chart video generator (auto guitar-diagram sync)

| Field | Value |
|---|---|
| Status | 🔧 In progress |
| GitHub epic | [#2](https://github.com/ophiocus/TinyBoothSoundStudio/issues/2) (sub-issues #3–#7 = E1–E5) |
| Depends on | existing STFT/chroma (telemetry, tbviz), ffmpeg (export), Generator tone synth (FR-0009) |

## Summary

A new **video-generator tab**: take a song (audio, or a video's audio track) and emit that same song with a synchronised video track showing the **guitar chord being played** — one standard six-string fretboard diagram per chord, changing on the beat, over the **original untouched audio**.

## Pipeline (data-contract driven)

```
audio/video in
  → E1  tempo + beat grid + full-mix chord recognition + verb detection  →  ChordGrid
  → E2  editable chord-grid panel (hear it, correct it) before render
  → E3  each chord → one EADGBE guitar voicing
  → E4  each voicing → one fretboard pictograph frame  (convention spec)
  → E5  build video track, sync to grid, mux over original audio  →  output video
```

## Settled scope (from intake)

- **Full-mix chord recognition, not per-instrument** — output only ever shows one chord at a time; per-stem tripled effort for no visible gain. v1 is full-mix.
- **Verb** = a span, at the song's tempo, holding one recognisable melodic phrase — the repeating harmonic unit the progression cycles through.
- **Pictograph** = standard six-string fretboard diagram, one guitar, **standard E tuning (EADGBE)**.
- **The editable grid (E2) is first-class, not hidden** — auto detection misfires; hearing + fixing before render is what makes the tool trustworthy.

## Development approach

**Data contracts and pure libraries first; UI and I/O last.** Each epic is a pure, unit-tested module before its UI/I-O consumer, matching the `.tib`/album pattern. Heavy reuse of existing substrate:

| Need | Reuse |
|---|---|
| STFT / chroma | telemetry `compute_stft`, tbviz live chromagram |
| tempo / beat | the FR-0013.5 lick-detector's BPM/beat estimator |
| MIDI-ish playback (E2) | Generator (FR-0009) tone synthesis through the cpal player |
| video encode + mux (E5) | the bundled/located `ffmpeg` (image2 → h264, `-c:a copy`) |
| frame raster (E4) | the `image` crate (already a dep) — render once, show as egui texture *and* feed ffmpeg |
| new tab | `Tab` enum + central-panel dispatch (as Album did) |

**Dependency order → build waves:** E1 ∥ E4 (pure, no cross-dep) → E3, E2 (need E1) → E5 (needs E3 + E4). Ship one release per completed wave.

## E1 — ChordGrid analyzer (LANDED)

`src/chordgrid.rs`, pure + unit-tested (data-only, dumpable; no UI). Pipeline:

1. **STFT** (4096 / hop 1024, Hann) → magnitude frames.
2. **Onset envelope** = half-wave-rectified spectral flux per frame.
3. **Tempo** = autocorrelation of the onset envelope over 60–180 BPM, with a mild slow-tempo bias against octave errors.
4. **Beat grid** = phase-lock: pick the phase offset maximising onset energy on beats, step by the beat period.
5. **Beat-synchronous chroma** = mean of the per-frame 12-bin chroma over each beat interval. Pitch class via MIDI convention `round(69 + 12·log₂(f/440)) mod 12` (C=0).
6. **Chord match** = cosine of the beat chroma against L2-normalised binary templates for 6 qualities × 12 roots (maj / min / dom7 / min7 / maj7 / dim); confidence = best cosine; below 0.5 ⇒ `None` (N.C.).
7. **Verb** = shortest 2/4/8-bar loop (4/4 assumed) whose self-match ratio clears 0.6; `core_progression` = one representative chord per bar of the first loop.

**Data contract** (serde-serialisable, the input E2 edits / E3 consumes):
`ChordGrid { bpm, beat_times, cells: [ChordCell { start_secs, end_secs, beat_index, chord: Option<ChordLabel>, confidence }], core_progression: [ChordLabel], verb_span }`.

**Every cell carries a confidence** — the E2 editor flags weak cells for correction. This is by design: full-mix recognition *will* miss borrowed/extended chords; the editable grid turns each miss into a two-second fix.

Tests: C-major chord → `C`, A-minor → `Am`, silence → N.C., periodic onset → correct BPM, repeating C-G-Am-F → verb of 4 bars with that core progression, synth audio → non-empty grid. (Caught + fixed a pitch-class offset bug that also affected the shipped tbviz chroma/circle-of-fifths labels.)

## Open decision — E4 convention (gate before rasterising)

The fretboard-diagram visual language must be pinned before E4's renderer is built (the spec parks these for the operator): **neck orientation** (horizontal vs vertical), **finger colour** (colour-per-finger vs monochrome), **handedness** (right vs left). Confirmed once, written into E4's issue, and rendered consistently thereafter.

## Non-goals / risk

- Not a stem separator, transcriber, or tab generator — one chord at a time, EADGBE only.
- Chord recognition is imperfect; E2 is the mitigation, not a nice-to-have.
- E5 mux alignment (ffmpeg frame-rate ↔ beat grid) is the integration risk — prove a 10-second end-to-end slice early.
