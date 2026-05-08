//! Per-track audio telemetry — pure-DSP analysis baked at first save.
//!
//! Computes a small set of objective audio features per track once,
//! offline, on a background thread, and persists the result on
//! `Track.telemetry`. The Mix-tab lanes render tag chips from these
//! features; the project-health panel summarises their weight.
//!
//! Two layers:
//!
//!   • `TrackTelemetry` — universal features (RMS, spectral centroid,
//!     spectral flatness, onset rate, sustain ratio, mood proxies).
//!     Computed for every track regardless of role.
//!
//!   • `DrumKitTelemetry` — kit-class detection (kick / snare /
//!     hi-hat / tom / cymbal / other). Computed ONLY for tracks
//!     whose `TrackSource::SunoStem.role` is `Drums` or `Percussion`
//!     (TBSS-FR-0005 §"Lifecycle" — gated on role to avoid running
//!     drum classifiers on vocals).
//!
//! Algorithm choice: multi-band parallel onset detection (Option B
//! from the design discussion). One STFT pass; per-band energy
//! curves derived from that STFT; band-specific spectral-flux
//! onset detectors fire independently. Lets us catch simultaneous
//! events — e.g. kick + hat on the same downbeat — that a single
//! global onset detector would miss.
//!
//! Cost target: ~1-3 s per 3-minute mono stem on a modern CPU.
//! Always runs offline; never on the audio thread.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Schema version. Bumped when the analyzer changes shape so old
/// telemetry can be detected as stale and re-computed on demand.
pub const ANALYZER_VERSION: u32 = 1;

/// FFT window size for spectral analysis. Power of two for rustfft.
const FFT_SIZE: usize = 2048;
/// Hop size between FFT windows. 25% hop = 75% overlap = smooth
/// onset-detection but more compute.
const FFT_HOP: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackTelemetry {
    /// Schema version of the analyzer that produced this.
    pub analyzer_version: u32,

    // ── Spectral character (track-averaged) ──
    /// Spectral centroid normalised to [0, 1]. Brightness — high
    /// values = bright timbre, low = dark.
    pub spectral_centroid_avg: f32,
    pub spectral_centroid_std: f32,
    /// Spectral flatness — 0 = pure tonal, 1 = white-noise-like.
    /// Wiener-entropy formulation.
    pub spectral_flatness_avg: f32,
    /// 85%-energy roll-off frequency, normalised to [0, 1].
    pub spectral_rolloff_avg: f32,

    // ── Dynamics ──
    pub rms_avg_db: f32,
    pub rms_std_db: f32,
    pub crest_factor_avg: f32,
    pub peak_db: f32,

    // ── Rhythmic / articulation ──
    pub onset_count: u32,
    /// Average onsets per second over the active region.
    pub onset_rate_hz: f32,
    /// Sustain ratio: 0 = staccato (energy collapses fast after each
    /// onset), 1 = sustained (continuous energy throughout).
    pub sustain_ratio: f32,

    // ── Mood proxies (derived) ──
    /// Arousal in [0, 1]: weighted sum of RMS, onset rate, centroid.
    pub arousal: f32,
    /// Valence proxy in [-1, 1]: derived from centroid + flatness.
    /// **Phase-1 stub** — proper valence needs key detection (phase 2).
    /// Currently: bright + tonal = positive, dark + noisy = negative.
    pub valence: f32,

    // ── Drum kit (only when role == Drums | Percussion) ──
    /// Kit-class detection result. None for non-drum tracks.
    pub drum_kit: Option<DrumKitTelemetry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrumKitTelemetry {
    pub kick_count: u32,
    pub snare_count: u32,
    pub hihat_count: u32,
    pub tom_count: u32,
    pub cymbal_count: u32,
    pub other_count: u32,

    /// Full event list — every detected hit. No cap. The project-
    /// health panel surfaces total bytes so users can see when it's
    /// time to compact via re-analysis with a future `cap_events`
    /// option.
    pub events: Vec<DrumEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrumEvent {
    /// Onset time in seconds from track start.
    pub time_secs: f32,
    pub class: DrumClass,
    /// Peak amplitude at the onset, normalised to [0, 1].
    pub velocity: f32,
    /// Decay duration (peak → 30 % energy) in milliseconds.
    pub decay_ms: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DrumClass {
    Kick,
    Snare,
    HiHat,
    Tom,
    Cymbal,
    Other,
}

impl DrumClass {
    /// Tiny visual marker. Reserved for the upcoming per-track
    /// telemetry detail panel (TBSS-FR-0005 §"UI" — "click chip →
    /// open detail"); surfaced here so the API stays stable.
    #[allow(dead_code)]
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Kick => "🥁",
            Self::Snare => "🥁",
            Self::HiHat => "✨",
            Self::Tom => "🪘",
            Self::Cymbal => "🔔",
            Self::Other => "·",
        }
    }
    /// Lower-case singular label. Same reservation as `glyph` above
    /// (per-track detail panel will use this for the legend).
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Self::Kick => "kick",
            Self::Snare => "snare",
            Self::HiHat => "hat",
            Self::Tom => "tom",
            Self::Cymbal => "cymbal",
            Self::Other => "other",
        }
    }
}

