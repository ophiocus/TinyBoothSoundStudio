//! Crossfade DSP — pure functions. Given two decoded buffers (stereo
//! interleaved f32) at a common sample rate, produce the mixed
//! timeline with track B offset by `b_offset_frames` and an
//! equal-power crossfade across the overlap region.
//!
//! No I/O, no playback, no UI dependency — exercised end-to-end by
//! unit tests. See [TBSS-FR-0010].
//!
//! [TBSS-FR-0010]: ../../docs/feature-requests/TBSS-FR-0010-crossfade-tab.md

use std::f32::consts::PI;

/// Curve choice for the crossfade. MVP ships equal-power only; linear
/// is the architectural slot for the deferred picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossfadeCurve {
    /// `cos²(π·t/(2L))` / `sin²(π·t/(2L))` — sums to 1 in power.
    /// Right default for unrelated material (preserves perceived
    /// loudness through the transition).
    EqualPower,
    /// Linear ramp — sums to 1 in amplitude. Better for phase-
    /// coherent material (two takes of the same source) where the
    /// signals reinforce in the middle. Reserved for the picker.
    #[allow(dead_code)]
    Linear,
}

/// Inputs to the crossfade DSP. Buffers are stereo-interleaved f32
/// (`[L0, R0, L1, R1, …]`) at the same `sample_rate`. Frame counts
/// are inferred from `samples.len() / 2`.
///
/// The mix uses a **transition model** (TBSS-FR-0010 UX pass):
/// `fade_start_frame_abs` and `fade_end_frame_abs` are independent of
/// where the tracks overlap. Before `fade_start`, only A contributes;
/// after `fade_end`, only B contributes; in between, both are mixed
/// via the curve. This lets the user place the transition wherever
/// they want — including outside the tracks' actual overlap range.
///
/// Frame indices are **absolute** (relative to A's frame 0). Negative
/// values mean "before A starts." Output indexing happens internally.
pub struct CrossfadeSpec<'a> {
    pub a_samples: &'a [f32],
    pub b_samples: &'a [f32],
    pub sample_rate: u32,
    /// Absolute frame index where B starts (A starts at 0). Negative
    /// means B starts before A.
    pub b_offset_frames: i64,
    /// Absolute frame index where the fade begins (transition start).
    pub fade_start_frame_abs: i64,
    /// Absolute frame index where the fade ends (transition end).
    /// Must be `>= fade_start_frame_abs`; equality means an instant
    /// cut between A and B.
    pub fade_end_frame_abs: i64,
    pub curve: CrossfadeCurve,
}

/// Output of [`compute_mix`] — stereo-interleaved samples plus the
/// resolved overlap range (in frames, on the output timeline) so the
/// UI can shade and draw the curve.
pub struct CrossfadeMix {
    pub samples: Vec<f32>,
    /// Output-timeline first frame of the fade region. `None` when
    /// the fade range is degenerate (start >= end). The UI uses this
    /// for confirmation; the live render uses the seconds-based state
    /// directly.
    #[allow(dead_code)]
    pub fade_start_frame: Option<u64>,
    #[allow(dead_code)]
    pub fade_end_frame: Option<u64>,
    pub sample_rate: u32,
}

