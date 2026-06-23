//! Spectrogram (#2/#4/#8/#12) — scrolling log-frequency magnitude with a
//! perceptually-uniform magma colormap and EMA auto-ranging. The iconic
//! baseline the app was missing; substrate the eye reads as music.

use crate::{blit_image, magma, slider, FrameCtx, VizModule};
use eframe::egui;
use std::collections::VecDeque;

const MAX_COLS: usize = 600;
const ROWS: usize = 256;

#[derive(Debug, Clone)]
pub struct SpectrogramParams {
    pub f_min: f32,
    pub f_max: f32,
    pub gamma: f32,
    pub auto_range: bool,
}
impl Default for SpectrogramParams {
    fn default() -> Self {
        Self {
            f_min: 40.0,
            f_max: 16_000.0,
            gamma: 0.7,
            auto_range: true,
        }
    }
}

#[derive(Default)]
pub struct Spectrogram {
    p: SpectrogramParams,
    history: VecDeque<Vec<f32>>,
    ema_top: f32,
    tex: Option<egui::TextureHandle>,
}

impl VizModule for Spectrogram {
    fn id(&self) -> &'static str {
        "spectrogram"
    }
    fn label(&self) -> &'static str {
        "Spectrogram"
    }
    fn description(&self) -> &'static str {
        "Scrolling log-frequency magnitude, perceptual magma colormap, auto-ranged. The iconic baseline."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        if ctx.spectrum.is_empty() || ctx.sample_rate == 0 {
            return;
        }
        // Push the newest column; cap history.
        self.history.push_back(ctx.spectrum.clone());
        while self.history.len() > MAX_COLS {
            self.history.pop_front();
        }

        // EMA auto-range: track the 95th-percentile-ish top of the newest
        // column so quiet passages brighten and loud ones don't clip.
        let col_top = {
            let mut v: Vec<f32> = ctx.spectrum.clone();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            v[(v.len() as f32 * 0.95) as usize % v.len().max(1)].max(0.05)
        };
        if self.p.auto_range {
            if self.ema_top <= 0.0 {
                self.ema_top = col_top;
            } else {
                self.ema_top += 0.05 * (col_top - self.ema_top);
            }
        } else {
            self.ema_top = 1.0;
        }
        let top = self.ema_top.max(0.05);

        let cols = self.history.len().max(1);
        let slen = ctx.spectrum.len();
        let ratio = (self.p.f_max / self.p.f_min.max(1.0)).max(1.0);
        // Precompute the spectrum bin for each display row (log-f).
        let row_bin: Vec<usize> = (0..ROWS)
            .map(|r| {
                let frac = 1.0 - r as f32 / (ROWS - 1) as f32; // top = high
                let f = self.p.f_min * ratio.powf(frac);
                // invert bin_hz: bin = f * fft_size / sr, fft_size = 2*slen
                let bin = (f * (2 * slen) as f32 / ctx.sample_rate as f32) as usize;
                bin.min(slen - 1)
            })
            .collect();

        let mut img = egui::ColorImage::new([cols, ROWS], egui::Color32::BLACK);
        for (cx, column) in self.history.iter().enumerate() {
            for (r, &bin) in row_bin.iter().enumerate() {
                let v = (column.get(bin).copied().unwrap_or(0.0) / top).clamp(0.0, 1.0);
                let v = v.powf(self.p.gamma);
                let (rr, gg, bb) = magma(v);
                img.pixels[r * cols + cx] = egui::Color32::from_rgb(rr, gg, bb);
            }
        }
        blit_image(
            painter,
            rect,
            &mut self.tex,
            "viz_spectrogram",
            img,
            egui::TextureOptions::LINEAR,
        );

        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!(
                "Spectrogram · {:.0}–{:.0} Hz · log-f · auto×{:.2}",
                self.p.f_min, self.p.f_max, top
            ),
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(200),
        );
        // y-axis octave ticks.
        for &hz in &[100.0_f32, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0] {
            if hz < self.p.f_min || hz > self.p.f_max {
                continue;
            }
            let frac = (hz / self.p.f_min).log(ratio).clamp(0.0, 1.0);
            let y = rect.bottom() - frac * rect.height();
            painter.text(
                egui::pos2(rect.right() - 6.0, y),
                egui::Align2::RIGHT_CENTER,
                if hz >= 1000.0 {
                    format!("{:.0}k", hz / 1000.0)
                } else {
                    format!("{hz:.0}")
                },
                egui::FontId::monospace(10.0),
                egui::Color32::from_gray(150),
            );
        }
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        slider(
            ui,
            "Min Hz",
            "Bottom of the log-frequency axis.",
            &mut p.f_min,
            20.0..=500.0,
        );
        slider(
            ui,
            "Max Hz",
            "Top of the log-frequency axis (capped at Nyquist by the data).",
            &mut p.f_max,
            2000.0..=22000.0,
        );
        slider(
            ui,
            "Gamma",
            "Display gamma on the normalized magnitude. <1 lifts quiet detail.",
            &mut p.gamma,
            0.3..=1.5,
        );
        ui.checkbox(&mut p.auto_range, "Auto-range").on_hover_text(
            "Track the column's running 95th-percentile top so quiet passages \
                 brighten and loud ones don't clip. The autonomous-dimensioning move.",
        );
    }
}
