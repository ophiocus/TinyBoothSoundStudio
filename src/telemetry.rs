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
///
/// History:
///   v1 (0.4.13): universal features + drum-kit detection.
///   v2 (0.4.14): + guitar/bass pick analyzer (YIN), key estimation
///                  (Krumhansl-Schmuckler), user-selectable profile.
///   v3 (0.4.17): drum classifier dedupes across bands with dominant-
///                  flux arbitration. Pre-v3 manifests over-counted
///                  drum events ~3-6× (one snare hit produced separate
///                  Snare + Cymbal + HiHat events).
///   v4 (0.4.35): + cross_band_coherence — mean pairwise Pearson
///                  correlation of octave-band energy envelopes. The
///                  AI-audio fingerprint diagnostic from
///                  docs/sound-vision-philosophy.md §V. Natural
///                  recordings ≈ 0.6–0.9, AI-generated ≈ 0.2–0.5.
pub const ANALYZER_VERSION: u32 = 4;

/// FFT window size for spectral analysis. Power of two for rustfft.
const FFT_SIZE: usize = 2048;
/// Hop size between FFT windows. 25% hop = 75% overlap = smooth
/// onset-detection but more compute.
const FFT_HOP: usize = 512;

/// User-tweakable analyzer thresholds. Persisted to
/// `%APPDATA%\TinyBooth Sound Studio\telemetry_settings.json`.
/// Defaults are picked to be conservative on Suno output (the only
/// audio shape we know we'll always see). Surfaced via Admin →
/// Telemetry settings…. v0.4.14.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TelemetrySettings {
    /// k·MAD multiplier for the spectral-flux peak picker (drum +
    /// guitar onsets share this). Higher = fewer onsets detected.
    /// Default 3.0.
    pub drum_onset_k_mad: f32,
    /// Minimum velocity (peak amplitude in [0, 1] at the onset
    /// frame) for a guitar onset to count as a pick. Below this
    /// it's classified as Noise. Default 0.05.
    pub guitar_pick_threshold: f32,
    /// Same shape for bass — usually a touch lower because basses
    /// pluck quieter relative to peak. Default 0.04.
    pub bass_pick_threshold: f32,
    /// YIN cumulative-mean-difference threshold. Lags whose value
    /// drops below this are considered confident pitch picks.
    /// 0.10–0.20 is the standard range; lower = stricter. Default 0.15.
    pub yin_threshold: f32,
    /// Polyphony cutoff — events whose post-onset window has more
    /// than this many spectral peaks above –12 dB get classified
    /// as Strum (no pitch reported). Default 5.
    pub polyphony_peak_count: usize,
    /// Cents tolerance for "same pitch" classification (Pluck vs.
    /// Repeat). Default 50 (a quarter-tone). Bigger = more events
    /// classified as Repeat.
    pub same_pitch_cents: f32,
}

impl Default for TelemetrySettings {
    fn default() -> Self {
        Self {
            drum_onset_k_mad: 3.0,
            guitar_pick_threshold: 0.05,
            bass_pick_threshold: 0.04,
            yin_threshold: 0.15,
            polyphony_peak_count: 5,
            same_pitch_cents: 50.0,
        }
    }
}

impl TelemetrySettings {
    fn config_path() -> Option<std::path::PathBuf> {
        crate::config::Config::dir().map(|d| d.join("telemetry_settings.json"))
    }

    /// Load from disk; on first run / parse error fall back to
    /// defaults silently.
    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        let Ok(s) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&s).unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let Some(path) = Self::config_path() else {
            anyhow::bail!("no platform config dir");
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)?;
        Ok(())
    }
}

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

    /// Pick-stroke detection result with YIN pitch tracking.
    /// Populated for tracks whose resolved profile is `Guitar` or
    /// `Bass`. None otherwise. Added v0.4.14.
    #[serde(default)]
    pub guitar: Option<GuitarTelemetry>,

    /// Per-track Krumhansl-Schmuckler key estimate. Populated when
    /// pitch data is present (Guitar / Bass profiles). Added v0.4.14.
    #[serde(default)]
    pub key_estimate: Option<KeyEstimate>,

    /// **Cross-band coherence** — mean pairwise Pearson correlation
    /// of octave-band energy envelopes. The AI-audio fingerprint
    /// diagnostic from `docs/sound-vision-philosophy.md` §V.
    ///
    /// Physical instruments and natural recordings have correlated
    /// bands — when a string vibrates or a vocal cord opens, every
    /// frequency band shares the same low-frequency modulation
    /// envelope (the bands "move together"). Score typically 0.6–0.9.
    ///
    /// AI-generated audio has band-decorrelated micro-fluctuations:
    /// each band is generated semi-independently by the model and
    /// wobbles out of phase with the others. Score typically 0.2–0.5.
    ///
    /// In `[-1, 1]` mathematically; in practice `[0, 1]` for music
    /// content. Computed once at first save from the existing STFT
    /// pass — no extra audio I/O. Added v0.4.35 (telemetry v4).
    ///
    /// Phase 3 (TBD): a "Coherence Restoration" post-processing
    /// filter that re-correlates the bands by gating their modulation
    /// envelopes against a shared reference envelope. Goal: take AI
    /// output meaningfully closer to "sounds like a recording".
    #[serde(default)]
    pub cross_band_coherence: f32,
}

/// Pick-stroke / pitch telemetry for guitar / bass content. See
/// TBSS-FR-0005 §"Phase 2 (Guitar)" and the v0.4.14 design discussion
/// — pick events come from spectral-flux onsets, each event is
/// classified via a YIN pitch read in a 50-150 ms window post-onset
/// plus a polyphony probe. Strums become single events with no pitch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuitarTelemetry {
    /// Total picks detected (every kind — including Strum and Repeat).
    pub pick_count: u32,
    /// Picks where the dominant pitch matched the previous event
    /// (within ±50 cents). E.g. tremolo picking on a single fret.
    pub repeated_pick_count: u32,
    /// Picks whose pitch differed from the previous event by more
    /// than ±50 cents (a fret / string change). Strums don't count.
    pub pitch_change_count: u32,
    /// Picks classified as Slide / Bend (smooth pitch trajectory
    /// between two events with no intervening pluck transient).
    pub bend_or_slide_count: u32,
    /// Picks classified as Strum (polyphonic onset; no usable pitch).
    pub strum_count: u32,
    /// Estimated polyphony in [0, 1]. Mean of per-event polyphony
    /// scores. 0 ≈ pure monophonic line, 1 ≈ heavily strummed
    /// chordal content.
    pub estimated_polyphony: f32,
    /// Full per-event list. No cap (TBSS-FR-0005 §"Infinity"). The
    /// Project Health panel surfaces total bytes; users can see when
    /// to compact.
    pub events: Vec<GuitarEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuitarEvent {
    /// Onset time in seconds from track start.
    pub time_secs: f32,
    /// Detected fundamental in Hz, or `None` when YIN gave up
    /// (polyphonic moment / noise-only window). Persist raw Hz —
    /// cents-off-pitch and bend detection are free post-processing.
    pub pitch_hz: Option<f32>,
    /// YIN's normalised difference at the chosen lag. Lower = more
    /// confident; values ≤ 0.15 are typically reliable. Surfaced so
    /// downstream passes can filter without re-running the analyzer.
    pub confidence: f32,
    /// Peak amplitude at the onset in [0, 1]. Doubles as a "velocity"
    /// surrogate.
    pub velocity: f32,
    /// Decay duration (peak → 30 % energy) in milliseconds.
    pub decay_ms: f32,
    pub kind: PickKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PickKind {
    /// Single-string monophonic pick at a new pitch.
    Pluck,
    /// Same pitch as the previous event (within ±50 cents).
    Repeat,
    /// Polyphonic onset — likely a strummed chord. No pitch.
    Strum,
    /// Smooth pitch trajectory continuing from the previous event
    /// (slide or bend, not a fresh pluck).
    Slide,
    /// Onset detected but no usable pitch and not polyphonic — fret
    /// noise / fingering / breath / etc.
    Noise,
}

impl PickKind {
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Self::Pluck => "pluck",
            Self::Repeat => "repeat",
            Self::Strum => "strum",
            Self::Slide => "slide",
            Self::Noise => "noise",
        }
    }
}

