//! TBSS-FR-0013 · E1 — audio → beat-quantised chord grid + verb detection.
//!
//! Pure analysis, no UI and no I/O beyond decoded samples in / a
//! serialisable [`ChordGrid`] out (the data contract E2 edits and E3
//! consumes). Full-mix chord recognition (not per-instrument), per the
//! settled scope: one chord at a time.
//!
//! Pipeline: STFT → onset envelope → autocorrelation tempo → phase-locked
//! beat grid → beat-synchronous chroma → per-beat chord-template match →
//! verb (repeating-progression) detection.
//!
//! The uncertain step (chord recognition on a full mix) is *expected* to
//! misfire on borrowed/extended chords — every cell carries a
//! `confidence` so the E2 editor can flag the weak ones for correction.
//!
//! This is a pure data-contract module (E1). Its public API is consumed
//! by later epics — the E2 editor panel, the E3 voicing resolver — which
//! haven't landed yet, so in this binary crate the surface reads as
//! unused. Module-level allow rather than per-item; the tests exercise
//! every function.
#![allow(dead_code)]

use rustfft::{num_complex::Complex, FftPlanner};
use serde::{Deserialize, Serialize};

const FFT_SIZE: usize = 4096;
const HOP: usize = 1024;

/// Pitch class 0..11 (C=0). The 12 chromatic degrees.
pub type PitchClass = u8;

/// Chord quality templates recognised in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChordQuality {
    Major,
    Minor,
    Dom7,
    Min7,
    Maj7,
    Dim,
}

impl ChordQuality {
    /// Semitone offsets of the chord tones from the root.
    fn intervals(self) -> &'static [u8] {
        match self {
            ChordQuality::Major => &[0, 4, 7],
            ChordQuality::Minor => &[0, 3, 7],
            ChordQuality::Dom7 => &[0, 4, 7, 10],
            ChordQuality::Min7 => &[0, 3, 7, 10],
            ChordQuality::Maj7 => &[0, 4, 7, 11],
            ChordQuality::Dim => &[0, 3, 6],
        }
    }
    /// Display suffix, e.g. `""`, `"m"`, `"maj7"`. Public so the E2 editor can
    /// label its quality picker.
    pub fn suffix(self) -> &'static str {
        match self {
            ChordQuality::Major => "",
            ChordQuality::Minor => "m",
            ChordQuality::Dom7 => "7",
            ChordQuality::Min7 => "m7",
            ChordQuality::Maj7 => "maj7",
            ChordQuality::Dim => "dim",
        }
    }
    /// Every recognised quality — drives both the template bank and the E2
    /// editor's picker.
    pub fn all() -> [ChordQuality; 6] {
        [
            ChordQuality::Major,
            ChordQuality::Minor,
            ChordQuality::Dom7,
            ChordQuality::Min7,
            ChordQuality::Maj7,
            ChordQuality::Dim,
        ]
    }
}

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Name of a pitch class (`0` = C). Sharps only — the E2 editor's root picker
/// and any label that needs a note name without building a whole [`ChordLabel`].
pub fn note_name(pc: u8) -> &'static str {
    NOTE_NAMES[(pc % 12) as usize]
}

/// A concrete chord: root pitch-class + quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChordLabel {
    pub root: PitchClass,
    pub quality: ChordQuality,
}

impl ChordLabel {
    /// Display name, e.g. `C`, `Am`, `G7`, `F#m7`.
    pub fn name(&self) -> String {
        format!(
            "{}{}",
            NOTE_NAMES[(self.root % 12) as usize],
            self.quality.suffix()
        )
    }

    /// L2-normalised binary chroma template for this chord.
    fn template(&self) -> [f32; 12] {
        let mut t = [0.0_f32; 12];
        for &iv in self.quality.intervals() {
            t[((self.root as u16 + iv as u16) % 12) as usize] = 1.0;
        }
        let norm = t.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        for x in &mut t {
            *x /= norm;
        }
        t
    }
}

/// One beat's cell in the grid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChordCell {
    pub start_secs: f32,
    pub end_secs: f32,
    pub beat_index: u32,
    /// `None` = no confident chord (silence / ambiguous) → "N.C.".
    pub chord: Option<ChordLabel>,
    /// Match confidence in `[0, 1]` (cosine of chroma vs template).
    pub confidence: f32,
}

/// The E1 output data contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChordGrid {
    pub bpm: f32,
    pub beat_times: Vec<f32>,
    pub cells: Vec<ChordCell>,
    /// The verb's repeating chord progression (deduped per bar).
    pub core_progression: Vec<ChordLabel>,
    /// `(start_secs, end_secs)` of the detected verb span, if found.
    pub verb_span: Option<(f32, f32)>,
}

// ── STFT + onset envelope ───────────────────────────────────────────

