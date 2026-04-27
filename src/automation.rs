//! Volume automation — timestamped fader-gesture replay.
//!
//! A user arms a track or the master, plays the song, and rides the
//! fader. The UI thread samples `gain_db` at ~60 Hz, decimates by
//! "≥0.05 dB delta from last point", and pushes points onto a scratch
//! [`AutomationLane`]. On Stop the scratch lane replaces the track's
//! persisted `gain_automation` and a fresh [`SplineSampler`] is built
//! for the audio thread to query during playback.
//!
//! Replay uses Catmull-Rom interpolation via the `splines` crate
//! (TBSS-FR-0004 §4.4 surveyed alternatives — `splines` won).
//! Catmull-Rom needs four keys to interpolate; we pad each lane's
//! endpoints so any lane with ≥2 points yields a well-defined curve
//! over its whole duration.
//!
//! The audio thread only ever calls `SplineSampler::sample` — the
//! sampler is `Send + Sync` and held behind an `Arc`, mutated by
//! atomic-swap from the UI thread when a re-record completes.

use serde::{Deserialize, Serialize};
use splines::{Interpolation, Key, Spline};

/// One captured (or hand-edited) point. Time is in project-relative
/// seconds; gain is in dB.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct AutomationPoint {
    pub time_secs: f32,
    pub gain_db: f32,
}

/// A track's (or the master's) automation lane. Points are kept sorted
/// by `time_secs` — the recorder enforces it on push.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AutomationLane {
    pub points: Vec<AutomationPoint>,
}

impl AutomationLane {
    #[allow(dead_code)] // public API kept for Phase-3 (lane editor / programmatic builds)
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    #[allow(dead_code)] // exposed for the lane editor in Phase-3 polish
    pub fn duration_secs(&self) -> f32 {
        self.points.last().map(|p| p.time_secs).unwrap_or(0.0)
    }

    /// Push a new point if it differs from the last by at least
    /// `delta_db` AND the timestamp is strictly increasing. Returns
    /// `true` if the point was kept.
    pub fn record_point(&mut self, t: f32, gain_db: f32, delta_db: f32) -> bool {
        if let Some(last) = self.points.last() {
            if t <= last.time_secs {
                return false;
            }
            if (gain_db - last.gain_db).abs() < delta_db && t - last.time_secs < 0.5 {
                return false;
            }
        }
        self.points.push(AutomationPoint {
            time_secs: t,
            gain_db,
        });
        true
    }
}

/// Audio-thread-readable spline sampler. Cheap to clone (it's a thin
/// wrapper around an `Arc<Spline>`); built on the UI thread when the
/// lane changes and shipped to the player via an atomic swap.
#[derive(Default)]
pub struct SplineSampler {
    inner: Option<Spline<f32, f32>>,
    /// Constant fallback when there's nothing to interpolate.
    flat: Option<f32>,
}

impl SplineSampler {
    pub fn build(lane: &AutomationLane) -> Self {
        match lane.points.len() {
            0 => Self {
                inner: None,
                flat: None,
            },
            1 => Self {
                inner: None,
                flat: Some(lane.points[0].gain_db),
            },
            _ => {
                // Pad endpoints so Catmull-Rom always has 4 keys to work
                // with, regardless of where in the lane we sample.
                let first = lane.points.first().copied().unwrap();
                let last = lane.points.last().copied().unwrap();
                let mut keys: Vec<Key<f32, f32>> = Vec::with_capacity(lane.points.len() + 2);
                keys.push(Key::new(
                    first.time_secs - 1.0,
                    first.gain_db,
                    Interpolation::CatmullRom,
                ));
                for p in &lane.points {
                    keys.push(Key::new(p.time_secs, p.gain_db, Interpolation::CatmullRom));
                }
                keys.push(Key::new(
                    last.time_secs + 1.0,
                    last.gain_db,
                    Interpolation::CatmullRom,
                ));
                Self {
                    inner: Some(Spline::from_vec(keys)),
                    flat: None,
                }
            }
        }
    }