/// Frequency bands (Hz) used for multi-band onset detection. Tuned
/// against textbook drum-kit frequency content; thresholds picked
/// to be conservatively wide so we don't miss events at the edges.
struct FreqBands {
    sub_low: (f32, f32),  // kick fundamental
    low_mid: (f32, f32),  // tom / snare body
    mid: (f32, f32),      // snare body / mid percussion
    high_mid: (f32, f32), // snare wires / cymbal body
    high: (f32, f32),     // hi-hat / cymbal sheen
}

impl FreqBands {
    fn classic() -> Self {
        Self {
            sub_low: (40.0, 120.0),
            low_mid: (80.0, 300.0),
            mid: (200.0, 800.0),
            high_mid: (1_500.0, 5_000.0),
            high: (5_000.0, 12_000.0),
        }
    }
}

/// Run the analyzer on a WAV file. Returns the populated telemetry
/// struct or an anyhow error chain. `is_drum_stem` gates the drum-
/// kit detection so we don't run kick / snare / hat classifiers on
/// vocal content.
pub fn analyze_wav(path: &Path, is_drum_stem: bool) -> Result<TrackTelemetry> {
    let mut reader =
        hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spec = reader.spec();
    let sr = spec.sample_rate;
    let channels = spec.channels.max(1) as usize;
    let denom = i16::MAX as f32;

    // Mono-mix (mean of channels) into a single Vec<f32>. Telemetry
    // doesn't need stereo information — features are L+R averages.
    let raw: Vec<i16> = reader.samples::<i16>().filter_map(|s| s.ok()).collect();
    let frame_count = raw.len() / channels;
    let mut mono = Vec::with_capacity(frame_count);
    for f in 0..frame_count {
        let mut s = 0.0;
        for c in 0..channels {
            s += raw[f * channels + c] as f32;
        }
        mono.push(s / (channels as f32 * denom));
    }

    // ── Time-domain stats: RMS, peak, crest factor ────────────────
    let mut sum_sq = 0.0_f64;
    let mut peak = 0.0_f32;
    for &s in &mono {
        sum_sq += (s * s) as f64;
        let abs = s.abs();
        if abs > peak {
            peak = abs;
        }
    }
    let rms = ((sum_sq / mono.len().max(1) as f64).sqrt()) as f32;
    let rms_db = 20.0 * rms.max(1e-9).log10();
    let peak_db = 20.0 * peak.max(1e-9).log10();
    let crest = if rms > 1e-9 { peak / rms } else { 0.0 };

    // Short-term RMS for std calc (1024-sample windows).
    let mut short_rms_history = Vec::new();
    let win = 1024;
    for chunk in mono.chunks(win) {
        let s: f32 = chunk.iter().map(|x| x * x).sum();
        let r = (s / chunk.len() as f32).sqrt();
        let r_db = 20.0 * r.max(1e-9).log10();
        short_rms_history.push(r_db);
    }
    let rms_std_db = stddev(&short_rms_history);

    // ── STFT pass: compute spectral features + per-band energy ────
    let stft = compute_stft(&mono);

    // Spectral features per frame.
    let mut centroid_history = Vec::with_capacity(stft.len());
    let mut flatness_history = Vec::with_capacity(stft.len());
    let mut rolloff_history = Vec::with_capacity(stft.len());
    for frame in &stft {
        centroid_history.push(spectral_centroid(frame));
        flatness_history.push(spectral_flatness(frame));
        rolloff_history.push(spectral_rolloff(frame, 0.85));
    }
    let centroid_avg = mean(&centroid_history);
    let centroid_std = stddev(&centroid_history);
    let flatness_avg = mean(&flatness_history);
    let rolloff_avg = mean(&rolloff_history);

    // Per-band energy curves for onset detection.
    let bands = FreqBands::classic();
    let bin_hz = sr as f32 / FFT_SIZE as f32;
    let band_idx = |range: (f32, f32)| -> (usize, usize) {
        let lo = (range.0 / bin_hz) as usize;
        let hi = ((range.1 / bin_hz) as usize).min(stft[0].len() - 1);
        (lo.min(hi), hi)
    };

    let bands_idx = [
        band_idx(bands.sub_low),
        band_idx(bands.low_mid),
        band_idx(bands.mid),
        band_idx(bands.high_mid),
        band_idx(bands.high),
    ];
    let band_names = ["sub_low", "low_mid", "mid", "high_mid", "high"];

    // For each band, build an energy-over-time curve and a flux curve.
    let mut band_energy: Vec<Vec<f32>> = Vec::with_capacity(bands_idx.len());
    let mut band_flux: Vec<Vec<f32>> = Vec::with_capacity(bands_idx.len());
    for &(lo, hi) in &bands_idx {
        let energy: Vec<f32> = stft
            .iter()
            .map(|frame| frame[lo..=hi].iter().sum::<f32>())
            .collect();
        let flux: Vec<f32> = energy.windows(2).map(|w| (w[1] - w[0]).max(0.0)).collect();
        band_energy.push(energy);
        band_flux.push(flux);
    }

    // ── Onset detection per band ──────────────────────────────────
    // Aggregate onset count + per-band onset list (for drum
    // classification). Use adaptive threshold = median + k·MAD.
    let mut all_onset_frames: Vec<usize> = Vec::new();
    let mut per_band_onsets: Vec<Vec<usize>> = Vec::with_capacity(bands_idx.len());
    for flux in &band_flux {
        let onsets = peak_pick(flux, 3.0);
        for &f in &onsets {
            all_onset_frames.push(f);
        }
        per_band_onsets.push(onsets);
    }
    all_onset_frames.sort_unstable();
    all_onset_frames.dedup_by(|a, b| a.abs_diff(*b) < 3); // merge near-duplicates

    let total_secs = mono.len() as f32 / sr as f32;
    let onset_rate_hz = if total_secs > 0.1 {
        all_onset_frames.len() as f32 / total_secs
    } else {
        0.0
    };

    // ── Sustain ratio ────────────────────────────────────────────
    // Walk a sliding window: ratio = (active frames where energy >
    // 30% of local max) / total_frames. Higher = more sustained.
    let sustain_ratio = sustain_ratio_compute(&short_rms_history);

    // ── Drum-kit detection ───────────────────────────────────────
    let drum_kit = if is_drum_stem {
        Some(classify_drum_events(
            &per_band_onsets,
            &band_flux,
            &band_energy,
            &stft,
            &band_names,
            sr,
        ))
    } else {
        None
    };

    // ── Mood proxies ─────────────────────────────────────────────
    let arousal = arousal_proxy(rms, onset_rate_hz, centroid_avg);
    let valence = valence_proxy(centroid_avg, flatness_avg);

    Ok(TrackTelemetry {
        analyzer_version: ANALYZER_VERSION,
        spectral_centroid_avg: centroid_avg,
        spectral_centroid_std: centroid_std,
        spectral_flatness_avg: flatness_avg,
        spectral_rolloff_avg: rolloff_avg,
        rms_avg_db: rms_db,
        rms_std_db,
        crest_factor_avg: crest,
        peak_db,
        onset_count: all_onset_frames.len() as u32,
        onset_rate_hz,
        sustain_ratio,
        arousal,
        valence,
        drum_kit,
    })
}

