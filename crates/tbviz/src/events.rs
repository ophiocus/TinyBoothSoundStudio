//! Live sound-event detector — the "licks & beats" track.
//!
//! A streaming recovery of TinyBooth's offline telemetry drum analyzer
//! (`telemetry::analyze_wav` / `classify_drum_events`): the same
//! 5-band spectral-flux onset detection + adaptive `median + k·MAD`
//! peak-pick + dominant-band drum classification, reworked to run
//! frame-by-frame off the visualizer's rolling spectrum instead of a
//! whole-track STFT. Host-agnostic — works for TinyBooth and TinyAmp.
//!
//! The detector produces a [`LickFrame`] snapshot each frame (recent
//! events + tempo + beat phase). It is **absent until warmed** (returns
//! `None`), which is the structural null-gate the visualizer modules
//! rely on: `ctx.licks` is `Option`, so a plugin cannot accidentally
//! operate on a missing track.

use std::collections::VecDeque;

/// Drum-kit class of a detected event. Mirrors `telemetry::DrumClass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LickClass {
    Kick,
    Snare,
    HiHat,
    Tom,
    Cymbal,
    Other,
}

impl LickClass {
    /// Class → (r,g,b) tint for the counter-visual overlay. Warm lows,
    /// bright highs — reads as a drum-kit heat order.
    pub fn color(self) -> (u8, u8, u8) {
        match self {
            LickClass::Kick => (240, 90, 70),     // deep red — sub thump
            LickClass::Tom => (240, 150, 70),     // orange
            LickClass::Snare => (240, 220, 90),   // yellow — the backbeat
            LickClass::HiHat => (120, 220, 255),  // cyan — sizzle
            LickClass::Cymbal => (200, 160, 255), // violet — wash
            LickClass::Other => (180, 180, 190),  // grey
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            LickClass::Kick => "kick",
            LickClass::Snare => "snare",
            LickClass::HiHat => "hat",
            LickClass::Tom => "tom",
            LickClass::Cymbal => "cymbal",
            LickClass::Other => "hit",
        }
    }
}

/// One detected event, aged relative to the current frame.
#[derive(Debug, Clone, Copy)]
pub struct LickEvent {
    pub class: LickClass,
    /// Peak flux normalised to `[0, 1]` (a "velocity").
    pub velocity: f32,
    /// Seconds since the event fired (0 = this frame).
    pub age: f32,
    /// Which of the 5 bands owned the event (0=sub … 4=high).
    pub band: u8,
}

/// Per-frame snapshot handed to the visualizer modules.
pub struct LickFrame {
    /// Recent events, newest last, all with `age < RECENT_SECS`.
    pub events: Vec<LickEvent>,
    /// Estimated tempo once enough beats have been seen.
    pub bpm: Option<f32>,
    /// Position within the current beat, `[0, 1)`.
    pub beat_phase: f32,
    /// True on the exact frame an event fired.
    pub fired: bool,
}

