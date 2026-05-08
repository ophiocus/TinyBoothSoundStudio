# TBSS-FR-0005: Track telemetry — persistent per-track metadata

**Status**: design  
**Author(s)**: ophiocus  
**Filed**: 2026-05-08

## Summary

Compute a small set of pure-DSP audio-analysis features per track,
**once at first save** (after import or at recording stop), and
persist them inside the `.tinybooth` manifest as new optional fields
on `Track`. Surface the results as **glanceable tags in the Mix-tab
track lanes** so the user can see at a single glance whether a stem
is percussive or sustained, bright or dark, in what key, at what
tempo, and how AI-shimmery.

The same analyzer runs on Suno-imported stems and on user
recordings — telemetry is uniform across the two track origins.

## Motivation

The Mix-tab lanes currently show: a name, an A/B button, a "+
Correction" button, and a waveform. That's it. There's no visible
distinction at a glance between, say, a punchy percussive stem and
a sustained pad stem; between a stem in C major and one in F♯
minor; between a stem with strong AI-shimmer fingerprints and one
without.

A telemetry layer fixes that. Mathematically tractable per-track
features summarise each track's character and let the lanes
display short tag chips that capture, in a few characters each:

- **Pitch / key** ("E♭ min")
- **Tempo** ("128 BPM" — when detectable)
- **Rhythmic character** ("percussive" / "legato" / "mixed")
- **Brightness** ("bright" / "warm" / "dark")
- **Mood proxy** (arousal × valence — "energetic / dark", etc.)
- **AI fingerprint** ("⚠ shimmer" when cross-band coherence is
  anomalously low)

These tags also feed visualizer modes: the Onion Skin's axes
become user-pickable from any telemetry feature; new
"constellation" modes can plot per-track points across the project.

## Non-goals

- **Real-time** per-callback telemetry. That's a related-but-
  different concern (visualizer-driven, audio-thread budget,
  ephemeral). Keep separate.