// ───────────────────── STFT + spectral helpers ─────────────────────

fn compute_stft(mono: &[f32]) -> Vec<Vec<f32>> {
    use rustfft::{num_complex::Complex, FftPlanner};
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let half = FFT_SIZE / 2;
    let mut frames = Vec::new();
    let mut buf = vec![Complex { re: 0.0, im: 0.0 }; FFT_SIZE];
    let mut start = 0;
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| {
            let t = i as f32 / (FFT_SIZE - 1) as f32;
            0.5 - 0.5 * (std::f32::consts::TAU * t).cos()
        })
        .collect();
    while start + FFT_SIZE <= mono.len() {
        for (i, b) in buf.iter_mut().enumerate() {
            b.re = mono[start + i] * window[i];
            b.im = 0.0;
        }
        fft.process(&mut buf);
        let mags: Vec<f32> = buf[..half].iter().map(|c| c.norm()).collect();
        frames.push(mags);
        start += FFT_HOP;
    }
    frames
}

fn spectral_centroid(spectrum: &[f32]) -> f32 {
    let mut weighted = 0.0;
    let mut total = 0.0;
    for (i, &m) in spectrum.iter().enumerate() {
        weighted += i as f32 * m;
        total += m;
    }
    if total < 1e-6 {
        0.0
    } else {
        (weighted / total) / spectrum.len() as f32
    }
}

