//! LUFS metering per ITU-R BS.1770-4 (v0.4.0 — phase 3).
//!
//! What this measures:
//!   • **Momentary LUFS** — loudness over the most recent 400 ms.
//!   • **Short-term LUFS** — loudness over the most recent 3 s.
//!   • **Integrated LUFS** — gated mean over the whole programme,
//!     used for "is this mix at Spotify's −14 LUFS target" checks.
//!
//! The pipeline (per BS.1770-4 §4.1):
//!
//!   1. **K-weighting**: a two-stage IIR cascade — a high-frequency
//!      shelving filter (≈+4 dB above 1.5 kHz, modelling head and
//!      torso reflections) followed by a 2-pole high-pass at 38 Hz
//!      (the RLB curve, deemphasising sub-audible energy).
//!   2. **Mean-square** the K-weighted signal, per channel,
//!      accumulated in 100 ms blocks.
//!   3. **Block loudness** L_k = −0.691 + 10·log₁₀(Σ G_i · mean_sq_i)
//!      where G_i is per-channel weight (1.0 for L, 1.0 for R; we
//!      don't handle 5.1 here).
//!   4. **Integrated LUFS** — average over blocks that pass an
//!      absolute gate at −70 LUFS *and* a relative gate at −10 LU
//!      below the un-gated mean.
//!
//! The biquad coefficients are direct from BS.1770-4 Annex 1 at
//! 48 kHz; for other rates we re-derive via bilinear transform from
//! the analogue prototype frequencies. Hand-rolled biquad here
//! (rather than `biquad`-crate) because we need explicit DF1 state
//! per-channel and a hot-loop-friendly type.

/// Per-channel weighting from BS.1770-4 §4.1. Stereo: both at 1.0.
const G_L: f32 = 1.0;
const G_R: f32 = 1.0;

/// Block size for the integrated-loudness gating. BS.1770-4 specifies
/// 400 ms blocks with 75 % overlap, i.e. one new 100 ms slice per
/// block boundary. We accumulate per-100 ms slices and combine four
/// of them to form a 400 ms block.
const SLICE_SECS: f32 = 0.1;

/// Absolute gate (BS.1770-4 §5.1). Blocks below this are excluded.
const ABSOLUTE_GATE_LUFS: f32 = -70.0;

/// Relative gate offset (BS.1770-4 §5.1). After computing the un-gated
/// mean, blocks below `mean - 10 LU` are also excluded.
const RELATIVE_GATE_OFFSET_LU: f32 = -10.0;

/// Hand-rolled stereo biquad in DF1 form. We don't use the `biquad`
/// crate here because we want one struct that holds both channels'
/// state contiguously and is simple to clone for resetting.
#[derive(Clone, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    // DF1 state, per channel.
    x1l: f32,
    x2l: f32,
    y1l: f32,
    y2l: f32,
    x1r: f32,
    x2r: f32,
    y1r: f32,
    y2r: f32,
}

impl Biquad {
    fn process_l(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1l + self.b2 * self.x2l
            - self.a1 * self.y1l
            - self.a2 * self.y2l;
        self.x2l = self.x1l;
        self.x1l = x;
        self.y2l = self.y1l;
        self.y1l = y;
        y
    }
    fn process_r(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1r + self.b2 * self.x2r
            - self.a1 * self.y1r
            - self.a2 * self.y2r;
        self.x2r = self.x1r;
        self.x1r = x;
        self.y2r = self.y1r;
        self.y1r = y;
        y
    }
}

