//! Suno-import coherence analysis (v0.4.0 — phase 2).
//!
//! Verifies that imported stems actually compose into the bundled Suno
//! mixdown:
//!
//!   1. Sum all stems at unity gain.
//!   2. Subtract the mixdown.
//!   3. Compute the residual's RMS relative to the mixdown's RMS — the
//!      "coherence score" surfaced in the import-result modal. Below
//!      ~−30 dB means stems and mixdown roughly compose; above ~−20 dB
//!      means a stem is missing, mislabelled, polarity-flipped, or
//!      length-mismatched.
//!   4. For each stem, the Pearson correlation with the mixdown across
//!      its active region. Strongly-negative correlation flags a stem
//!      that should probably be polarity-flipped.
//!
//! Analysis runs on a decimated f32-mono signal at ~`ANALYSIS_HZ` Hz —
//! fine for integral measurements like RMS and correlation, and keeps
//! memory bounded regardless of song length. A 3-min song at 4 kHz is
//! ~720 k samples per buffer.

use anyhow::{Context, Result};
use std::path::Path;

/// Decimation target for the analysis pass. RMS and correlation are
/// integral measurements so we don't need full bandwidth — 4 kHz is
/// plenty of resolution for "do these stems sum to the mixdown".
const ANALYSIS_HZ: u32 = 4_000;

/// Threshold below which we flag a stem as needing a polarity flip.
/// Pearson r < this value ⇒ "anti-phase relative to the mixdown".
/// Tuned conservatively: a clean coherent stem typically reads r > 0.3
/// against the mixdown; an inverted one reads r < −0.3. Values between
/// the thresholds get no badge in either direction.
const POLARITY_FLIP_THRESHOLD: f32 = -0.3;

/// What the import surfaces to the user.
#[derive(Debug, Clone)]
pub struct CoherenceReport {
    /// Mixdown RMS in dBFS. Ballpark for context.
    pub mixdown_rms_db: f32,
    /// Residual (stems_sum − mixdown) RMS in dBFS.
    pub residual_rms_db: f32,
    /// `residual_rms_db − mixdown_rms_db`. Negative = stems compose well.
    /// −30 dB or below is "coherent"; above −20 dB is "something is off".
    pub relative_db: f32,
    /// Per-stem flags. Index matches the order stems were passed in.
    pub stems: Vec<StemCoherence>,
}

#[derive(Debug, Clone)]
pub struct StemCoherence {
    pub display_name: String,
    /// Pearson correlation with the mixdown, in `[-1, 1]`. Negative
    /// values mean the stem moves opposite to the mixdown — usually
    /// polarity-inverted.
    pub correlation: f32,
    /// Heuristic recommendation: should this stem get a polarity flip?
    pub suggests_polarity_flip: bool,
}

impl CoherenceReport {
    /// One-line human-readable summary for the import-result modal.
    pub fn summary_line(&self) -> String {
        let verdict = if self.relative_db <= -30.0 {
            "stems compose cleanly into the mixdown"
        } else if self.relative_db <= -20.0 {
            "stems mostly compose — minor residual"
        } else if self.relative_db <= -10.0 {
            "noticeable residual — a stem may be missing or anti-phase"
        } else {
            "large residual — stems do NOT compose into the mixdown"
        };
        format!(
            "Coherence: residual {:.1} dB below mixdown ({verdict}).",
            -self.relative_db
        )
    }

    /// Per-stem flag list, just the ones that flagged.
    pub fn flagged_stems(&self) -> Vec<&StemCoherence> {
        self.stems
            .iter()
            .filter(|s| s.suggests_polarity_flip)
            .collect()
    }
}

/// Run the coherence analysis. `stems` is `(display_name, wav_path)` for
/// each stem track on disk. `mixdown_path` is the bundled Suno mixdown.
///
/// Returns an error only if the mixdown itself fails to load. Stems that
/// fail to load are skipped with a zero-correlation entry so the import
/// proceeds; this is a diagnostic, not a gate.
pub fn report(stems: &[(String, &Path)], mixdown_path: &Path) -> Result<CoherenceReport> {
    let mixdown = load_decimated(mixdown_path)
        .with_context(|| format!("loading mixdown {}", mixdown_path.display()))?;
    let mut stems_sum = vec![0.0_f32; mixdown.len()];
    let mut per_stem = Vec::with_capacity(stems.len());
    for (name, path) in stems {
        match load_decimated(path) {
            Ok(stem) => {
                let len = mixdown.len().min(stem.len());
                for (i, s) in stem.iter().take(len).enumerate() {
                    stems_sum[i] += s;
                }
                let corr = pearson_correlation(&stem[..len], &mixdown[..len]);
                per_stem.push(StemCoherence {
                    display_name: name.clone(),
                    correlation: corr,
                    suggests_polarity_flip: corr < POLARITY_FLIP_THRESHOLD,
                });
            }
            Err(_) => {
                // Don't fail the whole report — surface the gap as a zero entry.
                per_stem.push(StemCoherence {
                    display_name: name.clone(),
                    correlation: 0.0,
                    suggests_polarity_flip: false,
                });
            }
        }
    }
    let residual: Vec<f32> = (0..mixdown.len())
        .map(|i| stems_sum[i] - mixdown[i])
        .collect();
    let mix_rms = rms(&mixdown);
    let res_rms = rms(&residual);
    let mix_db = lin_to_db(mix_rms);
    let res_db = lin_to_db(res_rms);
    Ok(CoherenceReport {
        mixdown_rms_db: mix_db,
        residual_rms_db: res_db,
        relative_db: res_db - mix_db,
        stems: per_stem,
    })
}