fn spectral_flatness(spectrum: &[f32]) -> f32 {
    // Geometric mean / arithmetic mean. 1 = white, 0 = tonal.
    if spectrum.is_empty() {
        return 0.0;
    }
    let mut sum_log = 0.0;
    let mut sum = 0.0;
    let mut count = 0;
    for &m in spectrum {
        let m = m.max(1e-9);
        sum_log += m.ln();
        sum += m;
        count += 1;
    }
    if count == 0 || sum < 1e-9 {
        return 0.0;
    }
    let geo_mean = (sum_log / count as f32).exp();
    let arith_mean = sum / count as f32;
    (geo_mean / arith_mean).clamp(0.0, 1.0)
}

fn spectral_rolloff(spectrum: &[f32], frac: f32) -> f32 {
    let total: f32 = spectrum.iter().sum();
    if total < 1e-6 {
        return 0.0;
    }
    let target = total * frac;
    let mut cum = 0.0;
    for (i, &m) in spectrum.iter().enumerate() {
        cum += m;
        if cum >= target {
            return i as f32 / spectrum.len() as f32;
        }
    }
    1.0
}

// ───────────────────── onset detection ─────────────────────

/// Peak-pick a flux-like curve with adaptive threshold.
/// `k_mad` tunes sensitivity — higher = fewer onsets detected.
fn peak_pick(curve: &[f32], k_mad: f32) -> Vec<usize> {
    if curve.len() < 8 {
        return Vec::new();
    }
    let med = median(curve);
    let mad = mad_around(curve, med);
    let threshold = med + k_mad * mad.max(1e-6);

    let mut onsets = Vec::new();
    let lookback = 3;
    let lookforward = 3;
    let min_separation = 5; // frames

    for i in lookback..(curve.len() - lookforward) {
        let v = curve[i];
        if v < threshold {
            continue;
        }
        // Local maximum check.
        let mut is_peak = true;
        for j in 1..=lookback {
            if curve[i - j] >= v {
                is_peak = false;
                break;
            }
        }
        if is_peak {
            for j in 1..=lookforward {
                if curve[i + j] > v {
                    is_peak = false;
                    break;
                }
            }
        }
        if is_peak {
            if let Some(&last) = onsets.last() {
                if i - last < min_separation {
                    continue;
                }
            }
            onsets.push(i);
        }
    }
    onsets
}

// ───────────────────── drum-class classification ─────────────────────

