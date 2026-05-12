//! Mix-tab spectrum panel — pinned at the top of the Mix tab when
//! `Config.show_spectrum_panel` is true (default). Live FFT of the
//! master output bus tap (`PlayerState.output_viz`), drawn as bars
//! on a log-frequency X axis with a slow-release peak-decay trail
//! sitting above the live spectrum.
//!
//! No new audio-thread plumbing: `output_viz` is the same tap the
//! standalone visualizer canvas reads (`src/ui/visualizer.rs`),
//! filled by the cpal callback in `src/player.rs`.
//!
//! The decay-trail state (`app.spectrum_trail`) is owned by the UI
//! thread — pure post-processing on top of the per-frame FFT. The
//! tail decays per frame at 0.95× regardless of the FFT result, so
//! when playback stops the bars naturally fall to the floor over
//! ~1–2 s rather than freezing at the last value.
//!
//! Added v0.4.18.

use crate::app::TinyBoothApp;
use eframe::egui;
use egui::{Color32, Pos2, Stroke};

/// X-axis frequency range (log-scale). 20 Hz to 20 kHz covers the
/// full audible band; the upper bound also matches the typical
/// 48 kHz sample-rate Nyquist of 24 kHz with 4 kHz of headroom for
/// when a track is sampled higher.
const F_LO_HZ: f32 = 20.0;
const F_HI_HZ: f32 = 20_000.0;

/// Per-frame decay multiplier for the peak-hold trail. 0.95 at ~30
/// fps gives a ~1 s release time — fast enough to follow the music,
/// slow enough that you can read peak content visually.
const TRAIL_DECAY: f32 = 0.95;

pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui, height: f32) {
    let avail_w = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(avail_w, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(8, 8, 12));

    // Pull the master-bus sample tap. Lock window is microseconds —
    // matches what the visualizer canvas does at src/ui/visualizer.rs.
    let (samples, sr) = match app.player.as_ref() {
        Some(player) => {
            let s: Vec<f32> = player
                .state
                .output_viz
                .lock()
                .iter()
                .map(|(l, r)| 0.5 * (l + r))
                .collect();
            (s, player.state.sample_rate.max(1))
        }
        None => (Vec::new(), 48_000),
    };

    if samples.len() < 64 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "spectrum — start playback to feed the analyzer",
            egui::FontId::proportional(11.0),
            Color32::from_gray(100),
        );
        // Still tick the trail down so it's empty when audio resumes.
        decay_trail(&mut app.spectrum_trail);
        request_repaint(ui);
        return;
    }

    // FFT (Hann-windowed, 512..4096 power-of-two, log-mag mapped to
    // [0, 1] over an ~80 dB range — the helper handles all of that).
    let spec = crate::analysis::spectrum(&samples);
    if spec.is_empty() {
        decay_trail(&mut app.spectrum_trail);
        request_repaint(ui);
        return;
    }

    // Maintain trail length parity with the spectrum (handles SR
    // changes, fft-size shifts, etc.).
    if app.spectrum_trail.len() != spec.len() {
        app.spectrum_trail.resize(spec.len(), 0.0);
    }
    for (t, &v) in app.spectrum_trail.iter_mut().zip(spec.iter()) {
        *t = (v).max(*t * TRAIL_DECAY);
    }

    // Bin → frequency. The `analysis::spectrum` helper drops the DC
    // bin and the upper mirror, so spec[i] corresponds to FFT bin i+1
    // (i ∈ [0, fft_size/2)). Reconstructing fft_size from spec.len():
    // fft_size = spec.len() * 2.
    let fft_size = (spec.len() as u32) * 2;
    let bin_hz = sr as f32 / fft_size as f32;
    let bin_freq = |i: usize| -> f32 { (i + 1) as f32 * bin_hz };

    // Map a frequency to an X pixel via log10 — the only useful axis
    // for music spectra (linear hides everything below 4 kHz).
    let log_lo = F_LO_HZ.log10();
    let log_hi = F_HI_HZ.log10();
    let x_for_freq = |f: f32| -> f32 {
        let n = ((f.max(F_LO_HZ).log10() - log_lo) / (log_hi - log_lo)).clamp(0.0, 1.0);
        rect.min.x + n * rect.width()
    };
    let y_for_norm = |n: f32| -> f32 {
        // n already in [0, 1]; invert for screen-y (top is loud).
        rect.max.y - n.clamp(0.0, 1.0) * rect.height()
    };

    // Decade gridlines (100 Hz, 1 kHz, 10 kHz). Cheap visual frame
    // of reference for "where am I in the spectrum".
    let grid = Stroke::new(0.5, Color32::from_gray(30));
    for &f in &[100.0_f32, 1_000.0, 10_000.0] {
        let x = x_for_freq(f);
        painter.line_segment([Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)], grid);
    }

    // Bars (live spectrum) at one px per column, picking the loudest
    // bin that maps into each column — avoids gaps when there are
    // more pixels than bins at the high end.
    let cols = rect.width() as usize;
    if cols == 0 {
        return;
    }
    let mut col_live = vec![0.0_f32; cols];
    let mut col_trail = vec![0.0_f32; cols];
    for (i, (&v, &t)) in spec.iter().zip(app.spectrum_trail.iter()).enumerate() {
        let f = bin_freq(i);
        if !(F_LO_HZ..=F_HI_HZ).contains(&f) {
            continue;
        }
        let px = (x_for_freq(f) - rect.min.x) as usize;
        let px = px.min(cols - 1);
        if v > col_live[px] {
            col_live[px] = v;
        }
        if t > col_trail[px] {
            col_trail[px] = t;
        }
    }
    // Spread each non-zero column to its right neighbour so that
    // log-axis bunching at the low end doesn't leave 1-px holes
    // between bars at the high end.
    for i in 1..cols {
        if col_live[i] == 0.0 && col_live[i - 1] > 0.0 {
            col_live[i] = col_live[i - 1] * 0.85;
        }
        if col_trail[i] == 0.0 && col_trail[i - 1] > 0.0 {
            col_trail[i] = col_trail[i - 1] * 0.85;
        }
    }

    let live_color = Color32::from_rgb(100, 220, 150);
    let trail_color = Color32::from_rgba_unmultiplied(230, 200, 80, 180);
    for (i, &v) in col_live.iter().enumerate() {
        if v <= 0.0 {
            continue;
        }
        let x = rect.min.x + i as f32;
        painter.line_segment(
            [Pos2::new(x, rect.max.y), Pos2::new(x, y_for_norm(v))],
            Stroke::new(1.0, live_color),
        );
    }
    // Trail line drawn over the bars so it sits on top.
    let mut prev: Option<Pos2> = None;
    for (i, &t) in col_trail.iter().enumerate() {
        if t <= 0.0 {
            prev = None;
            continue;
        }
        let p = Pos2::new(rect.min.x + i as f32, y_for_norm(t));
        if let Some(pv) = prev {
            painter.line_segment([pv, p], Stroke::new(1.2, trail_color));
        }
        prev = Some(p);
    }

    request_repaint(ui);
}