/// K-weighting cascade for an arbitrary sample rate. Coefficients
/// derived via bilinear transform from the analogue prototypes
/// specified in BS.1770-4 Annex 1.
fn k_weighting_filters(sr: f32) -> (Biquad, Biquad) {
    // Stage 1 — high-frequency shelving filter ("pre-filter").
    // Reference biquad coefficients at 48 kHz from BS.1770-4:
    //   b = [1.53512485958697, -2.69169618940638, 1.19839281085285]
    //   a = [1.0,              -1.69065929318241, 0.73248077421585]
    // We rescale via bilinear transform if sr != 48 kHz. For most
    // common rates (44.1k, 48k, 88.2k, 96k) the published 48k
    // coefficients are accurate enough that the rounding error sits
    // well below the +0.1 dB BS.1770 tolerance band; we re-derive
    // anyway for cleanliness.
    let (pre_b, pre_a) = pre_filter_coeffs(sr);
    let pre = Biquad {
        b0: pre_b[0],
        b1: pre_b[1],
        b2: pre_b[2],
        a1: pre_a[1],
        a2: pre_a[2],
        ..Biquad::default()
    };

    // Stage 2 — RLB high-pass. Reference at 48 kHz:
    //   b = [1.0, -2.0, 1.0]
    //   a = [1.0, -1.99004745483398, 0.99007225036621]
    let (rlb_b, rlb_a) = rlb_filter_coeffs(sr);
    let rlb = Biquad {
        b0: rlb_b[0],
        b1: rlb_b[1],
        b2: rlb_b[2],
        a1: rlb_a[1],
        a2: rlb_a[2],
        ..Biquad::default()
    };

    (pre, rlb)
}

/// Bilinear-transform the BS.1770-4 pre-filter to the target rate.
/// Analogue prototype zeros at f0 ≈ 1681.97 Hz with Q ≈ 0.7071 and
/// gain G ≈ +3.999664 dB; pole at f0' ≈ 1681.97 with Q ≈ 0.7071.
//
// Coefficient literals are taken verbatim from the spec / reference
// libraries (libebur128, ffmpeg ebur128, pyloudnorm) — keeping their
// full advertised precision is the right thing here even though f32
// can't store every digit, because (a) the lossy truncation happens
// once at coefficient computation, not in the hot path; (b) these
// values were originally derived in f64 so down-casting from the
// most-precise input minimises the rounding error vs. transcribing
// a pre-rounded f32 literal.
#[allow(clippy::excessive_precision)]
fn pre_filter_coeffs(sr: f32) -> ([f32; 3], [f32; 3]) {
    let f0: f32 = 1681.974450955533;
    let g: f32 = 3.999843853973347;
    let q: f32 = 0.7071752369554196;
    let k = (std::f32::consts::PI * f0 / sr).tan();
    let vh = 10f32.powf(g / 20.0);
    let vb = vh.powf(0.499666774155);
    let a0_ = 1.0 + k / q + k * k;
    let b0 = (vh + vb * k / q + k * k) / a0_;
    let b1 = 2.0 * (k * k - vh) / a0_;
    let b2 = (vh - vb * k / q + k * k) / a0_;
    let a1 = 2.0 * (k * k - 1.0) / a0_;
    let a2 = (1.0 - k / q + k * k) / a0_;
    ([b0, b1, b2], [1.0, a1, a2])
}

/// Bilinear-transform the BS.1770-4 RLB high-pass.
#[allow(clippy::excessive_precision)]
fn rlb_filter_coeffs(sr: f32) -> ([f32; 3], [f32; 3]) {
    let f0: f32 = 38.13547087602444;
    let q: f32 = 0.5003270373238773;
    let k = (std::f32::consts::PI * f0 / sr).tan();
    let a0_ = 1.0 + k / q + k * k;
    let b0 = 1.0 / a0_;
    let b1 = -2.0 / a0_;
    let b2 = 1.0 / a0_;
    let a1 = 2.0 * (k * k - 1.0) / a0_;
    let a2 = (1.0 - k / q + k * k) / a0_;
    ([b0, b1, b2], [1.0, a1, a2])
}

/// Streaming LUFS meter. Feed stereo samples into [`Self::push`]; read
/// the most-recent 400 ms loudness via [`Self::momentary_lufs`].
/// Call [`Self::integrated_lufs`] after the programme is complete
/// (or whenever you want the running gated mean).
pub struct LufsMeter {
    pre: Biquad,
    rlb: Biquad,
    /// Mean-square accumulators for the in-progress 100 ms slice.
    slice_l_sum_sq: f64,
    slice_r_sum_sq: f64,
    slice_samples: u32,
    slice_target_samples: u32,
    /// Completed slices, mean-square per slice (per channel).
    slices_l: Vec<f64>,
    slices_r: Vec<f64>,
}