fn classify_drum_events(
    per_band_onsets: &[Vec<usize>],
    band_flux: &[Vec<f32>],
    band_energy: &[Vec<f32>],
    stft: &[Vec<f32>],
    _band_names: &[&str],
    sr: u32,
) -> DrumKitTelemetry {
    // Indices (matches FreqBands::classic order):
    const SUB: usize = 0;
    const LOW_MID: usize = 1;
    const MID: usize = 2;
    const HIGH_MID: usize = 3;
    const HIGH: usize = 4;

    let frame_secs = FFT_HOP as f32 / sr as f32;

    // Flatten + classify all per-band onsets. We classify each band's
    // events on its own to avoid the matching-merge problem; events
    // from different bands at the same frame count as separate hits
    // (kick + hat on a downbeat = 2 events).
    let mut events: Vec<DrumEvent> = Vec::new();

    let normalise_flux_peak = |flux: &[f32], frame: usize| -> f32 {
        let v = flux[frame];
        let max = flux.iter().cloned().fold(0.0_f32, f32::max);
        if max < 1e-6 {
            0.0
        } else {
            (v / max).clamp(0.0, 1.0)
        }
    };

    let decay_ms_for = |energy: &[f32], onset_frame: usize| -> f32 {
        if onset_frame >= energy.len() - 1 {
            return 0.0;
        }
        // Find peak after onset (small look-ahead).
        let look_ahead = 10.min(energy.len() - onset_frame - 1);
        let mut peak_idx = onset_frame;
        let mut peak_val = energy[onset_frame];
        for (i, &v) in energy
            .iter()
            .enumerate()
            .skip(onset_frame)
            .take(look_ahead)
        {
            if v > peak_val {
                peak_val = v;
                peak_idx = i;
            }
        }
        let target = peak_val * 0.3;
        for (i, &v) in energy.iter().enumerate().skip(peak_idx) {
            if v < target {
                return (i - peak_idx) as f32 * frame_secs * 1000.0;
            }
        }
        (energy.len() - peak_idx) as f32 * frame_secs * 1000.0
    };

    // Classify SUB-band onsets as Kick or low-Tom (harmonic test).
    for &frame in &per_band_onsets[SUB] {
        let velocity = normalise_flux_peak(&band_flux[SUB], frame);
        let decay = decay_ms_for(&band_energy[SUB], frame);
        // Harmonic test: HNR in 100ms post-onset window.
        let class = if is_harmonic_after_onset(stft, frame, sr) {
            DrumClass::Tom
        } else {
            DrumClass::Kick
        };
        events.push(DrumEvent {
            time_secs: frame as f32 * frame_secs,
            class,
            velocity,
            decay_ms: decay,
        });
    }

    // LOW_MID onsets: Tom (harmonic) or pass — most LOW_MID activity
    // is co-incident with SUB or HIGH_MID, so we only fire here for
    // events that have NO SUB onset within ±3 frames AND have a
    // discernible harmonic structure (rules out cymbal bleed).
    for &frame in &per_band_onsets[LOW_MID] {
        let near_sub = per_band_onsets[SUB].iter().any(|&f| frame.abs_diff(f) <= 3);
        if near_sub {
            continue;
        }
        if !is_harmonic_after_onset(stft, frame, sr) {
            continue;
        }
        let velocity = normalise_flux_peak(&band_flux[LOW_MID], frame);
        let decay = decay_ms_for(&band_energy[LOW_MID], frame);
        events.push(DrumEvent {
            time_secs: frame as f32 * frame_secs,
            class: DrumClass::Tom,
            velocity,
            decay_ms: decay,
        });
    }

    // MID + HIGH_MID together = Snare. Snare has body in MID and
    // wires in HIGH_MID; we fire when EITHER band detects an onset
    // and the other shows a co-incident energy bump.
    for &frame in &per_band_onsets[MID] {
        let velocity = normalise_flux_peak(&band_flux[MID], frame);
        let decay = decay_ms_for(&band_energy[MID], frame);
        events.push(DrumEvent {
            time_secs: frame as f32 * frame_secs,
            class: DrumClass::Snare,
            velocity,
            decay_ms: decay,
        });
    }
    for &frame in &per_band_onsets[HIGH_MID] {
        let near_mid = per_band_onsets[MID].iter().any(|&f| frame.abs_diff(f) <= 3);
        if near_mid {
            continue; // counted as snare via MID detector
        }
        let velocity = normalise_flux_peak(&band_flux[HIGH_MID], frame);
        let decay = decay_ms_for(&band_energy[HIGH_MID], frame);
        // Long decay + HIGH_MID dominant + no MID burst = cymbal.
        let class = if decay > 800.0 {
            DrumClass::Cymbal
        } else {
            DrumClass::Other
        };
        events.push(DrumEvent {
            time_secs: frame as f32 * frame_secs,
            class,
            velocity,
            decay_ms: decay,
        });
    }

    // HIGH onsets: Hi-hat (short decay) or Cymbal (long decay).
    for &frame in &per_band_onsets[HIGH] {
        let velocity = normalise_flux_peak(&band_flux[HIGH], frame);
        let decay = decay_ms_for(&band_energy[HIGH], frame);
        let class = if decay > 800.0 {
            DrumClass::Cymbal
        } else {
            DrumClass::HiHat
        };
        events.push(DrumEvent {
            time_secs: frame as f32 * frame_secs,
            class,
            velocity,
            decay_ms: decay,
        });
    }

    // Sort by time (we appended per-band; merge for clean ordering).
    events.sort_by(|a, b| {
        a.time_secs
            .partial_cmp(&b.time_secs)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut kit = DrumKitTelemetry {
        kick_count: 0,
        snare_count: 0,
        hihat_count: 0,
        tom_count: 0,
        cymbal_count: 0,
        other_count: 0,
        events,
    };
    for ev in &kit.events {
        match ev.class {
            DrumClass::Kick => kit.kick_count += 1,
            DrumClass::Snare => kit.snare_count += 1,
            DrumClass::HiHat => kit.hihat_count += 1,
            DrumClass::Tom => kit.tom_count += 1,
            DrumClass::Cymbal => kit.cymbal_count += 1,
            DrumClass::Other => kit.other_count += 1,
        }
    }
    kit
}

/// Check whether an event has a clear harmonic structure (suggesting
/// a pitched tom / kick with strong fundamental) vs broadband noise
/// (suggesting a cymbal smear or unpitched percussion).
///
/// Implementation: examine STFT frames just after the onset; if the
/// spectrum has a sharp peak (high HNR), it's harmonic.
fn is_harmonic_after_onset(stft: &[Vec<f32>], onset_frame: usize, _sr: u32) -> bool {
    // Look 3-5 frames after onset for a stable spectrum.
    let start = onset_frame + 2;
    if start >= stft.len() {
        return false;
    }
    let end = (start + 3).min(stft.len());
    // Average the magnitudes over this window.
    let mut avg_spec = vec![0.0_f32; stft[0].len()];
    let mut count = 0;
    for frame in &stft[start..end] {
        for (a, &m) in avg_spec.iter_mut().zip(frame.iter()) {
            *a += m;
        }
        count += 1;
    }
    if count == 0 {
        return false;
    }
    for a in &mut avg_spec {
        *a /= count as f32;
    }
    // Crude HNR: peak / mean. Pitched content has peak ≫ mean.
    let mean = avg_spec.iter().sum::<f32>() / avg_spec.len() as f32;
    let peak = avg_spec.iter().cloned().fold(0.0_f32, f32::max);
    if mean < 1e-9 {
        return false;
    }
    (peak / mean) > 15.0
}

// ───────────────────── proxies ─────────────────────

fn arousal_proxy(rms: f32, onset_rate_hz: f32, centroid: f32) -> f32 {
    // Weighted blend; clamps to [0, 1].
    let r = (rms.clamp(0.0, 0.5) / 0.5).powf(0.5);
    let o = (onset_rate_hz / 8.0).clamp(0.0, 1.0);
    let c = centroid.clamp(0.0, 1.0);
    (0.4 * r + 0.4 * o + 0.2 * c).clamp(0.0, 1.0)
}

fn valence_proxy(centroid: f32, flatness: f32) -> f32 {
    // Bright + tonal → +0.5; dark + noisy → -0.5.
    // Scale to [-1, 1].
    let bright_minus_dark = (centroid - 0.5) * 2.0;
    let tonal_minus_noisy = (1.0 - flatness - 0.5) * 2.0;
    (0.6 * bright_minus_dark + 0.4 * tonal_minus_noisy).clamp(-1.0, 1.0)
}

// ───────────────────── tiny stats ─────────────────────

fn mean(s: &[f32]) -> f32 {
    if s.is_empty() {
        0.0
    } else {
        s.iter().sum::<f32>() / s.len() as f32
    }
}

fn stddev(s: &[f32]) -> f32 {
    if s.len() < 2 {
        return 0.0;
    }
    let m = mean(s);
    let var = s.iter().map(|x| (x - m).powi(2)).sum::<f32>() / s.len() as f32;
    var.sqrt()
}

fn median(s: &[f32]) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<f32> = s.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sorted[sorted.len() / 2]
}

fn mad_around(s: &[f32], centre: f32) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    let mut deviations: Vec<f32> = s.iter().map(|x| (x - centre).abs()).collect();
    deviations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    deviations[deviations.len() / 2]
}