- **ML / LLM-based analysis.** Genre classification, mood-label
  prediction, multi-pitch transcription — defer. Pure DSP only for
  now (there's plenty of material there).
- **Beat tracking with full sophistication.** A pure-DSP tempo
  estimator (autocorrelation of onset-strength curve, peak-pick
  with reasonable BPM range) is in scope; full Ellis/Krebs-grade
  beat tracking is a stretch goal.

## Schema

New optional field on `Track`:

```rust
pub struct Track {
    // ... existing fields ...
    
    /// Pre-computed analysis baked at first save (after recording
    /// or Suno import). Optional — projects from before this RFC
    /// have None, can be re-analyzed lazily on first open or via
    /// an explicit Project-tab button.
    #[serde(default)]
    pub telemetry: Option<TrackTelemetry>,
}

pub struct TrackTelemetry {
    /// Schema version. Bumped when the analyzer changes; older
    /// telemetry gets re-computed on next save.
    pub analyzer_version: u32,
    
    // ── Pitch / harmonic ──
    /// Median fundamental frequency in Hz, when the track has a
    /// meaningful pitch. None for percussion / unpitched material.
    pub pitch_median_hz: Option<f32>,
    /// Estimated key as `(tonic_pitch_class, mode)`, with confidence.
    /// `tonic_pitch_class` is 0..11 where 0=C; mode is Major | Minor.
    pub key: Option<KeyEstimate>,
    /// 12-bin chromagram averaged over the track.
    pub chroma_avg: [f32; 12],
    
    // ── Rhythmic ──
    /// Estimated tempo in BPM, when periodic structure is strong
    /// enough. None for non-periodic content.
    pub tempo_bpm: Option<f32>,
    /// Total number of detected onsets across the track.
    pub onset_count: u32,
    /// Onsets per second (averaged over the track).
    pub onset_rate_hz: f32,
    /// Mean inter-onset interval in seconds.
    pub mean_ioi_secs: Option<f32>,
    
    // ── Spectral character (track-averaged) ──
    /// Spectral centroid, normalised to [0, 1] across the spectrum.
    pub spectral_centroid_avg: f32,
    pub spectral_centroid_std: f32,
    /// Spectral flatness — 0=tonal, 1=noise (Wiener entropy).
    pub spectral_flatness_avg: f32,
    /// 85%-energy roll-off frequency, normalised to [0, 1].
    pub spectral_rolloff_avg: f32,
    
    // ── Dynamics ──
    /// Track-wide RMS in dBFS.
    pub rms_avg_db: f32,
    /// Standard deviation of short-term RMS — dynamic range proxy.
    pub rms_std_db: f32,
    /// Crest factor (peak / RMS) averaged over short windows.
    pub crest_factor_avg: f32,
    /// Integrated LUFS (BS.1770-4). For tracks where it makes sense.
    pub integrated_lufs: Option<f32>,
    
    // ── Envelope / articulation ──
    /// Mean attack time across detected onsets, in milliseconds.
    /// None when no onsets detected.
    pub mean_attack_ms: Option<f32>,
    /// Mean decay time (peak → 30% energy), milliseconds.
    pub mean_decay_ms: Option<f32>,
    /// 0=fully staccato (energy collapses immediately after each
    /// onset), 1=fully sustained (no decay between onsets).
    pub sustain_ratio: f32,
    
    // ── AI fingerprint ──
    /// Cross-band coherence score — see `docs/sound-vision-
    /// philosophy.md` §V. 1 = natural-recording-like
    /// correlation between adjacent-band micro-fluctuations;
    /// 0 = AI-style band-decorrelated noise. Lower scores
    /// suggest a candidate for the future Coherence Restoration
    /// filter.
    pub coherence_score: f32,
    
    // ── Mood proxies (derived from above; cached for fast UI) ──
    /// Arousal in [0, 1]: derived from RMS, onset rate, centroid.
    pub arousal: f32,
    /// Valence in [-1, 1]: derived from chromagram major/minor
    /// template fit and dissonance estimate. -1=dark/sad,
    /// +1=bright/happy.
    pub valence: f32,
}

pub struct KeyEstimate {
    pub tonic: u8,                     // 0..11, C=0
    pub mode: KeyMode,
    pub confidence: f32,               // 0..1
}

pub enum KeyMode { Major, Minor }
```

Storage cost: ~120 bytes per track (the chroma array dominates).
A 9-stem Suno project gains ~1 KB in its manifest. Negligible.

`#[serde(default)]` on the `telemetry` field means old manifests
load with `None`, no migration needed.

## Analyzer

New module `src/telemetry.rs`:

```rust
pub fn analyze_wav(path: &Path) -> Result<TrackTelemetry>;
pub const ANALYZER_VERSION: u32 = 1;
```

Internal structure (one pass over the file, accumulating
running statistics):

1. Decode the WAV with hound, sum to mono, into an
   `f32` buffer (or stream-process for very long tracks).
2. Compute long-window RMS / peak / crest-factor stats by
   walking the buffer once (O(N)).
3. STFT pass: 2048-sample window, hop 512, Hann-windowed
   (rustfft is already a dep). For each frame compute:
   - spectral centroid
   - spectral flatness
   - spectral rolloff
   - spectral flux (per-bin |X(t,k)| - |X(t-1,k)| with rectifier)
   - chromagram bin (fold octaves, sum into 12 pitch classes)
4. Onset detection: peak-pick on spectral-flux time series;
   adaptive threshold (median + k·MAD).
5. Per-onset envelope tracking: walk forward from each onset,
   record peak amplitude and time-to-30%-decay → mean attack /
   decay / sustain.
6. Pitch tracking: YIN per frame on a sub-sampled feed; median
   filter the per-frame f0 estimates; classify as pitched /
   unpitched on YIN's confidence.
7. Key detection: Krumhansl-Schmuckler — correlate the track-
   averaged chroma vector against the 24 (12 keys × 2 modes)
   reference profiles; pick highest correlation.