impl LickFrame {
    /// The strongest event this frame (if any fired) — convenience for
    /// modules that want a single impulse.
    pub fn strongest(&self) -> Option<&LickEvent> {
        self.events.iter().filter(|e| e.age < 1e-3).max_by(|a, b| {
            a.velocity
                .partial_cmp(&b.velocity)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }
}

// 5 bands (Hz), matching `telemetry::FreqBands::classic()`.
const BANDS: [(f32, f32); 5] = [
    (40.0, 120.0),       // sub_low  — kick fundamental
    (80.0, 300.0),       // low_mid  — tom / snare body
    (200.0, 800.0),      // mid      — snare
    (1_500.0, 5_000.0),  // high_mid — cymbal body
    (5_000.0, 12_000.0), // high    — hi-hat / sheen
];
/// k·MAD multiplier — matches `TelemetrySettings::drum_onset_k_mad`.
const K_MAD: f32 = 3.0;
/// Rolling flux window (frames) feeding the adaptive threshold (~2 s).
const FLUX_HIST: usize = 60;
/// Frames of history before the detector trusts its threshold.
const WARMUP: usize = 20;
/// Minimum gap between emitted events (dominant-band de-dup / refractory).
const REFRACTORY_SECS: f64 = 0.06;
/// Events older than this are dropped from the snapshot.
const RECENT_SECS: f32 = 1.2;

/// Streaming lick/beat detector. Persistent across frames (the host
/// owns one and `update`s it each frame).
pub struct LickDetector {
    prev_energy: [f32; 5],
    flux_hist: [VecDeque<f32>; 5],
    flux_max: [f32; 5],
    recent: VecDeque<(f64, LickClass, f32, u8)>, // (time, class, vel, band)
    beat_times: VecDeque<f64>,
    bpm: Option<f32>,
    last_fire: f64,
    fired_this_frame: bool,
    frames_seen: usize,
}

impl Default for LickDetector {
    fn default() -> Self {
        Self {
            prev_energy: [0.0; 5],
            flux_hist: Default::default(),
            flux_max: [1e-6; 5],
            recent: VecDeque::with_capacity(64),
            beat_times: VecDeque::with_capacity(16),
            bpm: None,
            last_fire: -1.0,
            fired_this_frame: false,
            frames_seen: 0,
        }
    }
}

fn band_energy(spectrum: &[f32], sample_rate: u32, band: (f32, f32)) -> f32 {
    if spectrum.is_empty() || sample_rate == 0 {
        return 0.0;
    }
    let fft_size = (spectrum.len() * 2) as f32;
    let lo = ((band.0 * fft_size / sample_rate as f32) as usize).min(spectrum.len() - 1);
    let hi = ((band.1 * fft_size / sample_rate as f32) as usize).min(spectrum.len() - 1);
    if hi <= lo {
        return spectrum[lo];
    }
    spectrum[lo..=hi].iter().sum()
}

fn median(v: &VecDeque<f32>) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s: Vec<f32> = v.iter().copied().collect();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    s[s.len() / 2]
}

fn mad(v: &VecDeque<f32>, med: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut dev: Vec<f32> = v.iter().map(|x| (x - med).abs()).collect();
    dev.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    dev[dev.len() / 2]
}

fn class_of(band: usize) -> LickClass {
    // Live simplification of `classify_drum_events` band→class mapping
    // (no post-onset harmonic/decay look-ahead in a streaming context).
    match band {
        0 => LickClass::Kick,
        1 => LickClass::Tom,
        2 => LickClass::Snare,
        3 => LickClass::Cymbal,
        _ => LickClass::HiHat,
    }
}

impl LickDetector {
    /// Advance the detector by one frame with the current spectrum.
    pub fn update(&mut self, spectrum: &[f32], sample_rate: u32, time: f64) {
        self.fired_this_frame = false;
        if spectrum.is_empty() || sample_rate == 0 {
            return;
        }
        self.frames_seen += 1;

        // Per-band energy → half-wave-rectified flux → history.
        let mut flux = [0.0_f32; 5];
        for (b, &band) in BANDS.iter().enumerate() {
            let e = band_energy(spectrum, sample_rate, band);
            flux[b] = (e - self.prev_energy[b]).max(0.0);
            self.prev_energy[b] = e;
            self.flux_max[b] = self.flux_max[b].max(flux[b]);
            let h = &mut self.flux_hist[b];
            h.push_back(flux[b]);
            while h.len() > FLUX_HIST {
                h.pop_front();
            }
        }

        if self.frames_seen < WARMUP {
            return;
        }

        // One event per refractory window: the band with the largest
        // above-threshold flux wins (dominant-band rule).
        if time - self.last_fire < REFRACTORY_SECS {
            return;
        }
        let mut best_band: Option<usize> = None;
        let mut best_flux = 0.0_f32;
        for (b, &fb) in flux.iter().enumerate() {
            let hist = &self.flux_hist[b];
            let med = median(hist);
            let thr = med + K_MAD * mad(hist, med).max(1e-6);
            if fb > thr && fb > best_flux {
                best_flux = fb;
                best_band = Some(b);
            }
        }
        if let Some(b) = best_band {
            self.last_fire = time;
            self.fired_this_frame = true;
            let vel = (flux[b] / self.flux_max[b].max(1e-6)).clamp(0.0, 1.0);
            let class = class_of(b);
            self.recent.push_back((time, class, vel, b as u8));
            while self.recent.len() > 64 {
                self.recent.pop_front();
            }
            // Tempo from strong backbeat events (kick / snare).
            if matches!(class, LickClass::Kick | LickClass::Snare) {
                self.beat_times.push_back(time);
                while self.beat_times.len() > 12 {
                    self.beat_times.pop_front();
                }
                self.recompute_bpm();
            }
        }
    }