/// Krumhansl-Schmuckler key estimate. Computed from the pitch-class
/// histogram of every `GuitarEvent.pitch_hz` (when present), weighted
/// by event velocity × duration-until-next-event.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct KeyEstimate {
    /// 0 = C, 1 = C♯/D♭, …, 11 = B.
    pub root: u8,
    pub mode: KeyMode,
    /// Pearson correlation against the winning K-S template. Higher
    /// = stronger key feeling. Below ~0.5 is "key is ambiguous".
    pub confidence: f32,
    /// Second-place key — useful when `confidence` is close, e.g.
    /// minor-mode tracks often correlate with their relative major.
    pub second_choice_root: u8,
    pub second_choice_mode: KeyMode,
    pub second_choice_confidence: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum KeyMode {
    Major,
    Minor,
}

impl KeyMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Major => "maj",
            Self::Minor => "min",
        }
    }
}

impl KeyEstimate {
    /// Human-readable label like "G♯ min" (12-tone naming, sharps).
    pub fn label(&self) -> String {
        const NOTES: [&str; 12] = [
            "C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B",
        ];
        format!("{} {}", NOTES[(self.root as usize) % 12], self.mode.label())
    }
}

/// User-selectable analyzer profile per track. `Auto` resolves at
/// dispatch time from `TrackSource` (drums → drum kit, guitar/bass →
/// guitar pitch analyzer, everything else → universal-only). Explicit
/// values override the auto-resolution — useful when Suno mislabels
/// a stem (e.g. a percussive synth pad) or when a recorded take has
/// no role at all. v0.4.14.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TelemetryProfile {
    /// Infer from `TrackSource`. Default for back-compat with v0.4.13
    /// manifests (this is the only profile they ever knew).
    #[default]
    Auto,
    /// Universal features only — no instrument-specific layer.
    UniversalOnly,
    Drums,
    Guitar,
    Bass,
    /// Skip telemetry entirely. Useful for room tone, count-ins,
    /// silence stems, anything not worth analyzing.
    None,
}

impl TelemetryProfile {
    /// Resolve to a concrete analyzer profile given the track source.
    /// `Auto` reads `StemRole`; explicit values pass through.
    pub fn resolve(self, source: &crate::project::TrackSource) -> ResolvedProfile {
        use crate::project::{StemRole, TrackSource};
        match self {
            Self::None => ResolvedProfile::None,
            Self::UniversalOnly => ResolvedProfile::UniversalOnly,
            Self::Drums => ResolvedProfile::Drums,
            Self::Guitar => ResolvedProfile::Guitar,
            Self::Bass => ResolvedProfile::Bass,
            Self::Auto => match source {
                TrackSource::SunoStem { role, .. } => match role {
                    StemRole::Drums | StemRole::Percussion => ResolvedProfile::Drums,
                    StemRole::ElectricGuitar | StemRole::AcousticGuitar => ResolvedProfile::Guitar,
                    StemRole::Bass => ResolvedProfile::Bass,
                    _ => ResolvedProfile::UniversalOnly,
                },
                TrackSource::Recorded => ResolvedProfile::UniversalOnly,
            },
        }
    }

    /// All variants in stable order — for the dropdown.
    pub fn all() -> &'static [TelemetryProfile] {
        &[
            Self::Auto,
            Self::UniversalOnly,
            Self::Drums,
            Self::Guitar,
            Self::Bass,
            Self::None,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::UniversalOnly => "Universal only",
            Self::Drums => "Drums",
            Self::Guitar => "Guitar",
            Self::Bass => "Bass",
            Self::None => "Off",
        }
    }
}

/// Concrete (non-Auto) analyzer profile — what `analyze_wav` actually
/// dispatches on. Internal to the telemetry pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedProfile {
    None,
    UniversalOnly,
    Drums,
    Guitar,
    Bass,
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

/// Run the analyzer on a WAV file with the user-selected profile.
/// Returns the populated telemetry struct or an anyhow error chain.
/// `profile` is the already-resolved profile (Auto resolution is
/// done by the caller via `TelemetryProfile::resolve`). The
/// `settings` controls thresholds — pick velocity, YIN tolerance,
/// polyphony cutoff. v0.4.14.
pub fn analyze_wav(
    path: &Path,
    profile: ResolvedProfile,
    settings: &TelemetrySettings,
) -> Result<TrackTelemetry> {
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
        let onsets = peak_pick(flux, settings.drum_onset_k_mad);
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

    // ── Profile-gated instrument layers ──────────────────────────
    let drum_kit = if profile == ResolvedProfile::Drums {
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

    let (guitar, key_estimate) =
        if profile == ResolvedProfile::Guitar || profile == ResolvedProfile::Bass {
            let pick_threshold = if profile == ResolvedProfile::Bass {
                settings.bass_pick_threshold
            } else {
                settings.guitar_pick_threshold
            };
            let lo_hz = if profile == ResolvedProfile::Bass {
                30.0
            } else {
                70.0
            };
            let hi_hz = if profile == ResolvedProfile::Bass {
                500.0
            } else {
                1_400.0
            };
            let g = analyze_guitar_picks(
                &mono,
                sr,
                &per_band_onsets,
                &band_flux,
                &band_energy,
                pick_threshold,
                lo_hz,
                hi_hz,
                settings,
            );
            let k = estimate_key_from_events(&g.events);
            (Some(g), k)
        } else {
            (None, None)
        };

    // ── Mood proxies ─────────────────────────────────────────────
    let arousal = arousal_proxy(rms, onset_rate_hz, centroid_avg);
    let valence = valence_proxy(centroid_avg, flatness_avg);

    // ── Cross-band coherence (AI-audio fingerprint) ──────────────
    // Cheap — reuses the STFT we already have; one extra pass over
    // 8 octave-spaced bands × N frames + 28 Pearson correlations.
    let cross_band_coherence = compute_cross_band_coherence(&stft, sr);

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
        guitar,
        key_estimate,
        cross_band_coherence,
    })
}