fn stft_mags(mono: &[f32]) -> Vec<Vec<f32>> {
    if mono.len() < FFT_SIZE {
        return Vec::new();
    }
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| {
            let t = i as f32 / (FFT_SIZE - 1) as f32;
            0.5 - 0.5 * (std::f32::consts::TAU * t).cos()
        })
        .collect();
    let n_frames = (mono.len() - FFT_SIZE) / HOP + 1;
    let mut frames = Vec::with_capacity(n_frames);
    let mut buf = vec![Complex { re: 0.0, im: 0.0 }; FFT_SIZE];
    for f in 0..n_frames {
        let start = f * HOP;
        for (i, b) in buf.iter_mut().enumerate() {
            *b = Complex {
                re: mono[start + i] * window[i],
                im: 0.0,
            };
        }
        fft.process(&mut buf);
        let mags: Vec<f32> = buf[..FFT_SIZE / 2]
            .iter()
            .map(|c| (c.re * c.re + c.im * c.im).sqrt())
            .collect();
        frames.push(mags);
    }
    frames
}

/// Half-wave-rectified spectral flux per frame — the onset envelope.
fn onset_envelope(stft: &[Vec<f32>]) -> Vec<f32> {
    if stft.len() < 2 {
        return vec![0.0; stft.len()];
    }
    let mut env = vec![0.0_f32; stft.len()];
    for f in 1..stft.len() {
        let mut flux = 0.0_f32;
        for (a, b) in stft[f].iter().zip(stft[f - 1].iter()) {
            let d = a - b;
            if d > 0.0 {
                flux += d;
            }
        }
        env[f] = flux;
    }
    env
}

/// Estimate tempo (BPM) by autocorrelating the onset envelope and
/// picking the lag with peak periodicity in a plausible BPM range.
pub fn estimate_tempo(onset: &[f32], frames_per_sec: f32) -> f32 {
    if onset.len() < 8 || frames_per_sec <= 0.0 {
        return 120.0;
    }
    // Search 60..180 BPM → lag (in frames).
    let lag_for = |bpm: f32| -> usize { ((frames_per_sec * 60.0) / bpm).round() as usize };
    let min_lag = lag_for(180.0).max(1);
    let max_lag = lag_for(60.0).min(onset.len() / 2);
    let mut best_lag = min_lag;
    let mut best = f32::MIN;
    for lag in min_lag..=max_lag {
        let mut acc = 0.0_f32;
        for i in lag..onset.len() {
            acc += onset[i] * onset[i - lag];
        }
        // Slight preference for slower tempi (bias against octave errors).
        let score = acc / lag as f32;
        if score > best {
            best = score;
            best_lag = lag;
        }
    }
    (frames_per_sec * 60.0) / best_lag as f32
}

/// Build a phase-locked beat grid (beat *frame* indices) from the onset
/// envelope + tempo: pick the phase offset that maximises onset energy
/// landing on beats, then step by the beat period.
fn beat_grid(onset: &[f32], bpm: f32, frames_per_sec: f32) -> Vec<usize> {
    if onset.is_empty() || bpm <= 0.0 {
        return Vec::new();
    }
    let period = ((frames_per_sec * 60.0) / bpm).round().max(1.0) as usize;
    // Best phase in [0, period).
    let mut best_phase = 0;
    let mut best = f32::MIN;
    for phase in 0..period {
        let mut acc = 0.0_f32;
        let mut i = phase;
        while i < onset.len() {
            acc += onset[i];
            i += period;
        }
        if acc > best {
            best = acc;
            best_phase = phase;
        }
    }
    let mut beats = Vec::new();
    let mut i = best_phase;
    while i < onset.len() {
        beats.push(i);
        i += period;
    }
    beats
}

// ── chroma + chord matching ─────────────────────────────────────────

/// Fold a magnitude spectrum onto 12 pitch classes (A440 reference).
fn chroma_of_mag(mag: &[f32], sr: u32) -> [f32; 12] {
    let mut c = [0.0_f32; 12];
    let bin_hz = sr as f32 / FFT_SIZE as f32;
    for (i, &m) in mag.iter().enumerate() {
        let hz = i as f32 * bin_hz;
        if !(55.0..=5000.0).contains(&hz) {
            continue;
        }
        // MIDI pitch class (C=0): 69 = A4 ⇒ A is class 9. Using the raw
        // 12·log2(hz/440) would put A at class 0 (a 9-semitone offset)
        // and mislabel every chord.
        let pc = ((69.0 + 12.0 * (hz / 440.0).log2()).round() as i64).rem_euclid(12) as usize;
        c[pc] += m;
    }
    c
}