fn sustain_ratio_compute(rms_history_db: &[f32]) -> f32 {
    if rms_history_db.is_empty() {
        return 0.0;
    }
    let max = rms_history_db.iter().cloned().fold(f32::MIN, f32::max);
    let threshold = max - 10.0; // 10 dB below max = "active"
    let active = rms_history_db.iter().filter(|&&r| r > threshold).count();
    active as f32 / rms_history_db.len() as f32
}

// ───────────────────── async service ─────────────────────
//
// One worker thread drains a request queue, runs `analyze_wav` per
// request, and ships results back to the UI thread via an mpsc
// channel. The UI thread `apply_pending()` drains results each frame,
// patches them onto the matching `Track` in `app.project`, and saves
// the manifest incrementally.
//
// `project_root` is included in both request and result so that if
// the user switches projects mid-analysis the UI thread can drop
// stale results silently. Each result also carries the absolute path
// of the WAV — used to verify the track still points at that file
// (Trim writes a fresh WAV, so the path may have changed).

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

#[derive(Debug, Clone)]
pub struct TelemetryRequest {
    pub project_root: PathBuf,
    pub track_id: String,
    pub abs_path: PathBuf,
    pub is_drum_stem: bool,
}

#[derive(Debug)]
pub struct TelemetryResult {
    pub project_root: PathBuf,
    pub track_id: String,
    pub abs_path: PathBuf,
    /// `Ok` = analysis succeeded; `Err` = error string for the status bar.
    pub outcome: Result<TrackTelemetry, String>,
}

