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

## E4 convention — LOCKED

The fretboard-diagram visual language, pinned before E4's renderer is built:

- **Neck orientation:** horizontal (fretboard lying left→right).
- **Fingers:** numbers **1–4** (not colours), sourced from a **chord-shape database** — a chord is only renderable if it exists in the DB.
- **Handedness:** a display-time boolean + mirror function; stored data is canonical right-handed, the renderer mirrors on demand.

## E3-groundwork — chord-voicing engine (LANDED)

`src/chorddb.rs`, pure + unit-tested. The locked E4 decision "a chord is only renderable if in the DB" made the database first-class, so rather than hand-enter shapes it is **generated**: a voicing's left hand is a matrix (fret + finger per string); enumerate playable fret combinations in a moving neck window, keep only those whose sounding pitch-classes spell the target chord (root in bass, no foreign notes), assign an ergonomic fingering (lowest-fret barre + greedy 1→4), score and rank. **Every voicing is pitch-class-correct by construction.**

- **16 qualities** (maj / min / dom7 / min7 / maj7 / dim / dim7 / aug / sus2 / sus4 / 6 / m6 / m7b5 / add9 / 9 / 5) × 12 roots → **2,287 ranked voicings** (top 12 per chord) — a 50× jump over the 44 hand-entered rows, erasing the research file's own 11-item `gaps` list.
- The **5th is modelled as optional**, so the idiomatic open C7 (`x32310`, no 5th) is a first-class voicing, not a special case.
- The **44 verified voicings** (`docs/research/…verified.json`) become the *golden set*: concrete open/named shapes are overlaid at rank 0 (recognisable grips + canonical fingering), and every one is a correctness anchor (a test asserts each spells its chord).
- `ChordDb::best_near(root, q, prev_base_fret)` is E3's fret-travel-minimising selector; `for_label()` bridges straight from an E1 `ChordLabel`.
- v1 playability model (documented, relaxable): contiguous sounding strings, root-position only, ≤4 fingers with one barre, frets 0–12, span ≤4.

## E3 — voicing resolver (LANDED)

`src/chordvoice.rs`, pure + tested. Collapses E1's per-beat cells into `VoicedSpan`s (consecutive beats holding the same chord merge into one span), then picks one voicing per span via `ChordDb::best_near`, **threading the previous `base_fret` forward** so diagrams don't leap up and down the neck between chords. N.C. spans carry no voicing and don't reset hand position.

The fret-travel test was **mutation-checked**: replacing the threaded position with `None` makes it fail (Am picks `base_fret` 1 when 0 was reachable), so it genuinely constrains the resolver rather than passing vacuously.

## E5 — video build + mux (LANDED, end-to-end slice proven)

`src/chordvideo.rs`. One E4 diagram per span → H.264 video muxed over the original untouched audio. Decisions, each driven by something measured:

- **concat demuxer, not `image2`.** A chord holds for seconds, so one PNG per *frame* would write ~5,400 files for a 3-minute song at 30 fps. The concat demuxer takes one image per span plus an explicit `duration` — it encodes E1's beat timings directly instead of approximating them by frame duplication.
- **The final entry is repeated** — the concat demuxer ignores the last entry's `duration`; without the repeat the closing chord flashes by in a single frame.
- **ffmpeg runs with the frame directory as its cwd**, with bare filenames in the concat list — this sidesteps Windows drive-letter/backslash escaping in concat paths entirely.
- **The encoder is probed, not hardcoded.** Not every ffmpeg ships `libx264` (this workstation's build is `--disable-libx264`, offering `libopenh264` + hardware encoders). `pick_h264_encoder()` scans `ffmpeg -encoders` in preference order, software first for reproducibility.
- **Audio is stream-copied** (`-c:a copy`), AAC only as a fallback. Measured caveat: MP4 *does* accept `pcm_s16le`, so a WAV source copies bit-exact but plays in fewer players — the compatibility re-encode belongs in the E2 UI as an explicit opt-in, never a silent downgrade at this layer.

**The FR's headline risk is now retired.** The `#[ignore]`d `e2e_ten_second_slice` test proves the whole chain for real: synthesised C-G-Am-F in → E1 detects 117.5 BPM / 18 cells → E3 coalesces to exactly **4 spans "C G Am F"** → 10.00 s H.264 out, ffprobe-verified for both streams and duration drift.

## Remaining

**E2 — the editable chord-grid panel** is the only epic left: a new tab showing the grid, flagging low-confidence cells, letting the operator correct them (and hear the correction via the FR-0009 tone synth) before hitting render. Everything it orchestrates — analyse, resolve, preview a diagram, build the video — already exists and is tested.

## Non-goals / risk

- Not a stem separator, transcriber, or tab generator — one chord at a time, EADGBE only.
- Chord recognition is imperfect; E2 is the mitigation, not a nice-to-have.
- ~~E5 mux alignment (ffmpeg frame-rate ↔ beat grid) is the integration risk — prove a 10-second end-to-end slice early.~~ **Retired** — the slice is proven and lives on as a test (see E5 above).