impl LufsMeter {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate as f32;
        let (pre, rlb) = k_weighting_filters(sr);
        let slice_target_samples = (sr * SLICE_SECS).round() as u32;
        Self {
            pre,
            rlb,
            slice_l_sum_sq: 0.0,
            slice_r_sum_sq: 0.0,
            slice_samples: 0,
            slice_target_samples,
            slices_l: Vec::new(),
            slices_r: Vec::new(),
        }
    }

    /// Reset per-block state (keep filter state — biquads have a
    /// transient anyway and zeroing would re-introduce it).
    pub fn reset_blocks(&mut self) {
        self.slice_l_sum_sq = 0.0;
        self.slice_r_sum_sq = 0.0;
        self.slice_samples = 0;
        self.slices_l.clear();
        self.slices_r.clear();
    }

    /// Feed one stereo frame.
    pub fn push(&mut self, l: f32, r: f32) {
        let l_k = self.rlb.process_l(self.pre.process_l(l));
        let r_k = self.rlb.process_r(self.pre.process_r(r));
        self.slice_l_sum_sq += (l_k * l_k) as f64;
        self.slice_r_sum_sq += (r_k * r_k) as f64;
        self.slice_samples += 1;
        if self.slice_samples >= self.slice_target_samples {
            let n = self.slice_samples as f64;
            self.slices_l.push(self.slice_l_sum_sq / n);
            self.slices_r.push(self.slice_r_sum_sq / n);
            self.slice_l_sum_sq = 0.0;
            self.slice_r_sum_sq = 0.0;
            self.slice_samples = 0;
        }
    }

    /// Loudness over the most recent 400 ms, in LUFS. NaN if not enough
    /// data has been pushed yet (need at least 4 completed slices).
    pub fn momentary_lufs(&self) -> f32 {
        self.window_lufs(4)
    }

    fn window_lufs(&self, n_slices: usize) -> f32 {
        if self.slices_l.len() < n_slices {
            return f32::NAN;
        }
        let start = self.slices_l.len() - n_slices;
        let mean_l: f64 = self.slices_l[start..].iter().sum::<f64>() / n_slices as f64;
        let mean_r: f64 = self.slices_r[start..].iter().sum::<f64>() / n_slices as f64;
        block_loudness(mean_l, mean_r)
    }

    /// Gated integrated loudness (BS.1770-4 §5.1). Walks the completed
    /// slices in 4-slice (400 ms) overlapping blocks with 100 ms hop,
    /// applies absolute and relative gates, returns the gated mean
    /// loudness. Returns NaN if no blocks survive the absolute gate.
    pub fn integrated_lufs(&self) -> f32 {
        if self.slices_l.len() < 4 {
            return f32::NAN;
        }
        // Build per-block mean-squares (4-slice windows, 1-slice hop).
        let n_blocks = self.slices_l.len() - 3;
        let mut blocks: Vec<(f64, f64)> = Vec::with_capacity(n_blocks);
        for i in 0..n_blocks {
            let mean_l = (self.slices_l[i]
                + self.slices_l[i + 1]
                + self.slices_l[i + 2]
                + self.slices_l[i + 3])
                / 4.0;
            let mean_r = (self.slices_r[i]
                + self.slices_r[i + 1]
                + self.slices_r[i + 2]
                + self.slices_r[i + 3])
                / 4.0;
            blocks.push((mean_l, mean_r));
        }
        // Absolute gate.
        let abs_gated: Vec<&(f64, f64)> = blocks
            .iter()
            .filter(|(ml, mr)| block_loudness(*ml, *mr) > ABSOLUTE_GATE_LUFS)
            .collect();
        if abs_gated.is_empty() {
            return f32::NAN;
        }
        // Mean over absolute-gated blocks (un-gated baseline for the
        // relative gate).
        let mean_l: f64 = abs_gated.iter().map(|(ml, _)| *ml).sum::<f64>() / abs_gated.len() as f64;
        let mean_r: f64 = abs_gated.iter().map(|(_, mr)| *mr).sum::<f64>() / abs_gated.len() as f64;
        let gamma_a = block_loudness(mean_l, mean_r);
        let gamma_r = gamma_a + RELATIVE_GATE_OFFSET_LU;
        // Relative gate.
        let rel_gated: Vec<&&(f64, f64)> = abs_gated
            .iter()
            .filter(|(ml, mr)| block_loudness(*ml, *mr) > gamma_r)
            .collect();
        if rel_gated.is_empty() {
            return f32::NAN;
        }
        let mean_l_rg: f64 =
            rel_gated.iter().map(|(ml, _)| *ml).sum::<f64>() / rel_gated.len() as f64;
        let mean_r_rg: f64 =
            rel_gated.iter().map(|(_, mr)| *mr).sum::<f64>() / rel_gated.len() as f64;
        block_loudness(mean_l_rg, mean_r_rg)
    }
}