/// Background worker that owns one OS thread. Drop the service to
/// stop the thread (the request channel closes; the worker exits its
/// `recv()` loop).
pub struct TelemetryService {
    req_tx: mpsc::Sender<TelemetryRequest>,
    res_rx: mpsc::Receiver<TelemetryResult>,
    /// Number of requests dispatched but not yet drained as results.
    /// Used by the UI to surface "Analyzing N/M..." in the status bar.
    pending: usize,
    total_dispatched: usize,
}

impl TelemetryService {
    pub fn spawn() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<TelemetryRequest>();
        let (res_tx, res_rx) = mpsc::channel::<TelemetryResult>();
        thread::Builder::new()
            .name("tbss-telemetry".into())
            .spawn(move || {
                while let Ok(req) = req_rx.recv() {
                    let outcome =
                        analyze_wav(&req.abs_path, req.is_drum_stem).map_err(|e| format!("{e:#}"));
                    let result = TelemetryResult {
                        project_root: req.project_root,
                        track_id: req.track_id,
                        abs_path: req.abs_path,
                        outcome,
                    };
                    if res_tx.send(result).is_err() {
                        // UI side is gone — exit cleanly.
                        break;
                    }
                }
            })
            .expect("spawning telemetry worker thread");

        Self {
            req_tx,
            res_rx,
            pending: 0,
            total_dispatched: 0,
        }
    }

    /// Queue an analysis. Silently drops if the worker thread has
    /// died (we'd rather degrade than crash on a background fault).
    pub fn dispatch(&mut self, req: TelemetryRequest) {
        if self.req_tx.send(req).is_ok() {
            self.pending += 1;
            self.total_dispatched += 1;
        }
    }

    /// Drain every result that's arrived since the last call. The
    /// caller applies them to its project state. Decrements `pending`
    /// per result drained.
    pub fn drain(&mut self) -> Vec<TelemetryResult> {
        let mut out = Vec::new();
        while let Ok(r) = self.res_rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            out.push(r);
        }
        if self.pending == 0 {
            // Reset the running total once everything's drained so
            // the next batch starts clean for status-bar purposes.
            self.total_dispatched = 0;
        }
        out
    }

    /// True when there's any analysis in flight.
    pub fn has_pending(&self) -> bool {
        self.pending > 0
    }

    /// Returns `(done, total)` if a batch is in flight, else None.
    pub fn progress(&self) -> Option<(usize, usize)> {
        if self.total_dispatched == 0 {
            return None;
        }
        let done = self.total_dispatched - self.pending;
        Some((done, self.total_dispatched))
    }
}