8. Tempo: autocorrelate the onset-strength curve over a 0.25-2 s
   lag range; convert to BPM; require minimum confidence (peak
   prominence) to emit a tempo at all.
9. Cross-band coherence: compute per-bin frame-to-frame
   derivative; pairwise correlation across adjacent-bin
   neighbourhoods; mean correlation = coherence score.
10. Derive arousal + valence from the above.

Cost: ~1-2 seconds per 3-minute mono stem at 48 kHz on a
modern CPU. Acceptable for a first-save offline pass; the
analyzer never touches the audio thread.

## Lifecycle

| Trigger | Behaviour |
|---|---|
| Suno import completes | Spawn a background analyzer for each newly-imported track. Save the project incrementally as each finishes. Status bar surfaces "Analyzing 4/9…". |
| `stop_take` (recording finalises) | Spawn a background analyzer for the new track. Show "Analyzing your take…" briefly. |
| Project trim applied | Re-analyze each cropped track (their character may have shifted slightly). Cheap relative to the trim's own disk I/O. |
| Existing project opened with `telemetry == None` | Lazy: don't analyze on open (would block the load). Show a one-time prompt: "Analyze tracks for telemetry?" Project-tab button does the same. |
| Schema version mismatch | On project load, if any track's `analyzer_version < current`, queue a re-analyze pass. Status bar confirms what re-ran. |

Async pattern: each `analyze_wav` runs on `std::thread::spawn`,
sends result via `mpsc::channel<(track_id, TrackTelemetry)>` back
to the UI thread. App drains the channel each frame, applies
results to `app.project.tracks[i].telemetry`, marks dirty, saves.

## UI: tag rendering in the Mix tab lanes

Each lane's header (currently 220 px wide, contains: name + A/B +
"+ Correction") gains a wrapping row of small chip widgets below
the existing buttons. Each chip:

```
[ icon  short-text ]
```

Compact. Fits ~3-4 chips per row in the existing header width.

Tag chip examples:

| Telemetry signal | Tag visual | Hover tooltip |
|---|---|---|
| `key = Eb minor, conf 0.78` | `♪ E♭m` | "Estimated key: E♭ minor (78% confidence)" |
| `tempo_bpm = 128.5` | `🥁 128` | "Tempo: 128.5 BPM" |
| `onset_rate_hz > 4` | `🔨 percussive` | "Dense onsets: 5.2/s" |
| `sustain_ratio < 0.3` | `⚡ staccato` | "Short notes: sustain 0.18" |
| `sustain_ratio > 0.7` | `🌊 sustained` | "Long notes: sustain 0.82" |
| `centroid_avg > 0.55` | `☀️ bright` | "Centroid: 0.58" |
| `centroid_avg < 0.30` | `🌙 warm` | "Centroid: 0.22" |
| `coherence_score < 0.4` | `⚠ shimmer` | "Cross-band coherence: 0.31. Possible AI fingerprint." |
| `valence > 0.4 && arousal > 0.6` | `✨ uplifting` | "High valence + high arousal" |
| `valence < -0.2 && arousal < 0.4` | `🌃 brooding` | "Low valence + low arousal" |

Logic for "show this chip" lives in `tag_chips_for(&telemetry)`
that returns a `Vec<Chip>`. Project-level toggles let the user
hide tag categories they don't care about.

Behaviour notes:

- Chips never reflow into a second visible row that pushes the
  waveform down. If they don't fit, the overflow is hidden
  behind a `…` chip whose tooltip lists the rest.
- Hover any chip → full tooltip with the underlying numeric.
- Click a chip → opens a "Track telemetry" panel with all
  features listed.

## Visualizer integration (cross-cutting)

Once telemetry exists, the visualizer's Onion Skin mode gains
**user-pickable axes**: instead of hard-coded
`(centroid, RMS)`, the user picks any two telemetry features as
X / Y. Realistic combinations: `(arousal, valence)` for mood
space; `(centroid, sustain_ratio)` for "bright-staccato vs
warm-legato"; `(onset_rate, coherence)` for diagnostic.

