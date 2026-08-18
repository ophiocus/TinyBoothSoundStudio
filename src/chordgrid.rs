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

/// Tempo search band, in BPM.
const MIN_BPM: f32 = 60.0;
const MAX_BPM: f32 = 180.0;
/// Centre and width (in octaves) of the log-Gaussian tempo prior that guards
/// against half-/double-time octave errors.
const TEMPO_PREF: f32 = 120.0;
const TEMPO_SPREAD: f32 = 0.9;

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

/// Log-domain cost of switching chords between adjacent beats.
///
/// Labelling each beat independently makes the grid jitter: on a dense mix the
/// per-beat chroma wobbles enough that the argmax flips constantly, producing
/// hundreds of one-beat "chords" in a song that really has a handful. A chord
/// is a *held* object, so decoding needs a memory of the previous beat. This is
/// the price a candidate must beat to displace the incumbent — higher means
/// longer, steadier chords.
///
/// Swept against a real 4-minute rock track (239.5 s, 514 beats at 126.8 BPM):
///
/// | penalty | spans | mean per chord |
/// |--------:|------:|---------------:|
/// | 0.00    |   375 |          0.6 s |
/// | 0.15    |   101 |          2.4 s |
/// | 0.30    |    44 |          5.4 s |
/// | 0.60    |    29 |          8.3 s |
/// | 1.10    |    12 |         20.0 s |
///
/// One bar at that tempo is ~1.9 s, so a chord held for one-to-two bars lands
/// around 2–4 s — the 0.15–0.3 region. The value below sits at the fast end of
/// that on purpose: **slight over-segmentation is the safe error**, because the
/// E2 editor can merge neighbouring spans trivially, whereas a chord change the
/// decoder never emitted cannot be recovered by editing.
const CHORD_CHANGE_PENALTY: f32 = 0.18;

/// Per-quality prior, applied multiplicatively to the template match.
///
/// L2-normalised binary templates are **not** scale-fair across qualities: a
/// four-note seventh spreads its unit norm over four bins (0.5 each) while a
/// triad spreads it over three (0.577 each), so on a dense chroma — where every
/// pitch class carries some energy — the seventh captures more of it and wins
/// on cosine alone. Left uncorrected the analyser labels almost everything a
/// seventh (on real material it produced Dmaj7 / D7 / Dm7 in near-equal
/// numbers, which cannot all be right). Triads are also simply more common, so
/// an extension has to genuinely earn its extra note.
fn quality_prior(q: ChordQuality) -> f32 {
    match q {
        ChordQuality::Major | ChordQuality::Minor => 1.0,
        ChordQuality::Dom7 | ChordQuality::Min7 => 0.90,
        ChordQuality::Maj7 => 0.88,
        ChordQuality::Dim => 0.85,
    }
}

/// Viterbi-decode a whole track's beat chromas into a stable chord sequence.
///
/// States are the 72 chords plus a "no chord" state. Emission is the
/// prior-weighted cosine against each template; transitions are free when the
/// chord holds and cost [`CHORD_CHANGE_PENALTY`] when it changes. Returns the
/// chosen label per beat together with its *raw* cosine, so the confidence the
/// editor displays still reflects real match quality rather than the decoder's
/// internal score.
pub fn smooth_chords(chromas: &[[f32; 12]], min_conf: f32) -> Vec<(Option<ChordLabel>, f32)> {
    smooth_chords_with(chromas, min_conf, CHORD_CHANGE_PENALTY)
}

