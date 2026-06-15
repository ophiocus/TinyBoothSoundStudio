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
pub struct CrossfadeSpec<'a> {
    pub a_samples: &'a [f32],
    pub b_samples: &'a [f32],
    pub sample_rate: u32,
    /// Frame offset of B relative to A's frame 0. Negative means B
    /// starts before A. The timeline runs `[min(0, b_offset),
    /// max(a_frames, b_offset + b_frames))`.
    pub b_offset_frames: i64,
    pub curve: CrossfadeCurve,
}

/// Output of [`compute_mix`] — stereo-interleaved samples plus the
/// resolved overlap range (in frames, on the output timeline) so the
/// UI can shade and draw the curve.
pub struct CrossfadeMix {
    pub samples: Vec<f32>,
    /// Output-timeline first frame of the overlap. `None` if the two
    /// tracks don't overlap (B's offset placed it entirely before or
    /// after A). Read by the UI tests; the live UI computes the
    /// overlap directly from the offset slider for render.
    #[allow(dead_code)]
    pub overlap_start_frame: Option<u64>,
    #[allow(dead_code)]
    pub overlap_end_frame: Option<u64>,
    pub sample_rate: u32,
}

/// Compute the mixed crossfade timeline. Stereo output regardless of
/// input layout (callers convert mono → stereo by duplication before
/// calling). Outside the overlap region each track contributes
/// unchanged; inside it the chosen curve attenuates A and ramps in B.
pub fn compute_mix(spec: &CrossfadeSpec<'_>) -> CrossfadeMix {
    let a_frames = (spec.a_samples.len() / 2) as i64;
    let b_frames = (spec.b_samples.len() / 2) as i64;

    // Timeline anchor: t=0 is whichever starts first.
    let a_start = 0_i64;
    let a_end = a_frames;
    let b_start = spec.b_offset_frames;
    let b_end = spec.b_offset_frames + b_frames;
    let timeline_start = a_start.min(b_start);
    let timeline_end = a_end.max(b_end);
    let timeline_frames = (timeline_end - timeline_start).max(0) as usize;

    // Overlap on the timeline coordinate (after shifting so frame 0
    // is timeline_start).
    let shift = -timeline_start;
    let a_local_start = a_start + shift; // == -timeline_start
    let a_local_end = a_end + shift;
    let b_local_start = b_start + shift;
    let b_local_end = b_end + shift;
    let overlap_start = a_local_start.max(b_local_start);
    let overlap_end = a_local_end.min(b_local_end);

    let overlap_present = overlap_end > overlap_start && overlap_start >= 0;
    let overlap_len = if overlap_present {
        (overlap_end - overlap_start) as u64
    } else {
        0
    };

    let mut out = vec![0.0_f32; timeline_frames * 2];

    for n in 0..timeline_frames as i64 {
        // Resolve which inputs are live at this output frame.
        let a_i = n - a_local_start;
        let b_i = n - b_local_start;
        let in_a = a_i >= 0 && a_i < a_frames;
        let in_b = b_i >= 0 && b_i < b_frames;
        if !in_a && !in_b {
            continue;
        }
        // Crossfade weights inside the overlap, otherwise 1.0 for the
        // present track.
        let (wa, wb) =
            if overlap_present && n >= overlap_start && n < overlap_end && overlap_len > 0 {
                // t in [0, 1] across the overlap.
                let t = (n - overlap_start) as f32 / overlap_len as f32;
                match spec.curve {
                    CrossfadeCurve::EqualPower => {
                        let arg = PI * t * 0.5;
                        let ca = arg.cos();
                        let sa = arg.sin();
                        // `cos²` / `sin²` — sums to 1 in power.
                        (ca * ca, sa * sa)
                    }
                    CrossfadeCurve::Linear => (1.0 - t, t),
                }
            } else {
                (if in_a { 1.0 } else { 0.0 }, if in_b { 1.0 } else { 0.0 })
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

    CrossfadeMix {
        samples: out,
        overlap_start_frame: if overlap_present {
            Some(overlap_start as u64)
        } else {
            None
        },
        overlap_end_frame: if overlap_present {
            Some(overlap_end as u64)
        } else {
            None
        },
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

    #[test]
    fn no_overlap_concatenates_without_blend() {
        // A is 100 frames of 0.5, B is 100 frames of -0.5, offset 200.
        let a = stereo_const(100, 0.5);
        let b = stereo_const(100, -0.5);
        let spec = CrossfadeSpec {
            a_samples: &a,
            b_samples: &b,
            sample_rate: 48_000,
            b_offset_frames: 200,
            curve: CrossfadeCurve::EqualPower,
        };
        let mix = compute_mix(&spec);
        assert_eq!(mix.overlap_start_frame, None);
        assert_eq!(mix.samples.len(), 300 * 2);
        // First 100 frames carry A.
        assert!((mix.samples[0] - 0.5).abs() < 1e-6);
        // Gap (frames 100..200) is silent.
        assert_eq!(mix.samples[150 * 2], 0.0);
        // Last 100 frames carry B.
        assert!((mix.samples[250 * 2] - (-0.5)).abs() < 1e-6);
    }

    #[test]
    fn equal_power_weights_sum_to_one_in_power() {
        // 200 frames of A and B, B starts at frame 100 → 100 frames overlap.
        let a = stereo_const(200, 1.0);
        let b = stereo_const(200, 1.0);
        let spec = CrossfadeSpec {
            a_samples: &a,
            b_samples: &b,
            sample_rate: 48_000,
            b_offset_frames: 100,
            curve: CrossfadeCurve::EqualPower,
        };
        let mix = compute_mix(&spec);
        assert_eq!(mix.overlap_start_frame, Some(100));
        assert_eq!(mix.overlap_end_frame, Some(200));
        // Inside the overlap: each output sample is wa·1 + wb·1, and
        // wa² + wb² == 1 for equal-power → wa + wb ranges in [1, √2].
        // The exact value at the centre (t=0.5) is sin²(π/4) + cos²(π/4)
        // = 0.5 + 0.5 = 1.0.
        let centre = mix.samples[150 * 2];
        assert!(
            (centre - 1.0).abs() < 1e-5,
            "centre of equal-power crossfade should be 1.0, got {centre}"
        );
        // Power sum across the whole overlap region is constant at 1.0
        // (both inputs are unit signals).
        for n in 100..200 {
            let s = mix.samples[n * 2];
            // For two unit inputs, s = wa + wb. Power-sum constraint:
            // wa² + wb² = 1. Therefore s² ≤ 2 (achieved at the centre).
            assert!((0.999..=1.4143).contains(&s));
        }
    }

    #[test]
    fn b_offset_negative_shifts_timeline_origin() {
        let a = stereo_const(50, 0.5);
        let b = stereo_const(50, 0.5);
        let spec = CrossfadeSpec {
            a_samples: &a,
            b_samples: &b,
            sample_rate: 48_000,
            b_offset_frames: -20, // B starts 20 frames before A
            curve: CrossfadeCurve::EqualPower,
        };
        let mix = compute_mix(&spec);
        // Timeline spans frame -20..50 = 70 frames total.
        assert_eq!(mix.samples.len(), 70 * 2);
        // Frame 0 of the output is B alone (A hasn't started).
        assert!((mix.samples[0] - 0.5).abs() < 1e-6);
        // Overlap region is [20, 50) → 30 frames.
        assert_eq!(mix.overlap_start_frame, Some(20));
        assert_eq!(mix.overlap_end_frame, Some(50));
    }

    #[test]
    fn silent_inputs_produce_silence() {
        let a = stereo_const(100, 0.0);
        let b = stereo_const(100, 0.0);
        let spec = CrossfadeSpec {
            a_samples: &a,
            b_samples: &b,
            sample_rate: 48_000,
            b_offset_frames: 50,
            curve: CrossfadeCurve::EqualPower,
        };
        let mix = compute_mix(&spec);
        assert!(mix.samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn linear_curve_sums_amplitudes_to_one() {
        let a = stereo_const(100, 1.0);
        let b = stereo_const(100, 1.0);
        let spec = CrossfadeSpec {
            a_samples: &a,
            b_samples: &b,
            sample_rate: 48_000,
            b_offset_frames: 0, // total overlap
            curve: CrossfadeCurve::Linear,
        };
        let mix = compute_mix(&spec);
        // Linear curve: at every frame inside the overlap, wa + wb = 1.
        // For two unit inputs, output sample is always 1.0.
        for n in 0..100 {
            assert!((mix.samples[n * 2] - 1.0).abs() < 1e-5);
        }
    }
}