A new mode — **Constellation** — plots EVERY track in the
project as a moving point in 2D telemetry space. You see the
mix's centre of mass and how each stem contributes.

These are downstream features; the tag-on-lane work doesn't
depend on them.

## Phase plan

**Phase 1 — minimum viable** (one v0.4.x patch):

- Schema additions on `Track`.
- `analyze_wav` covering: RMS, peak, crest, spectral centroid,
  spectral flatness, spectral rolloff, onset detection, onset
  rate, sustain ratio. (Skip pitch / key / tempo for phase 1 —
  those are the heavier algorithms.)
- Synchronous analysis at import time and at stop_take time.
  Block briefly; status bar shows "analyzing".
- 4 tag chips in the lane: brightness, sustain, density,
  loudness.

**Phase 2 — pitch and harmony**:

- YIN pitch tracker → `pitch_median_hz`.
- Chromagram + Krumhansl key estimation → `key`.
- Two more tag chips: key (`♪ E♭m`), pitch class.

**Phase 3 — tempo + AI fingerprint**:

- Onset-strength autocorrelation → `tempo_bpm`.
- Cross-band coherence → `coherence_score` + `⚠ shimmer` chip.
- Async analyzer pipeline (background threads instead of
  blocking save).

**Phase 4 — visualizer integration**:

- Onion Skin user-pickable axes.
- Constellation mode.

## Open questions

- **Tracks with no obvious pitch / key** (drums) — the analyzer
  has to know to suppress the "pitch" / "key" tags rather than
  emit nonsense. Use the spectral-flatness threshold (high
  flatness = noise-dominated → no key tag).
- **Mood proxies** — the simple weighted-sum approach for
  arousal / valence will produce reasonable but not
  authoritative results. Acknowledge it; show the underlying
  numbers in the tooltip so the user can override their
  interpretation.
- **Re-analysis on every trim** — could we just rescale the
  numerics rather than reanalysing? RMS / centroid / etc. are
  per-frame, so the average shifts when frames are cut. Cheap
  to redo from scratch; complicated to incrementally update.
  Default: re-analyse.
- **Analyzer correctness validation** — for v0.5 ship, write
  unit tests that feed the analyzer **synthetic signals with
  known properties** (a 1 kHz sine → pitch 1000 Hz; white
  noise → high spectral flatness; steady rhythm → predictable
  tempo). Build confidence before letting the chips appear in
  the UI.

## What this would cost

- New module `src/telemetry.rs` ~600 LOC for phase 1; +400 LOC
  per subsequent phase.
- `Track` schema gains one Option<TrackTelemetry>; ~120 bytes
  per track on disk.
- UI: ~150 LOC for the chip rendering + tooltip wiring.
- No new crates needed (rustfft + hound already in tree).
- Tests: ~10 unit tests on synthetic signals.

Ship as v0.5.0 alongside the take-browser / reference A/B work,
or as a focused v0.4.x patch on its own. Phase 1 is small enough
that the latter is reasonable.

---

## References

- Lartillot, O. *MIRtoolbox*. The MIR feature catalogue most of
  this riffs on.
- de Cheveigné, A. & Kawahara, H. (2002). *YIN, a fundamental
  frequency estimator for speech and music.* JASA.
- Krumhansl, C. (1990). *Cognitive Foundations of Musical Pitch.*
  Oxford. (The Krumhansl-Schmuckler key profiles.)
- Bello, J. P. et al. (2005). *A tutorial on onset detection in
  music signals.* IEEE TASLP.
- Müller, M. (2015). *Fundamentals of Music Processing.*
  Springer. The reference textbook covering everything above.
- TBSS internal: `docs/sound-vision-philosophy.md` §V on the
  cross-band coherence score and its relationship to AI
  fingerprint detection.