    fn recompute_bpm(&mut self) {
        if self.beat_times.len() < 4 {
            return;
        }
        let mut iois: Vec<f64> = self
            .beat_times
            .iter()
            .zip(self.beat_times.iter().skip(1))
            .map(|(a, b)| b - a)
            .filter(|d| *d > 0.15 && *d < 2.0) // 30–400 bpm gate
            .collect();
        if iois.len() < 3 {
            return;
        }
        iois.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let med = iois[iois.len() / 2];
        let bpm = (60.0 / med) as f32;
        self.bpm = Some(bpm.clamp(50.0, 210.0));
    }

    /// Produce the per-frame snapshot for the modules. Returns `None`
    /// until the detector is warmed — the structural null-gate.
    pub fn snapshot(&self, now: f64) -> Option<LickFrame> {
        if self.frames_seen < WARMUP {
            return None;
        }
        let events: Vec<LickEvent> = self
            .recent
            .iter()
            .filter_map(|&(t, class, vel, band)| {
                let age = (now - t) as f32;
                if (0.0..=RECENT_SECS).contains(&age) {
                    Some(LickEvent {
                        class,
                        velocity: vel,
                        age,
                        band,
                    })
                } else {
                    None
                }
            })
            .collect();
        let beat_phase = match (self.bpm, self.beat_times.back()) {
            (Some(bpm), Some(&last)) => {
                let period = 60.0 / bpm as f64;
                (((now - last) / period).rem_euclid(1.0)) as f32
            }
            _ => 0.0,
        };
        Some(LickFrame {
            events,
            bpm: self.bpm,
            beat_phase,
            fired: self.fired_this_frame,
        })
    }

    /// Clear all state (host calls this on stop / track change).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a spectrum (len = fft/2) at `sr` with a burst of energy in
    /// a given Hz range — a synthetic "hit" in one band.
    fn spectrum_with_burst(len: usize, sr: u32, band: (f32, f32), amp: f32) -> Vec<f32> {
        let mut s = vec![0.02_f32; len];
        let fft = (len * 2) as f32;
        let lo = ((band.0 * fft / sr as f32) as usize).min(len - 1);
        let hi = ((band.1 * fft / sr as f32) as usize).min(len - 1);
        for v in s.iter_mut().take(hi + 1).skip(lo) {
            *v = amp;
        }
        s
    }

    #[test]
    fn silence_produces_no_events() {
        let mut d = LickDetector::default();
        let silent = vec![0.0_f32; 1024];
        for i in 0..60 {
            d.update(&silent, 44_100, i as f64 * 0.02);
        }
        let snap = d.snapshot(60.0 * 0.02).unwrap();
        assert!(snap.events.is_empty(), "silence must yield no licks");
        assert!(snap.bpm.is_none());
    }

    #[test]
    fn low_band_pulse_train_detects_kicks() {
        let mut d = LickDetector::default();
        let len = 1024;
        let sr = 44_100;
        let quiet = spectrum_with_burst(len, sr, (40.0, 120.0), 0.02);
        let hit = spectrum_with_burst(len, sr, (40.0, 120.0), 1.0);
        let mut t = 0.0_f64;
        let dt = 0.02;
        // Warm up on quiet frames.
        for _ in 0..25 {
            d.update(&quiet, sr, t);
            t += dt;
        }
        // Then a pulse train: one loud sub-band frame every ~0.5 s.
        let mut kicks = 0;
        for beat in 0..8 {
            for f in 0..25 {
                let spec = if f == 0 { &hit } else { &quiet };
                d.update(spec, sr, t);
                if d.snapshot(t).unwrap().fired {
                    kicks += 1;
                }
                let _ = beat;
                t += dt;
            }
        }
        assert!(kicks >= 5, "expected several kick onsets, got {kicks}");
        // The events should classify as Kick (sub band).
        let snap = d.snapshot(t).unwrap();
        assert!(
            d.recent.iter().any(|&(_, c, _, _)| c == LickClass::Kick),
            "sub-band pulses should classify as Kick"
        );
        let _ = snap;
    }

    #[test]
    fn not_warmed_returns_none() {
        let d = LickDetector::default();
        assert!(d.snapshot(0.0).is_none(), "cold detector = no lick track");
    }
}