/// Tick every trail bin down by the decay factor — used when no
/// audio is currently feeding the spectrum so bars fall to floor
/// rather than freeze at their last value.
fn decay_trail(trail: &mut [f32]) {
    for t in trail.iter_mut() {
        *t *= TRAIL_DECAY;
    }
}

/// Live spectrum needs a steady repaint cadence — we don't get one
/// from input events. ~30 fps matches the visualizer canvas and is
/// indistinguishable from 60 to the eye for a slowly-changing FFT.
fn request_repaint(ui: &egui::Ui) {
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `decay_trail` must monotonically reduce every non-zero bin and
    /// leave zeros at zero. Guards the silence-fade behaviour.
    #[test]
    fn decay_trail_reduces_non_zero_and_preserves_zero() {
        let mut t = vec![1.0_f32, 0.5, 0.0, 0.25, 0.0];
        decay_trail(&mut t);
        assert!((t[0] - TRAIL_DECAY).abs() < 1e-6);
        assert!((t[1] - 0.5 * TRAIL_DECAY).abs() < 1e-6);
        assert_eq!(t[2], 0.0);
        assert!((t[3] - 0.25 * TRAIL_DECAY).abs() < 1e-6);
        assert_eq!(t[4], 0.0);
    }

    /// After enough ticks at 30 fps (~3 s), every bin should fall
    /// below 0.001 — the user's "audio stopped, bars settle" signal.
    #[test]
    fn decay_trail_settles_to_floor_within_three_seconds() {
        let mut t = vec![1.0_f32; 8];
        // 30 fps × 3 s = 90 ticks. 0.95^90 ≈ 0.0099 — already below
        // visible threshold for an 80-dB range.
        for _ in 0..90 {
            decay_trail(&mut t);
        }
        for &v in &t {
            assert!(v < 0.01, "bin still loud after 3 s: {v}");
        }
    }
}
