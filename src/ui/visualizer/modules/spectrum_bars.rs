//! Spectrum analyzer (#2) — log-frequency magnitude bars with EMA
//! smoothing and falling peak-hold. The calm, expected studio readout.

use crate::ui::visualizer::{hsv_to_rgb, slider, FrameCtx, VizModule};
use eframe::egui;

#[derive(Debug, Clone)]
pub struct SpectrumBarsParams {
    pub bars: usize,
    pub f_min: f32,
    pub f_max: f32,
    pub smoothing: f32,
    pub peak_fall: f32,
}
impl Default for SpectrumBarsParams {
    fn default() -> Self {
        Self {
            bars: 64,
            f_min: 30.0,
            f_max: 18_000.0,
            smoothing: 0.6,
            peak_fall: 0.015,
        }
    }
}

#[derive(Default)]
pub struct SpectrumBars {
    p: SpectrumBarsParams,
    levels: Vec<f32>,
    peaks: Vec<f32>,
}

impl VizModule for SpectrumBars {
    fn id(&self) -> &'static str {
        "spectrum_bars"
    }
    fn label(&self) -> &'static str {
        "Spectrum"
    }
    fn description(&self) -> &'static str {
        "Log-frequency magnitude bars with EMA smoothing + falling peak-hold."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        if ctx.spectrum.is_empty() || ctx.sample_rate == 0 {
            return;
        }
        let nbars = self.p.bars.max(4);
        if self.levels.len() != nbars {
            self.levels = vec![0.0; nbars];
            self.peaks = vec![0.0; nbars];
        }
        let slen = ctx.spectrum.len();
        let ratio = (self.p.f_max / self.p.f_min.max(1.0)).max(1.0);
        let alpha = 1.0 - self.p.smoothing.clamp(0.0, 0.95);

        // Accumulate spectrum energy into log-spaced bars.
        for b in 0..nbars {
            let f0 = self.p.f_min * ratio.powf(b as f32 / nbars as f32);
            let f1 = self.p.f_min * ratio.powf((b + 1) as f32 / nbars as f32);
            let bin0 = (f0 * (2 * slen) as f32 / ctx.sample_rate as f32) as usize;
            let bin1 = ((f1 * (2 * slen) as f32 / ctx.sample_rate as f32) as usize).max(bin0 + 1);
            let mut peak = 0.0_f32;
            for k in bin0..bin1.min(slen) {
                peak = peak.max(ctx.spectrum[k]);
            }
            self.levels[b] += alpha * (peak - self.levels[b]);
            if self.levels[b] > self.peaks[b] {
                self.peaks[b] = self.levels[b];
            } else {
                self.peaks[b] = (self.peaks[b] - self.p.peak_fall).max(self.levels[b]);
            }
        }

        let gap = 2.0;
        let bw = (rect.width() / nbars as f32 - gap).max(1.0);
        for b in 0..nbars {
            let x = rect.left() + b as f32 * rect.width() / nbars as f32;
            let h = self.levels[b].clamp(0.0, 1.0) * rect.height();
            let hue = 0.6 - 0.6 * b as f32 / nbars as f32;
            let (r, g, bl) = hsv_to_rgb(hue, 0.7, 0.9);
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x, rect.bottom() - h),
                    egui::pos2(x + bw, rect.bottom()),
                ),
                0.0,
                egui::Color32::from_rgb(r, g, bl),
            );
            let py = rect.bottom() - self.peaks[b].clamp(0.0, 1.0) * rect.height();
            painter.line_segment(
                [egui::pos2(x, py), egui::pos2(x + bw, py)],
                egui::Stroke::new(1.5, egui::Color32::from_gray(230)),
            );
        }

        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!(
                "Spectrum · {nbars} bars · {:.0} Hz–{:.0}k",
                self.p.f_min,
                self.p.f_max / 1000.0
            ),
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(160),
        );
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        slider(
            ui,
            "Bars",
            "Number of log-spaced frequency bars.",
            &mut p.bars,
            16..=128,
        );
        slider(
            ui,
            "Min Hz",
            "Bottom frequency.",
            &mut p.f_min,
            20.0..=200.0,
        );
        slider(
            ui,
            "Max Hz",
            "Top frequency.",
            &mut p.f_max,
            4000.0..=22000.0,
        );
        slider(
            ui,
            "Smoothing",
            "EMA weight on the previous level (higher = smoother bars).",
            &mut p.smoothing,
            0.0..=0.95,
        );
        slider(
            ui,
            "Peak fall",
            "How fast the peak-hold marker falls per frame.",
            &mut p.peak_fall,
            0.002..=0.05,
        );
    }
}