/// Load a WAV file as decimated f32 mono. Channel-sums to mono first
/// (mean of L+R for stereo), then integer-decimates the sample rate to
/// roughly [`ANALYSIS_HZ`]. Stems whose source rate isn't an integer
/// multiple of the analysis rate get the closest integer factor.
fn load_decimated(path: &Path) -> Result<Vec<f32>> {
    let mut reader =
        hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spec = reader.spec();
    let denom = i16::MAX as f32;
    let mono: Vec<f32> = match spec.channels {
        1 => reader
            .samples::<i16>()
            .map(|s| s.unwrap_or(0) as f32 / denom)
            .collect(),
        2 => {
            let raw: Vec<i16> = reader.samples::<i16>().filter_map(|s| s.ok()).collect();
            raw.chunks_exact(2)
                .map(|c| (c[0] as f32 + c[1] as f32) / (2.0 * denom))
                .collect()
        }
        n => anyhow::bail!("unsupported channel count: {n}"),
    };
    let factor = (spec.sample_rate / ANALYSIS_HZ).max(1) as usize;
    if factor == 1 {
        return Ok(mono);
    }
    let mut out = Vec::with_capacity(mono.len() / factor + 1);
    for chunk in mono.chunks(factor) {
        let avg = chunk.iter().sum::<f32>() / chunk.len() as f32;
        out.push(avg);
    }
    Ok(out)
}

fn rms(s: &[f32]) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = s.iter().map(|x| x * x).sum();
    (sum_sq / s.len() as f32).sqrt()
}

/// Pearson correlation coefficient over the `min(a.len(), b.len())`
/// leading samples. Returns 0.0 for degenerate inputs (empty or constant).
fn pearson_correlation(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    let inv_n = 1.0 / n as f32;
    let mean_a: f32 = a.iter().take(n).sum::<f32>() * inv_n;
    let mean_b: f32 = b.iter().take(n).sum::<f32>() * inv_n;
    let mut num = 0.0;
    let mut den_a = 0.0;
    let mut den_b = 0.0;
    for i in 0..n {
        let da = a[i] - mean_a;
        let db = b[i] - mean_b;
        num += da * db;
        den_a += da * da;
        den_b += db * db;
    }
    let den = (den_a * den_b).sqrt();
    if den < 1e-9 {
        0.0
    } else {
        num / den
    }
}

fn lin_to_db(lin: f32) -> f32 {
    20.0 * lin.max(1e-9).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_of_empty_is_zero() {
        assert_eq!(rms(&[]), 0.0);
    }

    #[test]
    fn rms_of_dc_signal_equals_amplitude() {
        let s = vec![0.5; 1000];
        assert!((rms(&s) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn correlation_of_identical_signals_is_one() {
        let s: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin()).collect();
        let r = pearson_correlation(&s, &s);
        assert!((r - 1.0).abs() < 1e-3, "got r = {r}");
    }

    #[test]
    fn correlation_of_inverted_signals_is_negative_one() {
        let a: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin()).collect();
        let b: Vec<f32> = a.iter().map(|x| -x).collect();
        let r = pearson_correlation(&a, &b);
        assert!((r + 1.0).abs() < 1e-3, "got r = {r}");
    }

    #[test]
    fn correlation_of_orthogonal_sines_is_near_zero() {
        // Discrete orthogonality of sin(2π·k·n/N) for integer k over n=0..N
        // — k=5 vs k=7 in a 1000-sample window has zero inner product.
        let n: usize = 1000;
        let a: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * 5.0 * i as f32 / n as f32).sin())
            .collect();
        let b: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * 7.0 * i as f32 / n as f32).sin())
            .collect();
        let r = pearson_correlation(&a, &b);
        assert!(r.abs() < 0.05, "expected near zero, got r = {r}");
    }

    #[test]
    fn summary_line_categorises_relative_db() {
        let cases = [
            (-35.0, "compose cleanly"),
            (-25.0, "mostly compose"),
            (-15.0, "noticeable residual"),
            (-5.0, "large residual"),
        ];
        for (rel_db, expected_substr) in cases {
            let r = CoherenceReport {
                mixdown_rms_db: -10.0,
                residual_rms_db: -10.0 + rel_db,
                relative_db: rel_db,
                stems: vec![],
            };
            assert!(
                r.summary_line().contains(expected_substr),
                "rel_db={rel_db}: expected `{expected_substr}` in `{}`",
                r.summary_line()
            );
        }
    }
}