impl Default for TelemetryService {
    fn default() -> Self {
        Self::spawn()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_synthetic(samples: Vec<f32>) -> Vec<i16> {
        samples
            .iter()
            .map(|&s| (s * i16::MAX as f32) as i16)
            .collect()
    }

    fn write_synthetic_wav(path: &Path, samples: Vec<f32>, sample_rate: u32) -> Result<()> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec)?;
        for &s in make_synthetic(samples).iter() {
            writer.write_sample(s)?;
        }
        writer.finalize()?;
        Ok(())
    }

    #[test]
    fn analyzer_handles_silence_without_panic() {
        let dir = std::env::temp_dir();
        let path = dir.join("tbss_test_silence.wav");
        write_synthetic_wav(&path, vec![0.0; 48_000], 48_000).unwrap();
        let t = analyze_wav(&path, false).unwrap();
        assert_eq!(t.analyzer_version, ANALYZER_VERSION);
        assert!(t.peak_db < -60.0);
        assert_eq!(t.onset_count, 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn analyzer_detects_pure_tone_brightness() {
        // 4 kHz sine for 1 second at 0.5 amplitude.
        let sr = 48_000;
        let f = 4_000.0;
        let samples: Vec<f32> = (0..sr)
            .map(|i| {
                let t = i as f32 / sr as f32;
                0.5 * (std::f32::consts::TAU * f * t).sin()
            })
            .collect();
        let dir = std::env::temp_dir();
        let path = dir.join("tbss_test_tone.wav");
        write_synthetic_wav(&path, samples, sr).unwrap();
        let t = analyze_wav(&path, false).unwrap();
        // 4kHz on a 24kHz Nyquist → centroid > 0.05 (4/24).
        assert!(t.spectral_centroid_avg > 0.05);
        // Pure tone → low flatness.
        assert!(
            t.spectral_flatness_avg < 0.3,
            "got flatness {}",
            t.spectral_flatness_avg
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn analyzer_detects_transients() {
        // 5 transient pulses spaced 100 ms apart over 1 second.
        let sr = 48_000;
        let mut samples = vec![0.0_f32; sr as usize];
        for n in 0..5 {
            let start = n * (sr as usize / 10);
            // Short noise burst (10 ms).
            for i in 0..(sr as usize / 100) {
                if start + i < samples.len() {
                    samples[start + i] = 0.5 * ((i * 13 % 7) as f32 / 7.0 - 0.5) * 2.0;
                }
            }
        }
        let dir = std::env::temp_dir();
        let path = dir.join("tbss_test_pulses.wav");
        write_synthetic_wav(&path, samples, sr).unwrap();
        let t = analyze_wav(&path, false).unwrap();
        // Should detect at least 3 of the 5 pulses (some may be merged).
        assert!(
            t.onset_count >= 3,
            "expected ≥3 onsets, got {}",
            t.onset_count
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn arousal_proxy_increases_with_loudness() {
        let calm = arousal_proxy(0.05, 0.5, 0.2);
        let energetic = arousal_proxy(0.4, 6.0, 0.7);
        assert!(energetic > calm);
        assert!((0.0..=1.0).contains(&calm));
        assert!((0.0..=1.0).contains(&energetic));
    }

    #[test]
    fn valence_proxy_clamps_within_range() {
        for c in [0.0, 0.25, 0.5, 0.75, 1.0] {
            for f in [0.0, 0.5, 1.0] {
                let v = valence_proxy(c, f);
                assert!((-1.0..=1.0).contains(&v), "c={} f={} v={}", c, f, v);
            }
        }
    }

    #[test]
    fn drum_class_glyphs_are_non_empty() {
        for c in [
            DrumClass::Kick,
            DrumClass::Snare,
            DrumClass::HiHat,
            DrumClass::Tom,
            DrumClass::Cymbal,
            DrumClass::Other,
        ] {
            assert!(!c.glyph().is_empty());
            assert!(!c.label().is_empty());
        }
    }
}