/// Compute the mixed crossfade timeline using the transition model.
/// Stereo output regardless of input layout (callers convert mono →
/// stereo by duplication before calling).
///
/// Weights:
/// - `n < fade_start`: only A contributes (B muted even if present).
/// - `fade_start <= n < fade_end`: both contribute per `curve`.
/// - `n >= fade_end`: only B contributes (A muted even if present).
///
/// `n` is an absolute frame index on the timeline whose origin is
/// `min(0, b_offset)`. All four boundary frames (b_offset, fade_start,
/// fade_end, the implicit a_start=0) are converted to output-frame
/// coordinates internally.
pub fn compute_mix(spec: &CrossfadeSpec<'_>) -> CrossfadeMix {
    let a_frames = (spec.a_samples.len() / 2) as i64;
    let b_frames = (spec.b_samples.len() / 2) as i64;

    let a_start_abs = 0_i64;
    let a_end_abs = a_frames;
    let b_start_abs = spec.b_offset_frames;
    let b_end_abs = spec.b_offset_frames + b_frames;
    let timeline_start = a_start_abs.min(b_start_abs);
    let timeline_end = a_end_abs.max(b_end_abs);
    let timeline_frames = (timeline_end - timeline_start).max(0) as usize;

    // Fade range expressed on the output timeline (origin = timeline_start).
    let fade_start_out = spec.fade_start_frame_abs - timeline_start;
    let fade_end_out = spec.fade_end_frame_abs - timeline_start;
    let fade_len = (fade_end_out - fade_start_out).max(0) as u64;
    let has_fade_range = fade_end_out > fade_start_out;

    let mut out = vec![0.0_f32; timeline_frames * 2];

    for n in 0..timeline_frames as i64 {
        // Output frame n ↔ absolute time (timeline_start + n).
        let a_i = n + timeline_start - a_start_abs;
        let b_i = n + timeline_start - b_start_abs;
        let in_a = a_i >= 0 && a_i < a_frames;
        let in_b = b_i >= 0 && b_i < b_frames;
        if !in_a && !in_b {
            continue;
        }
        // Transition-model weights.
        let (wa, wb) = if !has_fade_range {
            // Degenerate (or no) fade: instant cut at fade_start_out.
            if n < fade_start_out {
                (1.0, 0.0)
            } else {
                (0.0, 1.0)
            }
        } else if n < fade_start_out {
            (1.0, 0.0)
        } else if n >= fade_end_out {
            (0.0, 1.0)
        } else {
            let t = (n - fade_start_out) as f32 / fade_len as f32;
            match spec.curve {
                CrossfadeCurve::EqualPower => {
                    let arg = PI * t * 0.5;
                    let ca = arg.cos();
                    let sa = arg.sin();
                    (ca * ca, sa * sa)
                }
                CrossfadeCurve::Linear => (1.0 - t, t),
            }
        };

        let l_a = if in_a {
            spec.a_samples[(a_i as usize) * 2]
        } else {
            0.0
        };
        let r_a = if in_a {
            spec.a_samples[(a_i as usize) * 2 + 1]
        } else {
            0.0
        };
        let l_b = if in_b {
            spec.b_samples[(b_i as usize) * 2]
        } else {
            0.0
        };
        let r_b = if in_b {
            spec.b_samples[(b_i as usize) * 2 + 1]
        } else {
            0.0
        };

        out[(n as usize) * 2] = wa * l_a + wb * l_b;
        out[(n as usize) * 2 + 1] = wa * r_a + wb * r_b;
    }

    // Surface the fade range on the output timeline so the UI / tests
    // can confirm placement without re-deriving.
    let (fs_out, fe_out) = if has_fade_range {
        (
            Some(fade_start_out.max(0) as u64),
            Some(fade_end_out.max(0) as u64),
        )
    } else {
        (None, None)
    };
    CrossfadeMix {
        samples: out,
        fade_start_frame: fs_out,
        fade_end_frame: fe_out,
        sample_rate: spec.sample_rate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_const(frames: usize, value: f32) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            v.push(value);
            v.push(value);
        }
        v
    }

    /// Equal-power fade across the overlap of two unit-amplitude tracks
    /// — at every frame inside the fade region, wa² + wb² == 1.
    #[test]
    fn equal_power_weights_sum_to_one_in_power() {
        // 200 frames each, B starts at 100, fade spans [100, 200).
        let a = stereo_const(200, 1.0);
        let b = stereo_const(200, 1.0);
        let spec = CrossfadeSpec {
            a_samples: &a,
            b_samples: &b,
            sample_rate: 48_000,
            b_offset_frames: 100,
            fade_start_frame_abs: 100,
            fade_end_frame_abs: 200,
            curve: CrossfadeCurve::EqualPower,
        };
        let mix = compute_mix(&spec);
        assert_eq!(mix.fade_start_frame, Some(100));
        assert_eq!(mix.fade_end_frame, Some(200));
        // Centre of an equal-power crossfade of two equal-amplitude
        // unit inputs: wa = wb = 0.5 → output = 1.0.
        let centre = mix.samples[150 * 2];
        assert!((centre - 1.0).abs() < 1e-5);
        // For unit inputs A and B, the output amplitude varies from
        // 1.0 (at the edges) up to √2 (≈1.414, at the centre).
        for n in 100..200 {
            let s = mix.samples[n * 2];
            assert!((0.999..=1.4143).contains(&s));
        }
    }

    /// Linear curve over identical unit tracks: output is constant 1.0.
    #[test]
    fn linear_curve_sums_amplitudes_to_one() {
        let a = stereo_const(100, 1.0);
        let b = stereo_const(100, 1.0);
        let spec = CrossfadeSpec {
            a_samples: &a,
            b_samples: &b,
            sample_rate: 48_000,
            b_offset_frames: 0,
            fade_start_frame_abs: 0,
            fade_end_frame_abs: 100,
            curve: CrossfadeCurve::Linear,
        };
        let mix = compute_mix(&spec);
        for n in 0..100 {
            assert!((mix.samples[n * 2] - 1.0).abs() < 1e-5);
        }
    }

    /// Silent inputs always produce silent output regardless of fade.
    #[test]
    fn silent_inputs_produce_silence() {
        let a = stereo_const(100, 0.0);
        let b = stereo_const(100, 0.0);
        let spec = CrossfadeSpec {
            a_samples: &a,
            b_samples: &b,
            sample_rate: 48_000,
            b_offset_frames: 50,
            fade_start_frame_abs: 50,
            fade_end_frame_abs: 100,
            curve: CrossfadeCurve::EqualPower,
        };
        let mix = compute_mix(&spec);
        assert!(mix.samples.iter().all(|&s| s == 0.0));
    }

    /// Transition model: before `fade_start` only A contributes; after
    /// `fade_end` only B. Even when both tracks have audio in those
    /// regions, the other is muted — this is what makes the crossfade
    /// a clean transition.
    #[test]
    fn transition_model_mutes_non_active_track_outside_fade() {
        // Both tracks fully overlap (0..100) but the fade is the small
        // window [40, 60). Before 40 only A plays even though B is live;
        // after 60 only B plays even though A is live.
        let a = stereo_const(100, 0.5);
        let b = stereo_const(100, -0.5);
        let spec = CrossfadeSpec {
            a_samples: &a,
            b_samples: &b,
            sample_rate: 48_000,
            b_offset_frames: 0,
            fade_start_frame_abs: 40,
            fade_end_frame_abs: 60,
            curve: CrossfadeCurve::EqualPower,
        };
        let mix = compute_mix(&spec);
        // Frame 10: A only.
        assert!((mix.samples[10 * 2] - 0.5).abs() < 1e-6);
        // Frame 80: B only (A muted).
        assert!((mix.samples[80 * 2] - (-0.5)).abs() < 1e-6);
        // Frame 50 (centre of fade): wa=wb=0.5 of equal-power → 0.5·0.5 + 0.5·(-0.5) = 0.
        let centre = mix.samples[50 * 2];
        assert!(
            centre.abs() < 1e-5,
            "centre of equal-amplitude opposite-sign fade should be ≈0, got {centre}"
        );
    }

    /// Zero-length fade range collapses to an instant cut at
    /// `fade_start`.
    #[test]
    fn zero_length_fade_is_instant_cut() {
        let a = stereo_const(100, 0.7);
        let b = stereo_const(100, -0.3);
        let spec = CrossfadeSpec {
            a_samples: &a,
            b_samples: &b,
            sample_rate: 48_000,
            b_offset_frames: 0,
            fade_start_frame_abs: 50,
            fade_end_frame_abs: 50, // zero-length
            curve: CrossfadeCurve::EqualPower,
        };
        let mix = compute_mix(&spec);
        // Before 50: A.
        assert!((mix.samples[40 * 2] - 0.7).abs() < 1e-6);
        // At and after 50: B.
        assert!((mix.samples[50 * 2] - (-0.3)).abs() < 1e-6);
        assert!((mix.samples[60 * 2] - (-0.3)).abs() < 1e-6);
    }
}