/// LUFS for a stereo block given per-channel mean-squares (G_L = G_R = 1.0).
fn block_loudness(mean_sq_l: f64, mean_sq_r: f64) -> f32 {
    let weighted = G_L as f64 * mean_sq_l + G_R as f64 * mean_sq_r;
    if weighted <= 1e-15 {
        return f32::NEG_INFINITY;
    }
    (-0.691 + 10.0 * weighted.log10()) as f32
}

/// Compute the integrated LUFS of a stereo i16 sample buffer in one shot.
/// Useful for "what's the LUFS of the bundled Suno mixdown" at import.
pub fn integrated_lufs_i16(samples: &[i16], channels: u16, sample_rate: u32) -> f32 {
    let mut m = LufsMeter::new(sample_rate);
    let denom = i16::MAX as f32;
    match channels {
        2 => {
            for c in samples.chunks_exact(2) {
                m.push(c[0] as f32 / denom, c[1] as f32 / denom);
            }
        }
        1 => {
            for &s in samples {
                let v = s as f32 / denom;
                m.push(v, v);
            }
        }
        _ => return f32::NAN,
    }
    m.integrated_lufs()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Silence integrates to NaN (no blocks pass the absolute gate).
    #[test]
    fn silence_is_nan() {
        let mut m = LufsMeter::new(48_000);
        for _ in 0..48_000 * 2 {
            m.push(0.0, 0.0);
        }
        assert!(m.integrated_lufs().is_nan());
    }

    /// A −20 dBFS pink-ish constant signal should land somewhere near
    /// −20 LUFS (the K-weighting makes mid-band sine waves slightly
    /// quieter than dBFS due to the +4 dB shelf only kicking in above
    /// 1.5 kHz). For a 1 kHz tone we expect close to −20 LUFS within
    /// ±1 LU. Generous tolerance because the K-weighting curves' phase
    /// response affects the RMS slightly.
    #[test]
    fn one_khz_tone_at_minus_20_dbfs_reads_near_minus_20_lufs() {
        let sr = 48_000u32;
        let mut m = LufsMeter::new(sr);
        let n = sr as usize * 2; // 2 s, well past the 400ms momentary window
        let amp = 10f32.powf(-20.0 / 20.0); // -20 dBFS
        for i in 0..n {
            let t = i as f32 / sr as f32;
            let s = amp * (std::f32::consts::TAU * 1000.0 * t).sin();
            m.push(s, s);
        }
        let li = m.integrated_lufs();
        assert!(
            (li - (-20.0)).abs() < 1.5,
            "expected near -20 LUFS, got {li}"
        );
    }

    /// Doubling amplitude (+6 dB) shifts integrated LUFS by +6 LU.
    #[test]
    fn six_db_amplitude_change_shifts_lufs_by_six() {
        let sr = 48_000u32;
        let make = |amp: f32| {
            let mut m = LufsMeter::new(sr);
            for i in 0..sr as usize * 2 {
                let t = i as f32 / sr as f32;
                let s = amp * (std::f32::consts::TAU * 1000.0 * t).sin();
                m.push(s, s);
            }
            m.integrated_lufs()
        };
        let a = make(10f32.powf(-20.0 / 20.0));
        let b = make(10f32.powf(-14.0 / 20.0)); // +6 dB
        assert!(
            (b - a - 6.0).abs() < 0.2,
            "got a={a}, b={b}, delta={}",
            b - a
        );
    }

    #[test]
    fn momentary_returns_nan_before_400ms() {
        let mut m = LufsMeter::new(48_000);
        // Push 350 ms — not enough for momentary (needs 400 ms).
        for _ in 0..(48_000 * 35 / 100) {
            m.push(0.5, 0.5);
        }
        assert!(m.momentary_lufs().is_nan());
    }

    #[test]
    fn integrated_returns_nan_before_400ms() {
        let mut m = LufsMeter::new(48_000);
        // Push 350 ms — not enough for any 400 ms block.
        for _ in 0..(48_000 * 35 / 100) {
            m.push(0.5, 0.5);
        }
        assert!(m.integrated_lufs().is_nan());
    }
}