/// [`smooth_chords`] with an explicit change penalty — lets the penalty be
/// swept against real material instead of guessed.
pub fn smooth_chords_with(
    chromas: &[[f32; 12]],
    min_conf: f32,
    change_penalty: f32,
) -> Vec<(Option<ChordLabel>, f32)> {
    if chromas.is_empty() {
        return Vec::new();
    }
    // State 0 = N.C.; states 1..=72 are (root, quality) pairs.
    let mut states: Vec<Option<ChordLabel>> = Vec::with_capacity(73);
    states.push(None);
    for root in 0..12u8 {
        for quality in ChordQuality::all() {
            states.push(Some(ChordLabel { root, quality }));
        }
    }
    let templates: Vec<Option<[f32; 12]>> =
        states.iter().map(|s| s.map(|l| l.template())).collect();
    let n_states = states.len();

    // Raw cosine per (beat, state) — kept for reporting confidence.
    let mut raw = vec![0.0_f32; chromas.len() * n_states];
    let mut logem = vec![0.0_f32; chromas.len() * n_states];
    for (t, chroma) in chromas.iter().enumerate() {
        let energy: f32 = chroma.iter().sum();
        let norm = chroma.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        for s in 0..n_states {
            let (r, weighted) = match (&templates[s], states[s]) {
                (Some(t_vec), Some(label)) if energy >= 1e-6 => {
                    let dot: f32 = chroma
                        .iter()
                        .zip(t_vec.iter())
                        .map(|(a, b)| (a / norm) * b)
                        .sum();
                    (dot, dot * quality_prior(label.quality))
                }
                // N.C., or a silent beat: sits at the confidence floor, so it
                // wins only when nothing matches convincingly.
                _ => (0.0, min_conf),
            };
            raw[t * n_states + s] = r;
            logem[t * n_states + s] = weighted.max(1e-6).ln();
        }
    }

    // Forward pass. The transition matrix is uniform apart from the self-loop,
    // so each step only needs the running best previous state — O(T·S).
    let mut dp = vec![f32::NEG_INFINITY; chromas.len() * n_states];
    let mut back = vec![0_u16; chromas.len() * n_states];
    dp[..n_states].copy_from_slice(&logem[..n_states]);
    for t in 1..chromas.len() {
        let (prev_off, cur_off) = ((t - 1) * n_states, t * n_states);
        let mut best_prev = 0usize;
        for s in 1..n_states {
            if dp[prev_off + s] > dp[prev_off + best_prev] {
                best_prev = s;
            }
        }
        let switch_score = dp[prev_off + best_prev] - change_penalty;
        for s in 0..n_states {
            let stay = dp[prev_off + s];
            let (score, from) = if stay >= switch_score {
                (stay, s)
            } else {
                (switch_score, best_prev)
            };
            dp[cur_off + s] = score + logem[cur_off + s];
            back[cur_off + s] = from as u16;
        }
    }

    // Backtrace.
    let last = chromas.len() - 1;
    let mut s = (0..n_states)
        .max_by(|&a, &b| {
            dp[last * n_states + a]
                .partial_cmp(&dp[last * n_states + b])
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(0);
    let mut path = vec![0usize; chromas.len()];
    for t in (0..chromas.len()).rev() {
        path[t] = s;
        s = back[t * n_states + s] as usize;
    }

    path.iter()
        .enumerate()
        .map(|(t, &s)| (states[s], raw[t * n_states + s]))
        .collect()
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
    // Search MIN_BPM..MAX_BPM → lag (in frames). Round *outward* (ceil for the
    // fast end, floor for the slow end) so the integer lag can never represent
    // a tempo outside the band: plain `.round()` turned the 180 BPM edge into
    // lag 14, i.e. 184.6 BPM at 43.07 fps — outside the very range being
    // searched, and the bucket the estimator then pinned itself to.
    let lag_f = |bpm: f32| (frames_per_sec * 60.0) / bpm;
    let min_lag = (lag_f(MAX_BPM).ceil() as usize).max(1);
    let max_lag = (lag_f(MIN_BPM).floor() as usize).min(onset.len() / 2);
    if max_lag <= min_lag {
        return 120.0;
    }

    // Score each candidate lag.
    let mut scores = vec![0.0_f32; max_lag + 1];
    for (lag, slot) in scores
        .iter_mut()
        .enumerate()
        .take(max_lag + 1)
        .skip(min_lag)
    {
        let mut acc = 0.0_f32;
        for i in lag..onset.len() {
            acc += onset[i] * onset[i - lag];
        }
        // Normalise by the number of overlapping terms, not by the lag. The
        // overlap shrinks as lag grows, so an un-normalised sum already favours
        // fast tempi; the previous `acc / lag` divided by lag on top of that,
        // compounding the bias toward short lags rather than countering it (the
        // comment claimed the opposite of what the arithmetic did).
        let overlap = (onset.len() - lag) as f32;
        let mean = acc / overlap.max(1.0);
        // Octave-error guard done properly: a log-Gaussian prior over tempo,
        // centred on TEMPO_PREF. Symmetric in octaves, so it penalises
        // half-time and double-time equally instead of favouring one end.
        let bpm = (frames_per_sec * 60.0) / lag as f32;
        let z = (bpm / TEMPO_PREF).log2() / TEMPO_SPREAD;
        *slot = mean * (-0.5 * z * z).exp();
    }

    let mut best_lag = min_lag;
    for lag in min_lag..=max_lag {
        if scores[lag] > scores[best_lag] {
            best_lag = lag;
        }
    }

    // Sub-frame refinement. Integer lags quantise coarsely at speed (at 43 fps,
    // lag 14/15/16 land on 184.6/172.3/161.5 BPM — ~12 BPM apart), so fit a
    // parabola through the peak and its neighbours.
    let mut lag = best_lag as f32;
    if best_lag > min_lag && best_lag < max_lag {
        let (a, b, c) = (scores[best_lag - 1], scores[best_lag], scores[best_lag + 1]);
        let denom = a - 2.0 * b + c;
        if denom.abs() > f32::EPSILON {
            let delta = 0.5 * (a - c) / denom;
            if delta.abs() <= 1.0 {
                lag += delta;
            }
        }
    }

    ((frames_per_sec * 60.0) / lag).clamp(MIN_BPM, MAX_BPM)
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

    // Beat-synchronous chroma for the whole track first: the chord sequence is
    // decoded jointly (below) rather than per beat, so a beat's label depends
    // on its neighbours.
    let mut chromas: Vec<[f32; 12]> = Vec::new();
    for w in beat_frames.windows(2) {
        let (f0, f1) = (w[0], w[1]);
        let mut chroma = [0.0_f32; 12];
        for frame in &stft[f0..f1.min(stft.len())] {
            let c = chroma_of_mag(frame, sr);
            for k in 0..12 {
                chroma[k] += c[k];
            }
        }
        chromas.push(chroma);
    }

    let decoded = smooth_chords(&chromas, 0.5);
    let mut cells = Vec::new();
    let mut labels = Vec::new();
    for (bi, w) in beat_frames.windows(2).enumerate() {
        let (chord, confidence) = decoded.get(bi).copied().unwrap_or((None, 0.0));
        labels.push(chord);
        cells.push(ChordCell {
            start_secs: frame_secs(w[0]),
            end_secs: frame_secs(w[1]),
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

    /// The estimator must never report a tempo outside the band it searches.
    /// Regression: integer-lag rounding let the 180 BPM edge become lag 14 at
    /// 43.07 fps → 184.6 BPM, and the fast-biased scoring then pinned real
    /// music to exactly that bucket.
    #[test]
    fn tempo_never_escapes_the_search_band() {
        let fps = 44_100.0 / HOP as f32;
        // Dense noise-ish onsets, a steady fast pulse, and a very slow pulse —
        // each an invitation to run off one end of the band or the other.
        let mut cases: Vec<Vec<f32>> = Vec::new();
        cases.push((0..1500).map(|i| ((i * 37) % 11) as f32 / 11.0).collect());
        for period in [7usize, 11, 14, 15, 60, 90] {
            let mut v = vec![0.0_f32; 1500];
            for i in (0..v.len()).step_by(period) {
                v[i] = 1.0;
            }
            cases.push(v);
        }
        for (n, onset) in cases.iter().enumerate() {
            let bpm = estimate_tempo(onset, fps);
            assert!(
                (MIN_BPM..=MAX_BPM).contains(&bpm),
                "case {n}: {bpm:.1} BPM is outside the {MIN_BPM}–{MAX_BPM} band"
            );
        }
    }

    /// The prior must not systematically prefer double-time. A pulse whose
    /// period is unambiguous should come back at that period, not its octave.
    #[test]
    fn tempo_prior_does_not_favour_double_time() {
        let fps = 44_100.0 / HOP as f32;
        // 30-frame period ≈ 86 BPM. Double-time would read ≈172.
        let mut onset = vec![0.0_f32; 2000];
        for i in (0..onset.len()).step_by(30) {
            onset[i] = 1.0;
        }
        let bpm = estimate_tempo(&onset, fps);
        let expected = fps / 30.0 * 60.0;
        assert!(
            (bpm - expected).abs() < 5.0,
            "expected ~{expected:.0} BPM, got {bpm:.1} (octave error?)"
        );
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

    /// Build a beat chroma for `label` with a little broadband leakage, so it
    /// resembles a real full-mix beat rather than a clean template.
    fn noisy_chroma(label: ChordLabel, leak: f32) -> [f32; 12] {
        let mut c = [leak; 12];
        for &iv in label.quality.intervals() {
            c[((label.root as u16 + iv as u16) % 12) as usize] += 1.0;
        }
        c
    }

    /// A single wobbly beat inside a held chord must not become its own chord.
    /// Regression for the over-segmentation that produced hundreds of one-beat
    /// "chords" on real material.
    #[test]
    fn smoothing_absorbs_a_single_wobbly_beat() {
        let c = ChordLabel {
            root: 0,
            quality: ChordQuality::Major,
        };
        let g = ChordLabel {
            root: 7,
            quality: ChordQuality::Major,
        };
        let mut chromas: Vec<[f32; 12]> = (0..12).map(|_| noisy_chroma(c, 0.25)).collect();
        // A *marginal* wobble, not a substitution: still a C chroma, but with
        // enough B/D leakage (a passing tone, a bass run, a cymbal) to tip the
        // per-beat argmax toward G. Real jitter looks like this — a clean
        // foreign triad for exactly one beat would be a real chord change, and
        // following that one is correct.
        chromas[6][11] += 0.9;
        chromas[6][2] += 0.9;

        // Guard: the perturbation must actually be enough to flip an
        // *unsmoothed* decision, or this test proves nothing.
        let (solo, _) = match_chord(&chromas[6], 0.5);
        assert_ne!(solo, Some(c), "perturbation too weak to exercise smoothing");

        let smoothed = smooth_chords(&chromas, 0.5);
        let changes = smoothed.windows(2).filter(|w| w[0].0 != w[1].0).count();
        assert_eq!(
            changes, 0,
            "one wobbly beat should not spawn a chord change"
        );
        assert!(smoothed.iter().all(|(l, _)| *l == Some(c)));
        let _ = g;
    }

    /// A genuine, sustained change must still come through — smoothing should
    /// resist jitter, not freeze the output.
    #[test]
    fn smoothing_still_follows_a_real_change() {
        let c = ChordLabel {
            root: 0,
            quality: ChordQuality::Major,
        };
        let g = ChordLabel {
            root: 7,
            quality: ChordQuality::Major,
        };
        let mut chromas: Vec<[f32; 12]> = (0..8).map(|_| noisy_chroma(c, 0.25)).collect();
        chromas.extend((0..8).map(|_| noisy_chroma(g, 0.25)));

        let smoothed = smooth_chords(&chromas, 0.5);
        assert_eq!(smoothed.first().unwrap().0, Some(c));
        assert_eq!(smoothed.last().unwrap().0, Some(g));
        let changes = smoothed.windows(2).filter(|w| w[0].0 != w[1].0).count();
        assert_eq!(changes, 1, "expected exactly one chord change");
    }

    /// On a dense chroma a four-note seventh out-scores the triad purely on
    /// template norm. The quality prior must stop that: a plain triad plus
    /// broadband leakage should read as the triad, not as a seventh.
    #[test]
    fn quality_prior_keeps_triads_from_becoming_sevenths() {
        let c = ChordLabel {
            root: 0,
            quality: ChordQuality::Major,
        };
        let chromas: Vec<[f32; 12]> = (0..8).map(|_| noisy_chroma(c, 0.35)).collect();
        let smoothed = smooth_chords(&chromas, 0.5);
        for (label, _) in &smoothed {
            let q = label.expect("should detect a chord").quality;
            assert_eq!(
                q,
                ChordQuality::Major,
                "dense triad chroma read as {} — the seventh templates are winning on norm",
                label.unwrap().name()
            );
        }
    }
}