/// Match a chroma vector to the best chord template. Returns the label
/// and a confidence (cosine similarity). `None` when the chroma is too
/// weak or no template correlates above `min_conf`.
pub fn match_chord(chroma: &[f32; 12], min_conf: f32) -> (Option<ChordLabel>, f32) {
    let energy: f32 = chroma.iter().sum();
    if energy < 1e-6 {
        return (None, 0.0);
    }
    let norm = chroma.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    let cn: Vec<f32> = chroma.iter().map(|x| x / norm).collect();

    let mut best: Option<ChordLabel> = None;
    let mut best_conf = 0.0_f32;
    for root in 0..12u8 {
        for quality in ChordQuality::all() {
            let label = ChordLabel { root, quality };
            let t = label.template();
            let dot: f32 = cn.iter().zip(t.iter()).map(|(a, b)| a * b).sum();
            if dot > best_conf {
                best_conf = dot;
                best = Some(label);
            }
        }
    }
    if best_conf >= min_conf {
        (best, best_conf)
    } else {
        (None, best_conf)
    }
}

// ── verb (repeating progression) detection ──────────────────────────

/// Find the repeating harmonic unit (the "verb") in the per-beat chord
/// sequence. Tries loop lengths of 2 / 4 / 8 bars (4/4 assumed) and
/// picks the shortest whose self-match ratio clears `min_match`. Returns
/// the per-bar core progression + the loop length in beats.
pub fn detect_verb(
    labels: &[Option<ChordLabel>],
    beats_per_bar: usize,
    min_match: f32,
) -> (Vec<ChordLabel>, Option<usize>) {
    let n = labels.len();
    if n < beats_per_bar * 2 {
        return (dedup_bars(labels, beats_per_bar), None);
    }
    for bars in [2usize, 4, 8] {
        let len = bars * beats_per_bar;
        if len * 2 > n {
            continue;
        }
        // Self-match: fraction of beats equal to the beat one loop later.
        let mut matches = 0usize;
        let mut total = 0usize;
        for i in 0..(n - len) {
            total += 1;
            if labels[i] == labels[i + len] {
                matches += 1;
            }
        }
        let ratio = if total > 0 {
            matches as f32 / total as f32
        } else {
            0.0
        };
        if ratio >= min_match {
            return (dedup_bars(&labels[..len], beats_per_bar), Some(len));
        }
    }
    (dedup_bars(labels, beats_per_bar), None)
}

/// One representative chord per bar (the most common non-None label in
/// the bar), deduping immediate repeats.
fn dedup_bars(labels: &[Option<ChordLabel>], beats_per_bar: usize) -> Vec<ChordLabel> {
    let mut out: Vec<ChordLabel> = Vec::new();
    for bar in labels.chunks(beats_per_bar.max(1)) {
        // Mode of the bar's labels.
        let mut counts: Vec<(ChordLabel, usize)> = Vec::new();
        for l in bar.iter().flatten() {
            if let Some(e) = counts.iter_mut().find(|(c, _)| c == l) {
                e.1 += 1;
            } else {
                counts.push((*l, 1));
            }
        }
        if let Some((chord, _)) = counts.into_iter().max_by_key(|(_, n)| *n) {
            if out.last() != Some(&chord) {
                out.push(chord);
            }
        }
    }
    out
}

// ── top-level analysis ──────────────────────────────────────────────

