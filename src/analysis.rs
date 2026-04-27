//! Minimal FFT-based spectrum analyser and waveform peak decimator.

use rustfft::{num_complex::Complex, FftPlanner};

/// Compute a log-magnitude spectrum for `samples` using an FFT of the next
/// power of two (clamped to [512, 4096]), Hann-windowed. Returns values in
/// roughly [0.0, 1.0] suitable for a bar plot. Output length = fft_size / 2.
pub fn spectrum(samples: &[f32]) -> Vec<f32> {
    if samples.len() < 64 {
        return Vec::new();
    }
    let fft_size = samples.len().next_power_of_two().clamp(512, 4096);
    let take = fft_size.min(samples.len());
    let start = samples.len() - take;

    let mut buf: Vec<Complex<f32>> = (0..fft_size)
        .map(|i| {
            let x = if i < take { samples[start + i] } else { 0.0 };
            // Hann window over the populated region.
            let w = if i < take {
                let t = i as f32 / (take.max(2) - 1) as f32;
                0.5 - 0.5 * (std::f32::consts::TAU * t).cos()
            } else {
                0.0
            };
            Complex { re: x * w, im: 0.0 }
        })
        .collect();

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(fft_size);
    fft.process(&mut buf);

    // Log-magnitude, normalised. Drop the DC bin and the upper mirror half.
    let n = fft_size / 2;
    let mut out = Vec::with_capacity(n);
    for bin in &buf[..n] {
        let mag = (bin.re * bin.re + bin.im * bin.im).sqrt();
        // 20 * log10(mag) roughly, with a floor, mapped to [0, 1].
        let db = 20.0 * (mag + 1e-6).log10();
        let norm = ((db + 80.0) / 80.0).clamp(0.0, 1.0);
        out.push(norm);
    }
    out
}

/// Decimate a long sample slice into `bins` peaks-per-bin (abs max).
/// Used by the live waveform view when the on-screen width is smaller
/// than the number of samples in the ring buffer.
pub fn peak_bins(samples: &[f32], bins: usize) -> Vec<f32> {
    if bins == 0 || samples.is_empty() {
        return Vec::new();
    }
    if samples.len() <= bins {
        return samples.iter().map(|s| s.abs()).collect();
    }
    let step = samples.len() as f32 / bins as f32;
    (0..bins)
        .map(|b| {
            let start = (b as f32 * step) as usize;
            let end = ((b as f32 + 1.0) * step) as usize;
            let end = end.min(samples.len()).max(start + 1);
            samples[start..end]
                .iter()
                .copied()
                .fold(0.0f32, |acc, s| acc.max(s.abs()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_bins_empty() {
        assert!(peak_bins(&[], 8).is_empty());
        assert!(peak_bins(&[1.0, -1.0], 0).is_empty());
    }

    #[test]
    fn peak_bins_short_input_passes_through_abs() {
        let r = peak_bins(&[0.5, -0.7, 0.2], 8);
        // Fewer samples than bins → just |s| per sample.
        assert_eq!(r.len(), 3);
        assert!((r[0] - 0.5).abs() < 1e-6);
        assert!((r[1] - 0.7).abs() < 1e-6);
        assert!((r[2] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn peak_bins_takes_abs_max_per_bin() {
        // 8 samples, 4 bins → 2 samples per bin, abs-max per pair.
        let s = vec![0.1, -0.9, 0.2, 0.3, -0.4, -0.6, 0.0, 0.7];
        let r = peak_bins(&s, 4);
        assert_eq!(r.len(), 4);
        assert!((r[0] - 0.9).abs() < 1e-6);
        assert!((r[1] - 0.3).abs() < 1e-6);
        assert!((r[2] - 0.6).abs() < 1e-6);
        assert!((r[3] - 0.7).abs() < 1e-6);
    }

    #[test]
    fn spectrum_short_input_is_empty() {
        assert!(spectrum(&[0.0; 8]).is_empty());
        assert!(spectrum(&[0.0; 32]).is_empty());
    }

    #[test]
    fn spectrum_silence_is_minimal() {
        let r = spectrum(&[0.0; 1024]);
        // Floor mapping: dB ~ -120 → ((-120 + 80)/80) clamped to 0.
        for v in &r {
            assert!(*v < 0.05, "silence should map near 0; got {v}");
        }
    }

    #[test]
    fn spectrum_pure_tone_peaks_in_band() {
        // 480 Hz sine, 1024 samples at 48 kHz.
        let sr = 48_000.0;
        let f = 480.0;
        let samples: Vec<f32> = (0..1024)
            .map(|n| (2.0 * std::f32::consts::PI * f * n as f32 / sr).sin() * 0.5)
            .collect();
        let r = spectrum(&samples);
        assert!(!r.is_empty());
        let peak_idx = r
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        // FFT size = 1024, bin width = sr / 1024 ≈ 46.875 Hz.
        // 480 Hz lands roughly at bin index 480 / 46.875 ≈ 10.24.
        assert!(
            (8..=13).contains(&peak_idx),
            "expected the peak near bin 10 for 480 Hz; found {peak_idx}"
        );
    }
}