    /// Interpolated gain at `t` (seconds). `None` means "no automation
    /// here — fall back to the static `track.gain_db`".
    pub fn sample(&self, t: f32) -> Option<f32> {
        if let Some(g) = self.flat {
            return Some(g);
        }
        self.inner.as_ref()?.sample(t)
    }

    /// True when the sampler will never return `Some` regardless of
    /// time. The audio callback uses this to skip the per-frame
    /// `sample()` call (cheap on its own, but lets us avoid a branch
    /// and a `db_to_lin` call when there's no automation at all).
    pub fn is_empty(&self) -> bool {
        self.inner.is_none() && self.flat.is_none()
    }
}

/// UI-thread recorder state. Lives on `app` while a recording is in
/// progress; flushed into the project on Stop.
#[derive(Debug, Default)]
pub struct Recorder {
    /// Per-track scratch lanes keyed by track index. Created on first
    /// captured point; drained on Stop.
    pub track_scratch: std::collections::HashMap<usize, AutomationLane>,
    pub master_scratch: AutomationLane,
}

impl Recorder {
    pub fn record_track(&mut self, idx: usize, t: f32, gain_db: f32) {
        self.track_scratch
            .entry(idx)
            .or_default()
            .record_point(t, gain_db, 0.05);
    }
    pub fn record_master(&mut self, t: f32, gain_db: f32) {
        self.master_scratch.record_point(t, gain_db, 0.05);
    }
    #[allow(dead_code)] // used by future "discard all in-flight" UX
    pub fn clear(&mut self) {
        self.track_scratch.clear();
        self.master_scratch = AutomationLane::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(points: &[(f32, f32)]) -> AutomationLane {
        AutomationLane {
            points: points
                .iter()
                .map(|&(t, g)| AutomationPoint {
                    time_secs: t,
                    gain_db: g,
                })
                .collect(),
        }
    }

    #[test]
    fn empty_lane_samples_to_none() {
        let s = SplineSampler::build(&AutomationLane::default());
        assert!(s.is_empty());
        assert_eq!(s.sample(0.0), None);
        assert_eq!(s.sample(99.0), None);
    }

    #[test]
    fn single_point_lane_is_constant() {
        let s = SplineSampler::build(&lane(&[(2.5, -3.0)]));
        assert!(!s.is_empty());
        // Returns the single value at any t.
        assert_eq!(s.sample(0.0), Some(-3.0));
        assert_eq!(s.sample(2.5), Some(-3.0));
        assert_eq!(s.sample(99.0), Some(-3.0));
    }

    #[test]
    fn two_point_lane_interpolates_smoothly() {
        let s = SplineSampler::build(&lane(&[(0.0, 0.0), (1.0, -6.0)]));
        let mid = s.sample(0.5).expect("interior point should sample");
        // Catmull-Rom on (0,0) → (1,-6) with mirrored endpoints should
        // give roughly linear behaviour through the interior — at most
        // a small overshoot.
        assert!(mid < 0.0, "midpoint should head toward -6 dB");
        assert!(mid > -6.0, "midpoint shouldn't overshoot the endpoint");
    }

    #[test]
    fn record_point_decimates() {
        let mut l = AutomationLane::default();
        // First point always kept.
        assert!(l.record_point(0.0, -3.0, 0.05));
        // Same time → rejected.
        assert!(!l.record_point(0.0, -3.5, 0.05));
        // Same gain (within delta) AND short interval → rejected.
        assert!(!l.record_point(0.01, -2.99, 0.05));
        // Big gain change → kept.
        assert!(l.record_point(0.02, -1.0, 0.05));
        // Long interval keeps the point even if gain is similar.
        assert!(l.record_point(1.0, -1.01, 0.05));

        assert_eq!(l.points.len(), 3);
        assert_eq!(l.points[0].time_secs, 0.0);
        assert_eq!(l.points[1].time_secs, 0.02);
        assert_eq!(l.points[2].time_secs, 1.0);
    }

    #[test]
    fn record_point_strict_time_increase() {
        let mut l = AutomationLane::default();
        l.record_point(1.0, 0.0, 0.05);
        // Time goes backward — must reject.
        assert!(!l.record_point(0.5, 0.0, 0.05));
    }
}