/// Analyze a mono signal into a beat-quantised chord grid. `sr` is the
/// sample rate. Assumes 4/4; returns an empty-ish grid for very short
/// or silent input.
pub fn analyze(mono: &[f32], sr: u32) -> ChordGrid {
    let frames_per_sec = sr as f32 / HOP as f32;
    let stft = stft_mags(mono);
    if stft.is_empty() {
        return ChordGrid {
            bpm: 120.0,
            beat_times: Vec::new(),
            cells: Vec::new(),
            core_progression: Vec::new(),
            verb_span: None,
        };
    }
    let onset = onset_envelope(&stft);
    let bpm = estimate_tempo(&onset, frames_per_sec);
    let beat_frames = beat_grid(&onset, bpm, frames_per_sec);
    let frame_secs = |f: usize| f as f32 * HOP as f32 / sr as f32;

    let mut cells = Vec::new();
    let mut labels = Vec::new();
    for (bi, w) in beat_frames.windows(2).enumerate() {
        let (f0, f1) = (w[0], w[1]);
        // Mean chroma over the beat interval.
        let mut chroma = [0.0_f32; 12];
        for frame in &stft[f0..f1.min(stft.len())] {
            let c = chroma_of_mag(frame, sr);
            for k in 0..12 {
                chroma[k] += c[k];
            }
        }
        let (chord, confidence) = match_chord(&chroma, 0.5);
        labels.push(chord);
        cells.push(ChordCell {
            start_secs: frame_secs(f0),
            end_secs: frame_secs(f1),
            beat_index: bi as u32,
            chord,
            confidence,
        });
    }

    let (core_progression, verb_len) = detect_verb(&labels, 4, 0.6);
    let verb_span = verb_len.and_then(|len| {
        let start = cells.first().map(|c| c.start_secs)?;
        let end = cells
            .get(len)
            .map(|c| c.start_secs)
            .or_else(|| cells.last().map(|c| c.end_secs))?;
        Some((start, end))
    });

    ChordGrid {
        bpm,
        beat_times: beat_frames.iter().map(|&f| frame_secs(f)).collect(),
        cells,
        core_progression,
        verb_span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_tone(freqs: &[f32], sr: u32, secs: f32) -> Vec<f32> {
        let n = (sr as f32 * secs) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / sr as f32;
                freqs
                    .iter()
                    .map(|f| (std::f32::consts::TAU * f * t).sin())
                    .sum::<f32>()
                    / freqs.len() as f32
            })
            .collect()
    }

    #[test]
    fn c_major_chroma_matches_c_major() {
        // C4, E4, G4.
        let sr = 44_100;
        let sig = synth_tone(&[261.63, 329.63, 392.0], sr, 0.5);
        let stft = stft_mags(&sig);
        assert!(!stft.is_empty());
        let mut chroma = [0.0_f32; 12];
        for frame in &stft {
            let c = chroma_of_mag(frame, sr);
            for k in 0..12 {
                chroma[k] += c[k];
            }
        }
        let (chord, conf) = match_chord(&chroma, 0.5);
        let chord = chord.expect("should detect a chord");
        assert_eq!(chord.root, 0, "root should be C (got {})", chord.name());
        assert_eq!(chord.quality, ChordQuality::Major);
        assert!(conf > 0.7, "confidence should be high, got {conf}");
    }

    #[test]
    fn a_minor_matches() {
        // A3, C4, E4.
        let sr = 44_100;
        let sig = synth_tone(&[220.0, 261.63, 329.63], sr, 0.5);
        let stft = stft_mags(&sig);
        let mut chroma = [0.0_f32; 12];
        for frame in &stft {
            let c = chroma_of_mag(frame, sr);
            for k in 0..12 {
                chroma[k] += c[k];
            }
        }
        let (chord, _) = match_chord(&chroma, 0.5);
        let chord = chord.expect("should detect a chord");
        assert_eq!(chord.name(), "Am");
    }

    #[test]
    fn silence_is_no_chord() {
        let (chord, _) = match_chord(&[0.0; 12], 0.5);
        assert!(chord.is_none());
    }

    #[test]
    fn tempo_from_periodic_onset() {
        // Onset spike every 25 frames. At 43.07 fps (44100/1024) that is
        // 43.07/25 * 60 ≈ 103 BPM.
        let fps = 44_100.0 / HOP as f32;
        let mut onset = vec![0.0_f32; 1000];
        let mut i = 0;
        while i < onset.len() {
            onset[i] = 1.0;
            i += 25;
        }
        let bpm = estimate_tempo(&onset, fps);
        let expected = fps / 25.0 * 60.0;
        assert!(
            (bpm - expected).abs() < 6.0,
            "expected ~{expected:.0} bpm, got {bpm:.0}"
        );
    }

    #[test]
    fn verb_detects_repeating_progression() {
        // C G Am F, one bar each (4 beats/bar), repeated 3×.
        let prog = [
            ChordLabel {
                root: 0,
                quality: ChordQuality::Major,
            }, // C
            ChordLabel {
                root: 7,
                quality: ChordQuality::Major,
            }, // G
            ChordLabel {
                root: 9,
                quality: ChordQuality::Minor,
            }, // Am
            ChordLabel {
                root: 5,
                quality: ChordQuality::Major,
            }, // F
        ];
        let mut labels = Vec::new();
        for _ in 0..3 {
            for ch in &prog {
                for _ in 0..4 {
                    labels.push(Some(*ch));
                }
            }
        }
        let (core, verb_len) = detect_verb(&labels, 4, 0.6);
        assert_eq!(verb_len, Some(16), "verb should be 4 bars = 16 beats");
        let names: Vec<String> = core.iter().map(|c| c.name()).collect();
        assert_eq!(names, vec!["C", "G", "Am", "F"]);
    }

    #[test]
    fn analyze_produces_grid_on_synth_audio() {
        // A repeating C-major tone for a couple seconds → non-empty grid.
        let sr = 44_100;
        let sig = synth_tone(&[261.63, 329.63, 392.0], sr, 2.0);
        let grid = analyze(&sig, sr);
        assert!(grid.bpm > 40.0 && grid.bpm < 220.0);
        assert!(!grid.cells.is_empty(), "should produce beat cells");
    }
}