/// Compute the cross-band coherence score for an STFT.
///
/// Algorithm:
/// 1. Pick 8 octave-spaced centres: 60, 120, 240, 480, 960, 1920,
///    3840, 7680 Hz. For each, sum FFT bin magnitudes in a 1/3-octave
///    window around the centre → per-band energy(t) curve.
/// 2. Normalise each curve to zero-mean, unit-variance (so absolute
///    loudness doesn't drown the modulation pattern we care about).
/// 3. Light EMA smoothing (~10 Hz cutoff at the STFT hop rate)
///    to focus on slow modulation envelopes — the timescale where
///    real-instrument bands move together.
/// 4. Compute pairwise Pearson correlations across all 28 band
///    pairs and return the mean.
///
/// Output range mathematically `[-1, 1]`; in practice `[0, 1]` for
/// music content. Pure silence / DC returns 0. Returns 0 on
/// degenerate input (too few frames, all-zero spectrum).
pub fn compute_cross_band_coherence(stft: &[Vec<f32>], sr: u32) -> f32 {
    if stft.len() < 16 || stft[0].is_empty() {
        return 0.0;
    }
    let bin_hz = sr as f32 / FFT_SIZE as f32;
    let half = stft[0].len();
    // Octave-spaced centres. 60 Hz floors at sub-bass; 7680 Hz tops
    // out below the typical 24-kHz Nyquist with headroom.
    const CENTRES_HZ: [f32; 8] = [60.0, 120.0, 240.0, 480.0, 960.0, 1920.0, 3840.0, 7680.0];
    // 1/3-octave window: lower = centre / 2^(1/6), upper = centre * 2^(1/6).
    const THIRD_OCT_RATIO: f32 = 1.122_462; // 2^(1/6)

    // Build per-band energy curves.
    let mut bands: Vec<Vec<f32>> = Vec::with_capacity(CENTRES_HZ.len());
    for &centre in &CENTRES_HZ {
        let lo = (centre / THIRD_OCT_RATIO / bin_hz).floor() as usize;
        let hi = ((centre * THIRD_OCT_RATIO / bin_hz).ceil() as usize).min(half - 1);
        if hi <= lo {
            bands.push(vec![0.0; stft.len()]);
            continue;
        }
        let curve: Vec<f32> = stft
            .iter()
            .map(|frame| frame[lo..=hi].iter().sum::<f32>())
            .collect();
        bands.push(curve);
    }

    // Light EMA smoothing to focus on slow modulation. STFT frame
    // rate ≈ sr / FFT_HOP (≈ 94 Hz at 48k); α = 0.2 gives a ~3-frame
    // time constant, attenuating modulation > ~10 Hz.
    for band in bands.iter_mut() {
        if band.is_empty() {
            continue;
        }
        let mut prev = band[0];
        for v in band.iter_mut() {
            let smoothed = 0.2 * (*v) + 0.8 * prev;
            *v = smoothed;
            prev = smoothed;
        }
    }

    // Zero-mean, unit-variance normalise each band.
    for band in bands.iter_mut() {
        let m = mean(band);
        let s = stddev(band).max(1e-9);
        for v in band.iter_mut() {
            *v = (*v - m) / s;
        }
    }

    // Mean pairwise Pearson correlation across all 28 pairs.
    // (Bands are already z-scored, so Pearson reduces to mean(b1·b2).)
    let n = bands.len();
    let mut sum = 0.0_f64;
    let mut pairs = 0_u32;
    for i in 0..n {
        for j in (i + 1)..n {
            let b1 = &bands[i];
            let b2 = &bands[j];
            if b1.is_empty() || b2.is_empty() {
                continue;
            }
            let len = b1.len().min(b2.len());
            if len < 2 {
                continue;
            }
            let r: f32 = b1
                .iter()
                .zip(b2.iter())
                .take(len)
                .map(|(a, b)| a * b)
                .sum::<f32>()
                / len as f32;
            if r.is_finite() {
                sum += r as f64;
                pairs += 1;
            }
        }
    }
    if pairs == 0 {
        0.0
    } else {
        (sum / pairs as f64).clamp(-1.0, 1.0) as f32
    }
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

    // Pre-compute each band's flux max — used to derive a per-event
    // normalised "velocity" in [0, 1] for the persisted DrumEvent
    // (the existing chip strip + project-health panel expect that
    // shape; we don't want to break the visual scale).
    let band_flux_max: Vec<f32> = band_flux
        .iter()
        .map(|f| f.iter().cloned().fold(0.0_f32, f32::max).max(1e-9))
        .collect();

    // Flatten every (frame, band, raw-flux) onset into one list. The
    // raw flux is what arbitrates the dominant band within a cluster:
    // for a single physical hit landing in multiple bands (snare →
    // MID + HIGH_MID + HIGH), the band with the largest *absolute*
    // energy rise is the one that physically owns the event. Comparing
    // raw flux works because all candidates inside one cluster come
    // from the same source event — cross-cluster scale differences
    // don't matter (clusters are picked one at a time, never against
    // each other).
    struct Candidate {
        frame: usize,
        band: usize,
        raw_flux: f32,
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    for (band, onsets) in per_band_onsets.iter().enumerate() {
        for &frame in onsets {
            let raw = band_flux[band].get(frame).copied().unwrap_or(0.0);
            candidates.push(Candidate {
                frame,
                band,
                raw_flux: raw,
            });
        }
    }
    candidates.sort_unstable_by_key(|c| c.frame);

    // Cluster candidates within ±3 frames of the previous cluster
    // member (sliding-window collapse, same shape as the universal
    // `all_onset_frames` dedup in `analyze_wav` but slightly looser
    // — `<= 3` instead of `< 3` — because cross-band peak pickers
    // can disagree on the exact frame by a hop or two even for a
    // single physical event). A snare hit produces flux peaks in
    // MID + HIGH_MID + HIGH within a frame or two; without clustering
    // we'd emit one Snare + one Cymbal + one HiHat for a single
    // physical hit (the v0.4.13–16 over-counting bug).
    let mut clusters: Vec<Vec<Candidate>> = Vec::new();
    let mut current: Vec<Candidate> = Vec::new();
    for c in candidates {
        let join = current
            .last()
            .map(|last| c.frame.abs_diff(last.frame) <= 3)
            .unwrap_or(false);
        if join {
            current.push(c);
        } else {
            if !current.is_empty() {
                clusters.push(std::mem::take(&mut current));
            }
            current.push(c);
        }
    }
    if !current.is_empty() {
        clusters.push(current);
    }

    let decay_ms_for = |energy: &[f32], onset_frame: usize| -> f32 {
        if onset_frame >= energy.len().saturating_sub(1) {
            return 0.0;
        }
        let look_ahead = 10.min(energy.len() - onset_frame - 1);
        let mut peak_idx = onset_frame;
        let mut peak_val = energy[onset_frame];
        for (i, &v) in energy.iter().enumerate().skip(onset_frame).take(look_ahead) {
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

    // Per cluster: the dominant band (largest absolute flux at the
    // cluster's frame) wins and decides the class. One event per
    // cluster, never more — that's the whole point of clustering.
    let mut events: Vec<DrumEvent> = Vec::with_capacity(clusters.len());
    for cluster in &clusters {
        let winner = cluster
            .iter()
            .max_by(|a, b| {
                a.raw_flux
                    .partial_cmp(&b.raw_flux)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("cluster is non-empty by construction");
        let frame = winner.frame;
        let band = winner.band;
        // Velocity: winner's flux normalised against its band's all-
        // time max. Same shape as the pre-v3 `normalise_flux_peak`
        // output (in [0, 1]) — preserves chip-strip / health-panel scale.
        let velocity = (winner.raw_flux / band_flux_max[band]).clamp(0.0, 1.0);
        let decay = decay_ms_for(&band_energy[band], frame);
        let class = match band {
            SUB => {
                // Harmonic test: HNR in 100ms post-onset window.
                if is_harmonic_after_onset(stft, frame, sr) {
                    DrumClass::Tom
                } else {
                    DrumClass::Kick
                }
            }
            LOW_MID => {
                // Pre-v3 dropped non-harmonic LOW_MID as cymbal bleed,
                // but post-clustering a LOW_MID-dominant cluster is by
                // definition not bleed (would have been beaten by
                // HIGH_MID/HIGH). Keep the harmonic→Tom rule, fall to
                // Other for the rest rather than discarding the event.
                if is_harmonic_after_onset(stft, frame, sr) {
                    DrumClass::Tom
                } else {
                    DrumClass::Other
                }
            }
            MID => DrumClass::Snare,
            HIGH_MID => {
                if decay > 800.0 {
                    DrumClass::Cymbal
                } else {
                    DrumClass::Other
                }
            }
            HIGH => {
                if decay > 800.0 {
                    DrumClass::Cymbal
                } else {
                    DrumClass::HiHat
                }
            }
            _ => DrumClass::Other,
        };
        events.push(DrumEvent {
            time_secs: frame as f32 * frame_secs,
            class,
            velocity,
            decay_ms: decay,
        });
    }

    // Defensive sort — clusters were already sorted by their first
    // frame, but the dominant winner can sit at frame+1 or +2.
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

// ───────────────────── guitar / bass pitch analyzer ─────────────────────

/// Pick-stroke detector: each spectral-flux onset in the MID +
/// HIGH_MID bands becomes a candidate event; the post-onset window
/// is fed to YIN for pitch + to a polyphony probe; the result is
/// classified into Pluck / Repeat / Strum / Slide / Noise.
#[allow(clippy::too_many_arguments)]
fn analyze_guitar_picks(
    mono: &[f32],
    sr: u32,
    per_band_onsets: &[Vec<usize>],
    _band_flux: &[Vec<f32>],
    band_energy: &[Vec<f32>],
    pick_threshold: f32,
    pitch_lo_hz: f32,
    pitch_hi_hz: f32,
    settings: &TelemetrySettings,
) -> GuitarTelemetry {
    const MID: usize = 2;
    const HIGH_MID: usize = 3;

    let frame_secs = FFT_HOP as f32 / sr as f32;
    let sr_f = sr as f32;

    // Merge MID + HIGH_MID onsets — the bands where pick attacks
    // live. Dedup near-duplicates within 3 frames so we don't fire
    // twice on a single pluck whose energy spans both bands.
    let mut onset_frames: Vec<usize> = Vec::new();
    for &f in &per_band_onsets[MID] {
        onset_frames.push(f);
    }
    for &f in &per_band_onsets[HIGH_MID] {
        onset_frames.push(f);
    }
    onset_frames.sort_unstable();
    onset_frames.dedup_by(|a, b| a.abs_diff(*b) < 3);

    // For each candidate onset, find peak amplitude in a small
    // post-onset window in the time domain — that's our velocity.
    // Then run YIN + polyphony probe.
    let yin_window_samples = (sr_f * 0.10) as usize; // 100 ms
    let pre_skip_samples = (sr_f * 0.020) as usize; // skip the attack noise (~20 ms)
    let lookahead_samples = (sr_f * 0.150) as usize;

    let lag_min = (sr_f / pitch_hi_hz) as usize;
    let lag_max = ((sr_f / pitch_lo_hz) as usize).min(yin_window_samples - 1);

    let mut events: Vec<GuitarEvent> = Vec::new();
    let mut polyphony_acc = 0.0_f32;
    let mut polyphony_n = 0_u32;

    for &of in &onset_frames {
        let sample_at_onset = of * FFT_HOP;
        if sample_at_onset >= mono.len() {
            continue;
        }
        // Velocity: max |sample| in 0..lookahead from the onset.
        let velocity = {
            let end = (sample_at_onset + lookahead_samples).min(mono.len());
            mono[sample_at_onset..end]
                .iter()
                .fold(0.0_f32, |m, &s| m.max(s.abs()))
        };

        // Decay: walk band_energy of MID, find peak then time to 30%.
        let decay_ms = decay_ms_from_energy(&band_energy[MID], of, frame_secs);

        if velocity < pick_threshold {
            // Sub-threshold onset = noise. Persist with no pitch.
            events.push(GuitarEvent {
                time_secs: of as f32 * frame_secs,
                pitch_hz: None,
                confidence: 1.0,
                velocity,
                decay_ms,
                kind: PickKind::Noise,
            });
            continue;
        }

        // YIN window — start a touch after the onset so the attack
        // transient doesn't dominate the autocorrelation.
        let win_start = (sample_at_onset + pre_skip_samples).min(mono.len());
        let win_end = (win_start + yin_window_samples).min(mono.len());
        if win_end - win_start < lag_max + 8 {
            // Not enough samples left — emit as Noise.
            events.push(GuitarEvent {
                time_secs: of as f32 * frame_secs,
                pitch_hz: None,
                confidence: 1.0,
                velocity,
                decay_ms,
                kind: PickKind::Noise,
            });
            continue;
        }
        let window = &mono[win_start..win_end];

        // Polyphony probe: spectral peak count in the window.
        let poly_score = polyphony_score(window, sr, settings);
        polyphony_acc += poly_score;
        polyphony_n += 1;

        let (pitch_opt, confidence) =
            yin_pitch(window, sr_f, lag_min, lag_max, settings.yin_threshold);

        // Strum classification — many spectral peaks → no pitch
        // even if YIN happened to lock onto something.
        let is_polyphonic = poly_score >= 0.65;

        let kind = if is_polyphonic {
            PickKind::Strum
        } else if pitch_opt.is_none() {
            PickKind::Noise
        } else {
            // Compare to most recent pitched event for Pluck / Repeat.
            let prev_pitch = events
                .iter()
                .rev()
                .find_map(|e| e.pitch_hz.filter(|_| e.kind != PickKind::Noise));
            match (prev_pitch, pitch_opt) {
                (Some(p_prev), Some(p_now)) => {
                    let cents = 1200.0 * (p_now / p_prev).log2().abs();
                    if cents <= settings.same_pitch_cents {
                        PickKind::Repeat
                    } else {
                        PickKind::Pluck
                    }
                }
                _ => PickKind::Pluck,
            }
        };

        let pitch_hz = if matches!(kind, PickKind::Strum | PickKind::Noise) {
            None
        } else {
            pitch_opt
        };

        events.push(GuitarEvent {
            time_secs: of as f32 * frame_secs,
            pitch_hz,
            confidence,
            velocity,
            decay_ms,
            kind,
        });
    }

    // Slide detection: a Pluck whose pitch is within 200 cents of the
    // previous Pluck AND fires within 100 ms gets reclassified as
    // Slide. Bend behaves the same way structurally — both have a
    // smooth-pitch-transition signature.
    for i in 1..events.len() {
        let (prev_t, prev_pitch, prev_kind) = (
            events[i - 1].time_secs,
            events[i - 1].pitch_hz,
            events[i - 1].kind,
        );
        let cur = &mut events[i];
        if cur.kind != PickKind::Pluck || prev_kind != PickKind::Pluck {
            continue;
        }
        let (Some(p1), Some(p0)) = (cur.pitch_hz, prev_pitch) else {
            continue;
        };
        if cur.time_secs - prev_t > 0.100 {
            continue;
        }
        let cents = 1200.0 * (p1 / p0).log2().abs();
        if (50.0..200.0).contains(&cents) {
            cur.kind = PickKind::Slide;
        }
    }

    // Roll up counts.
    let mut pick_count = 0_u32;
    let mut repeated = 0_u32;
    let mut pitch_change = 0_u32;
    let mut bends = 0_u32;
    let mut strums = 0_u32;
    for ev in &events {
        match ev.kind {
            PickKind::Noise => {}
            PickKind::Strum => {
                pick_count += 1;
                strums += 1;
            }
            PickKind::Pluck => {
                pick_count += 1;
                pitch_change += 1;
            }
            PickKind::Repeat => {
                pick_count += 1;
                repeated += 1;
            }
            PickKind::Slide => {
                pick_count += 1;
                bends += 1;
            }
        }
    }

    let estimated_polyphony = if polyphony_n > 0 {
        polyphony_acc / polyphony_n as f32
    } else {
        0.0
    };

    GuitarTelemetry {
        pick_count,
        repeated_pick_count: repeated,
        pitch_change_count: pitch_change,
        bend_or_slide_count: bends,
        strum_count: strums,
        estimated_polyphony: estimated_polyphony.clamp(0.0, 1.0),
        events,
    }
}

fn decay_ms_from_energy(energy: &[f32], onset_frame: usize, frame_secs: f32) -> f32 {
    if onset_frame >= energy.len() - 1 {
        return 0.0;
    }
    let look_ahead = 10.min(energy.len() - onset_frame - 1);
    let mut peak_idx = onset_frame;
    let mut peak_val = energy[onset_frame];
    for (i, &v) in energy.iter().enumerate().skip(onset_frame).take(look_ahead) {
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
}

/// Polyphony probe — count spectral peaks above –12 dB from max in
/// the time window. Returns a score in [0, 1] where 0 ≈ pure tone,
/// 1 ≈ many simultaneous fundamentals (strum). Uses the same
/// `settings.polyphony_peak_count` cutoff but reports a continuous
/// score so the GuitarTelemetry's `estimated_polyphony` can be a
/// useful average over the whole track, not just a binary.
fn polyphony_score(window: &[f32], sr: u32, settings: &TelemetrySettings) -> f32 {
    use rustfft::{num_complex::Complex, FftPlanner};
    // Pad / truncate to a fixed power-of-two FFT for the probe.
    const PROBE_SIZE: usize = 4096;
    let mut buf = vec![Complex { re: 0.0, im: 0.0 }; PROBE_SIZE];
    let n = window.len().min(PROBE_SIZE);
    for (i, b) in buf.iter_mut().enumerate().take(n) {
        let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (PROBE_SIZE - 1) as f32).cos();
        b.re = window[i] * w;
    }
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(PROBE_SIZE);
    fft.process(&mut buf);
    let half = PROBE_SIZE / 2;
    let mags: Vec<f32> = buf[..half].iter().map(|c| c.norm()).collect();
    let bin_hz = sr as f32 / PROBE_SIZE as f32;

    // Restrict to musical fundamentals range (50–2000 Hz). We don't
    // care if there's an extra peak at 8 kHz — that's overtone, not
    // a separate fundamental.
    let lo = (50.0 / bin_hz) as usize;
    let hi = ((2_000.0 / bin_hz) as usize).min(half);
    if hi <= lo + 4 {
        return 0.0;
    }

    let max = mags[lo..hi].iter().cloned().fold(0.0_f32, f32::max);
    if max < 1e-6 {
        return 0.0;
    }
    let threshold = max * 0.25; // –12 dB
    let mut peaks = 0_usize;
    for i in (lo + 1)..(hi - 1) {
        if mags[i] >= threshold && mags[i] > mags[i - 1] && mags[i] > mags[i + 1] {
            peaks += 1;
        }
    }
    // Map peak count → [0, 1] saturating at the configured cutoff.
    let cap = settings.polyphony_peak_count.max(1) as f32;
    (peaks as f32 / cap).clamp(0.0, 1.0)
}

/// YIN pitch tracker (de Cheveigné & Kawahara 2002). Returns the
/// detected fundamental in Hz and the cumulative mean normalised
/// difference at the chosen lag (lower = more confident; values
/// below `threshold` are typically reliable).
///
/// Implementation: difference function → cumulative mean normalised
/// difference (CMND) → first lag whose CMND drops below the threshold
/// → parabolic interpolation around that lag. ~80 LOC, no FFT —
/// straight time-domain autocorrelation. O(n²) in the lag range,
/// but n is only a few hundred for guitar / bass fundamentals at a
/// 100 ms window, so fast enough.
///
/// Returns `(None, normalised_difference_at_min)` when no lag in
/// the search range is below the threshold.
fn yin_pitch(
    x: &[f32],
    sr: f32,
    lag_min: usize,
    lag_max: usize,
    threshold: f32,
) -> (Option<f32>, f32) {
    if x.len() < lag_max + 4 || lag_max <= lag_min {
        return (None, 1.0);
    }
    let n = x.len();
    let max_tau = lag_max.min(n / 2);
    if max_tau <= lag_min {
        return (None, 1.0);
    }

    // Step 1 — difference function d_t(τ) = Σ_{j=0}^{W-1} (x_j − x_{j+τ})².
    // Step 2 — cumulative mean normalised difference d′_t(τ) = d_t(τ) /
    //          ((1/τ) · Σ_{k=1}^τ d_t(k)).  d′(0) := 1.
    let mut d = vec![0.0_f32; max_tau + 1];
    for tau in 1..=max_tau {
        let mut s = 0.0_f32;
        let limit = n - tau;
        for j in 0..limit {
            let diff = x[j] - x[j + tau];
            s += diff * diff;
        }
        d[tau] = s;
    }
    let mut cmnd = vec![1.0_f32; max_tau + 1];
    let mut running = 0.0_f64;
    for tau in 1..=max_tau {
        running += d[tau] as f64;
        let avg = (running / tau as f64) as f32;
        cmnd[tau] = if avg > 1e-9 { d[tau] / avg } else { 1.0 };
    }

    // Step 3 — first τ ≥ lag_min with cmnd(τ) < threshold AND a
    // local minimum (cmnd(τ-1) > cmnd(τ) and cmnd(τ+1) > cmnd(τ)).
    let mut chosen: Option<usize> = None;
    for tau in lag_min..max_tau {
        if cmnd[tau] < threshold && cmnd[tau] < cmnd[tau + 1] {
            // walk down to the local minimum (handles plateau edges)
            let mut t = tau;
            while t < max_tau && cmnd[t + 1] < cmnd[t] {
                t += 1;
            }
            chosen = Some(t);
            break;
        }
    }
    let Some(tau) = chosen else {
        // Best-effort: report the cmnd at the global minimum so the
        // caller has a confidence number even when no candidate
        // crossed the threshold.
        let (_, &min_v) = cmnd[lag_min..max_tau]
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or((0, &1.0));
        return (None, min_v);
    };

    // Step 4 — parabolic interpolation around the chosen lag for
    // sub-sample accuracy.
    let refined_tau = if tau == 0 || tau == max_tau {
        tau as f32
    } else {
        let s0 = cmnd[tau - 1];
        let s1 = cmnd[tau];
        let s2 = cmnd[tau + 1];
        let denom = 2.0 * (2.0 * s1 - s0 - s2);
        let delta = if denom.abs() > 1e-9 {
            (s2 - s0) / denom
        } else {
            0.0
        };
        tau as f32 + delta.clamp(-1.0, 1.0)
    };
    if refined_tau < 1.0 {
        return (None, cmnd[tau]);
    }
    (Some(sr / refined_tau), cmnd[tau])
}

// ───────────────────── Krumhansl-Schmuckler key estimation ─────────────────────

/// Krumhansl & Kessler (1982) major / minor key profiles. 12 values
/// each, indexed by pitch class (0=C, 1=C♯, …, 11=B). The numbers
/// reflect the perceived fit of each scale degree within the key.
const KS_MAJOR: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const KS_MINOR: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// Estimate the key from a list of `GuitarEvent`s. Builds a
/// pitch-class histogram (12 bins) weighted by velocity × duration-
/// until-next-event, correlates against each of the 24 K-S templates,
/// returns the best fit + a runner-up.
pub fn estimate_key_from_events(events: &[GuitarEvent]) -> Option<KeyEstimate> {
    let pitched: Vec<&GuitarEvent> = events
        .iter()
        .filter(|e| e.pitch_hz.is_some() && !matches!(e.kind, PickKind::Noise | PickKind::Strum))
        .collect();
    if pitched.len() < 8 {
        // Not enough pitched material to commit to a key.
        return None;
    }
    let mut histogram = [0.0_f32; 12];
    for (i, ev) in pitched.iter().enumerate() {
        let Some(hz) = ev.pitch_hz else { continue };
        // Convert Hz to MIDI: m = 69 + 12·log2(hz / 440).
        let midi = 69.0 + 12.0 * (hz / 440.0).log2();
        let pc = ((midi.round() as i32).rem_euclid(12)) as usize;
        // Weight by velocity × duration until the next pitched event
        // (default 0.25 s for the last event so it isn't ignored).
        let dur = if i + 1 < pitched.len() {
            (pitched[i + 1].time_secs - ev.time_secs).clamp(0.05, 4.0)
        } else {
            0.25
        };
        histogram[pc] += ev.velocity.max(0.05) * dur;
    }

    estimate_key_from_histogram(&histogram)
}

/// Public so the project-level aggregation (sum of per-track
/// histograms) can use the same K-S code path.
pub fn estimate_key_from_histogram(histogram: &[f32; 12]) -> Option<KeyEstimate> {
    let total: f32 = histogram.iter().sum();
    if total < 1e-3 {
        return None;
    }

    // Score every (root, mode) by Pearson correlation of the rotated
    // histogram against the corresponding K-S template.
    let mut scores: Vec<(u8, KeyMode, f32)> = Vec::with_capacity(24);
    for root in 0..12 {
        for &(template, mode) in &[(KS_MAJOR, KeyMode::Major), (KS_MINOR, KeyMode::Minor)] {
            let r = pearson_rotated(histogram, &template, root);
            scores.push((root as u8, mode, r));
        }
    }
    scores.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let (root, mode, conf) = scores[0];
    let (root2, mode2, conf2) = scores[1];
    Some(KeyEstimate {
        root,
        mode,
        confidence: conf,
        second_choice_root: root2,
        second_choice_mode: mode2,
        second_choice_confidence: conf2,
    })
}

/// Pearson correlation between `hist` (rotated by `root` so
/// `hist[root + i mod 12]` aligns with `template[i]`) and `template`.
fn pearson_rotated(hist: &[f32; 12], template: &[f32; 12], root: usize) -> f32 {
    let mut h_mean = 0.0_f32;
    let mut t_mean = 0.0_f32;
    for i in 0..12 {
        h_mean += hist[(root + i) % 12];
        t_mean += template[i];
    }
    h_mean /= 12.0;
    t_mean /= 12.0;
    let mut num = 0.0_f32;
    let mut den_h = 0.0_f32;
    let mut den_t = 0.0_f32;
    for i in 0..12 {
        let dh = hist[(root + i) % 12] - h_mean;
        let dt = template[i] - t_mean;
        num += dh * dt;
        den_h += dh * dh;
        den_t += dt * dt;
    }
    if den_h < 1e-9 || den_t < 1e-9 {
        return 0.0;
    }
    num / (den_h.sqrt() * den_t.sqrt())
}

/// Aggregate key estimate from many tracks' telemetry — sums their
/// pitch-class histograms (re-derived from each track's events) and
/// runs K-S over the union. The right call for the project-level
/// "Estimated key" readout in the Project tab.
pub fn estimate_song_key(tracks: &[crate::project::Track]) -> Option<KeyEstimate> {
    let mut histogram = [0.0_f32; 12];
    let mut total_events = 0_usize;
    for t in tracks {
        let Some(tel) = t.telemetry.as_ref() else {
            continue;
        };
        let Some(g) = tel.guitar.as_ref() else {
            continue;
        };
        let pitched: Vec<&GuitarEvent> = g
            .events
            .iter()
            .filter(|e| {
                e.pitch_hz.is_some() && !matches!(e.kind, PickKind::Noise | PickKind::Strum)
            })
            .collect();
        for (i, ev) in pitched.iter().enumerate() {
            let Some(hz) = ev.pitch_hz else { continue };
            let midi = 69.0 + 12.0 * (hz / 440.0).log2();
            let pc = ((midi.round() as i32).rem_euclid(12)) as usize;
            let dur = if i + 1 < pitched.len() {
                (pitched[i + 1].time_secs - ev.time_secs).clamp(0.05, 4.0)
            } else {
                0.25
            };
            histogram[pc] += ev.velocity.max(0.05) * dur;
        }
        total_events += pitched.len();
    }
    if total_events < 16 {
        return None;
    }
    estimate_key_from_histogram(&histogram)
}

/// Sentinel telemetry record — what a "Profile = Off" track gets.
/// All numerics zeroed; analyzer_version current so the dispatcher
/// won't keep re-trying. Safe to render — the chip strip skips zero
/// onset / zero pick / zero drum cases automatically.
fn empty_telemetry() -> TrackTelemetry {
    TrackTelemetry {
        analyzer_version: ANALYZER_VERSION,
        spectral_centroid_avg: 0.0,
        spectral_centroid_std: 0.0,
        spectral_flatness_avg: 0.0,
        spectral_rolloff_avg: 0.0,
        rms_avg_db: -120.0,
        rms_std_db: 0.0,
        crest_factor_avg: 0.0,
        peak_db: -120.0,
        onset_count: 0,
        onset_rate_hz: 0.0,
        sustain_ratio: 0.0,
        arousal: 0.0,
        valence: 0.0,
        cross_band_coherence: 0.0,
        drum_kit: None,
        guitar: None,
        key_estimate: None,
    }
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
    /// Profile already resolved from `(TelemetryProfile, TrackSource)`
    /// by the caller. The worker doesn't re-resolve.
    pub profile: ResolvedProfile,
    /// Snapshot of the user-tweakable thresholds at dispatch time.
    /// Avoids a shared lock; if the user edits settings mid-batch
    /// the in-flight requests still finish on the old values.
    pub settings: TelemetrySettings,
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
                    let outcome = if req.profile == ResolvedProfile::None {
                        // Profile == Off → don't even open the WAV.
                        Ok(empty_telemetry())
                    } else {
                        analyze_wav(&req.abs_path, req.profile, &req.settings)
                            .map_err(|e| format!("{e:#}"))
                    };
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
        let t = analyze_wav(
            &path,
            ResolvedProfile::UniversalOnly,
            &TelemetrySettings::default(),
        )
        .unwrap();
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
        let t = analyze_wav(
            &path,
            ResolvedProfile::UniversalOnly,
            &TelemetrySettings::default(),
        )
        .unwrap();
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
        let t = analyze_wav(
            &path,
            ResolvedProfile::UniversalOnly,
            &TelemetrySettings::default(),
        )
        .unwrap();
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

    /// Synthetic 440 Hz sine into YIN should report ~440 Hz within
    /// 5 cents. Confidence (cmnd at the chosen lag) should be near 0.
    #[test]
    fn yin_recovers_pure_a4() {
        let sr = 48_000.0;
        let f = 440.0;
        let n = 4096;
        let samples: Vec<f32> = (0..n)
            .map(|i| 0.5 * (std::f32::consts::TAU * f * i as f32 / sr).sin())
            .collect();
        let lag_min = (sr / 1_000.0) as usize;
        let lag_max = (sr / 80.0) as usize;
        let (pitch, conf) = yin_pitch(&samples, sr, lag_min, lag_max, 0.15);
        let pitch = pitch.expect("YIN should lock on a pure 440 Hz sine");
        let cents = 1200.0 * (pitch / f).log2().abs();
        assert!(
            cents < 5.0,
            "YIN drift = {cents:.2} cents (got {pitch:.2} Hz)"
        );
        assert!(conf < 0.10, "YIN confidence too high (worse): {conf}");
    }

    /// The polyphony probe should fire (high score) on a chord and
    /// stay quiet (low score) on a single sine. This is what gates
    /// `PickKind::Strum` in `analyze_guitar_picks`. We verify the
    /// gate, not YIN itself — YIN is known to lock onto the implied
    /// fundamental of a triad (the chord root), which is actually
    /// the right behaviour for "what key is this chord in".
    #[test]
    fn polyphony_probe_separates_mono_from_chord() {
        let sr = 48_000;
        let n = 4096;
        let mono: Vec<f32> = (0..n)
            .map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / sr as f32).sin())
            .collect();
        // Three pitches with no clean low-period commonality: detuned
        // so the polyphony probe sees three distinct spectral peaks.
        let chord: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / sr as f32;
                ((std::f32::consts::TAU * 313.0 * t).sin()
                    + (std::f32::consts::TAU * 461.0 * t).sin()
                    + (std::f32::consts::TAU * 727.0 * t).sin())
                    * 0.3
            })
            .collect();
        let s = TelemetrySettings::default();
        let mono_score = polyphony_score(&mono, sr, &s);
        let chord_score = polyphony_score(&chord, sr, &s);
        assert!(
            chord_score > mono_score + 0.1,
            "expected chord_score > mono_score by 0.1+, got mono={mono_score} chord={chord_score}"
        );
    }

    /// Krumhansl-Schmuckler should return the right key for a
    /// hand-built C major histogram (major-scale notes weighted
    /// strongly, off-scale notes weighted weakly).
    #[test]
    fn ks_finds_c_major_from_scale_histogram() {
        // C major scale degrees with strong tonic + dominant
        // weighting, weak chromatic notes. Should correlate strongest
        // with the major template rotated to C (root = 0).
        let mut h = [0.0_f32; 12];
        h[0] = 6.0; // C
        h[2] = 4.0; // D
        h[4] = 4.0; // E
        h[5] = 3.5; // F
        h[7] = 5.5; // G
        h[9] = 3.5; // A
        h[11] = 3.0; // B
                     // Light chromatic noise on the off-scale notes.
        h[1] = 0.5;
        h[3] = 0.5;
        h[6] = 0.5;
        h[8] = 0.5;
        h[10] = 0.5;

        let est = estimate_key_from_histogram(&h).expect("non-empty histogram → Some");
        assert_eq!(est.root, 0, "expected C, got {}", est.root);
        assert_eq!(est.mode, KeyMode::Major);
        assert!(
            est.confidence > 0.7,
            "confidence too low: {}",
            est.confidence
        );
    }

    /// K-S on an empty histogram returns None — guard against /0.
    #[test]
    fn ks_returns_none_on_empty_histogram() {
        let h = [0.0_f32; 12];
        assert!(estimate_key_from_histogram(&h).is_none());
    }

    /// `KeyEstimate::label` produces sensible strings for the canonical
    /// roots — a regression-guard for the 12-tone naming table.
    #[test]
    fn key_estimate_labels_match_table() {
        let make = |root: u8, mode: KeyMode| KeyEstimate {
            root,
            mode,
            confidence: 1.0,
            second_choice_root: 0,
            second_choice_mode: KeyMode::Major,
            second_choice_confidence: 0.0,
        };
        assert_eq!(make(0, KeyMode::Major).label(), "C maj");
        assert_eq!(make(9, KeyMode::Minor).label(), "A min");
        assert_eq!(make(8, KeyMode::Major).label(), "A♭ maj");
    }

    /// `TelemetryProfile::resolve` must honour explicit values and
    /// defer to `Auto` only when it's been chosen.
    #[test]
    fn profile_resolution_explicit_overrides_role() {
        use crate::project::{StemRole, TrackSource};
        let suno_drums = TrackSource::SunoStem {
            role: StemRole::Drums,
            original_filename: "drums.wav".into(),
            session_epoch: None,
            session_ordinal: None,
            provenance: None,
        };
        // Auto on a Drums stem → Drums.
        assert_eq!(
            TelemetryProfile::Auto.resolve(&suno_drums),
            ResolvedProfile::Drums
        );
        // Explicit Guitar wins, even if the role says Drums.
        assert_eq!(
            TelemetryProfile::Guitar.resolve(&suno_drums),
            ResolvedProfile::Guitar
        );
        // Explicit None always disables.
        assert_eq!(
            TelemetryProfile::None.resolve(&suno_drums),
            ResolvedProfile::None
        );
        // Recorded with Auto → UniversalOnly.
        assert_eq!(
            TelemetryProfile::Auto.resolve(&TrackSource::Recorded),
            ResolvedProfile::UniversalOnly
        );
    }

    /// End-to-end: write a synthetic guitar-like WAV (decaying sines
    /// at three different pitches with sharp attacks at known times),
    /// run `analyze_wav` with the Guitar profile, expect ≥3 picks
    /// classified as Pluck and a usable key estimate populated.
    #[test]
    fn analyzer_guitar_profile_detects_picks_and_pitch() {
        let sr = 48_000;
        let dur_secs = 1.5_f32;
        let total = (sr as f32 * dur_secs) as usize;
        let mut samples = vec![0.0_f32; total];

        // Three hits at 0.1, 0.5, 0.9 s, pitches A2 (~110), E3 (~165),
        // A3 (~220). Sharp attack + exponential decay (~250 ms tau).
        let hits = [(0.10_f32, 110.0_f32), (0.50, 164.81), (0.90, 220.0)];
        for (t0, f) in hits {
            let start = (t0 * sr as f32) as usize;
            let tau = sr as f32 * 0.25; // 250 ms decay
            for i in 0..(sr as usize / 2) {
                if start + i >= total {
                    break;
                }
                let env = (-(i as f32) / tau).exp();
                samples[start + i] +=
                    0.4 * env * (std::f32::consts::TAU * f * i as f32 / sr as f32).sin();
            }
        }

        let dir = std::env::temp_dir();
        let path = dir.join("tbss_test_guitar.wav");
        write_synthetic_wav(&path, samples, sr).unwrap();
        let t = analyze_wav(
            &path,
            ResolvedProfile::Guitar,
            &TelemetrySettings::default(),
        )
        .unwrap();
        let _ = std::fs::remove_file(&path);

        let g = t
            .guitar
            .expect("guitar profile must populate guitar telemetry");
        assert!(
            g.pick_count >= 2,
            "expected ≥2 picks, got {} (events: {})",
            g.pick_count,
            g.events.len()
        );
        // At least one event should have a pitched read.
        let pitched = g.events.iter().filter(|e| e.pitch_hz.is_some()).count();
        assert!(pitched >= 1, "no pitched events recovered");
    }

    /// Drum classifier must collapse multi-band flux peaks from a
    /// single physical hit into one event. Pre-v3, a snare hit
    /// produced separate Snare + Cymbal + HiHat events because each
    /// per-band onset list was walked independently — total drum
    /// counts on a 3:20 Suno stem ran ~5,300 events (≈ 27/sec,
    /// physically impossible).
    ///
    /// We exercise `classify_drum_events` directly with hand-built
    /// per-band onset lists rather than via `analyze_wav` — the
    /// latter's median+k·MAD threshold collapses to zero on near-
    /// silent synthetic input and fires on statistical bumps,
    /// confounding the dedup invariant we want to lock in.
    ///
    /// Setup: two physical hits modeled as two onset clusters:
    ///   - "Kick" at frame 100, fires SUB only.
    ///   - "Snare" at frame 200, fires MID + HIGH_MID + HIGH within
    ///     a 2-frame window — the exact multi-band scenario that
    ///     pre-v3 over-counted as Snare + Other + HiHat.
    ///
    /// Expected: exactly 2 events out, regardless of how the four
    /// per-band onsets are arranged within their cluster windows.
    #[test]
    fn drum_classifier_dedupes_per_hit_no_double_count() {
        // 5 bands, ~250 frames of timeline. Need flux + energy curves
        // long enough that `decay_ms_for` and `is_harmonic_after_onset`
        // don't run off the end — pad to 300 frames.
        let frames = 300usize;
        let mut band_flux: Vec<Vec<f32>> = (0..5).map(|_| vec![0.0_f32; frames]).collect();
        let mut band_energy: Vec<Vec<f32>> = (0..5).map(|_| vec![0.0_f32; frames]).collect();

        // SUB band sees the kick at frame 100.
        band_flux[0][100] = 1.0;
        for i in 0..15 {
            band_energy[0][100 + i] = (-(i as f32) / 5.0).exp();
        }

        // MID + HIGH_MID + HIGH all see the snare around frame 200,
        // landing on slightly different frames (199/200/201) the way
        // real per-band peak pickers disagree on the exact onset frame
        // for a single physical hit.
        band_flux[2][200] = 0.9; // MID strongest → cluster classifies as Snare
        band_flux[3][199] = 0.6;
        band_flux[4][201] = 0.5;
        for i in 0..15 {
            let env = (-(i as f32) / 5.0).exp();
            band_energy[2][200 + i] = env;
            band_energy[3][199 + i] = env;
            band_energy[4][201 + i] = env;
        }

        // Per-band onset lists — exactly what the peak picker would
        // produce for the energy/flux curves above.
        let per_band_onsets: Vec<Vec<usize>> =
            vec![vec![100], vec![], vec![200], vec![199], vec![201]];

        // Stft must be present for `is_harmonic_after_onset` — give it
        // a minimal flat-spectrum stand-in (peak/mean ratio < 15 →
        // SUB classifies as Kick rather than Tom).
        let stft: Vec<Vec<f32>> = (0..frames).map(|_| vec![1.0_f32; 32]).collect();
        let band_names = ["sub_low", "low_mid", "mid", "high_mid", "high"];

        let kit = classify_drum_events(
            &per_band_onsets,
            &band_flux,
            &band_energy,
            &stft,
            &band_names,
            48_000,
        );
        let total = kit.kick_count
            + kit.snare_count
            + kit.hihat_count
            + kit.tom_count
            + kit.cymbal_count
            + kit.other_count;
        assert_eq!(
            total, 2,
            "expected exactly 2 events (1 kick cluster + 1 snare cluster), got {} (k={} s={} h={} t={} c={} o={}) events={:?}",
            total,
            kit.kick_count,
            kit.snare_count,
            kit.hihat_count,
            kit.tom_count,
            kit.cymbal_count,
            kit.other_count,
            kit.events.iter().map(|e| (e.time_secs, e.class)).collect::<Vec<_>>()
        );
        assert_eq!(
            kit.events.len(),
            2,
            "events list length must match the per-class total"
        );
        // Sanity: dominant band of the snare cluster was MID (highest
        // normalised flux in {MID 0.9, HIGH_MID 0.6, HIGH 0.5}) → Snare.
        assert!(
            kit.snare_count == 1,
            "snare cluster should classify as Snare via MID-band winner; events={:?}",
            kit.events
                .iter()
                .map(|e| (e.time_secs, e.class))
                .collect::<Vec<_>>()
        );
        // And the kick cluster (SUB only) should classify as Kick
        // because the flat synthetic STFT yields no harmonic peak.
        assert!(
            kit.kick_count == 1,
            "kick cluster should classify as Kick on a flat (non-harmonic) spectrum; events={:?}",
            kit.events
                .iter()
                .map(|e| (e.time_secs, e.class))
                .collect::<Vec<_>>()
        );
    }

    /// Build a synthetic STFT where every band shares a common
    /// modulation envelope (the "natural instrument" case). All
    /// bands rise and fall together → coherence should be HIGH
    /// (≥ 0.7 typically).
    #[test]
    fn coherence_high_when_bands_move_together() {
        let sr = 48_000;
        let half = FFT_SIZE / 2;
        let n_frames = 200;
        let mut stft = Vec::with_capacity(n_frames);
        for t in 0..n_frames {
            // Shared slow envelope (~2 Hz at our frame rate).
            let env = 0.5 + 0.5 * (std::f32::consts::TAU * t as f32 / n_frames as f32 * 4.0).sin();
            let frame: Vec<f32> = (0..half).map(|_| env).collect();
            stft.push(frame);
        }
        let c = compute_cross_band_coherence(&stft, sr);
        assert!(
            c >= 0.7,
            "common-envelope STFT should score ≥0.7 coherence, got {c}"
        );
    }

    /// Build a synthetic STFT where each band's envelope is
    /// independently random (the "AI fingerprint" case). Bands
    /// modulate without correlation → coherence should be LOW
    /// (close to 0).
    #[test]
    fn coherence_low_when_bands_decorrelated() {
        let sr = 48_000;
        let half = FFT_SIZE / 2;
        let n_frames = 200;
        let bin_hz = sr as f32 / FFT_SIZE as f32;
        // Per-band independent envelopes. We use deterministic LFOs
        // at unrelated frequencies and phases so the test is stable
        // without an RNG dep.
        let band_freqs = [1.7_f32, 2.9, 3.3, 4.1, 5.7, 6.3, 7.1, 8.9];
        let band_phase = [0.0_f32, 0.5, 1.1, 1.8, 2.6, 3.4, 4.3, 5.2];
        let mut stft = Vec::with_capacity(n_frames);
        for t in 0..n_frames {
            let mut frame = vec![0.0_f32; half];
            for (bi, &centre) in [60.0_f32, 120.0, 240.0, 480.0, 960.0, 1920.0, 3840.0, 7680.0]
                .iter()
                .enumerate()
            {
                let bin = (centre / bin_hz).round() as usize;
                let lo = bin.saturating_sub(2);
                let hi = (bin + 2).min(half - 1);
                let env = 0.5
                    + 0.5
                        * (std::f32::consts::TAU * t as f32 / n_frames as f32 * band_freqs[bi]
                            + band_phase[bi])
                            .sin();
                for f in &mut frame[lo..=hi] {
                    *f = env;
                }
            }
            stft.push(frame);
        }
        let c = compute_cross_band_coherence(&stft, sr);
        assert!(
            c.abs() < 0.4,
            "decorrelated-envelope STFT should score |c|<0.4 (≈0); got {c}"
        );
    }

    /// Degenerate inputs return 0 rather than NaN / panicking.
    #[test]
    fn coherence_degenerate_inputs_safe() {
        let sr = 48_000;
        // Empty STFT
        assert_eq!(compute_cross_band_coherence(&[], sr), 0.0);
        // Too few frames
        let short: Vec<Vec<f32>> = (0..4).map(|_| vec![0.5; FFT_SIZE / 2]).collect();
        assert_eq!(compute_cross_band_coherence(&short, sr), 0.0);
        // All-zero spectrum → bands have zero variance → division
        // by stddev guarded with .max(1e-9); should not NaN.
        let zeros: Vec<Vec<f32>> = (0..200).map(|_| vec![0.0; FFT_SIZE / 2]).collect();
        let c = compute_cross_band_coherence(&zeros, sr);
        assert!(c.is_finite(), "all-zero STFT must not produce NaN; got {c}");
    }
}
