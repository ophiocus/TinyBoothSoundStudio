//! Optical-flow spectrogram (#16) — see time-frequency energy *move*.
//! Per bin we estimate the instantaneous-frequency velocity from the
//! phase (a derivative-of-window STFT), then colour each pixel by that
//! velocity (Middlebury-style flow hue) with luminance from magnitude.
//! Vibrato shimmers as oscillating hue; glissando reads as a hue glide.

use crate::{blit_image, hsv_to_rgb, slider, FrameCtx, VizModule};
use eframe::egui;
use rustfft::{num_complex::Complex, FftPlanner};
use std::collections::VecDeque;

const FFT: usize = 2048;
const MAX_COLS: usize = 600;
const ROWS: usize = 256;

#[derive(Debug, Clone)]
pub struct OpticalFlowParams {
    pub f_min: f32,
    pub f_max: f32,
    pub flow_gain: f32,
}
impl Default for OpticalFlowParams {
    fn default() -> Self {
        Self {
            f_min: 40.0,
            f_max: 16_000.0,
            flow_gain: 3.0,
        }
    }
}

pub struct OpticalFlow {
    p: OpticalFlowParams,
    h: Vec<f32>,
    dh: Vec<f32>,
    history: VecDeque<Vec<(f32, f32)>>, // (magnitude 0..1, signed velocity bins)
    ema_top: f32,
    tex: Option<egui::TextureHandle>,
}
impl Default for OpticalFlow {
    fn default() -> Self {
        let h: Vec<f32> = (0..FFT)
            .map(|n| {
                let t = n as f32 / (FFT - 1) as f32;
                0.5 - 0.5 * (std::f32::consts::TAU * t).cos()
            })
            .collect();
        let dh: Vec<f32> = (0..FFT)
            .map(|n| {
                let t = n as f32 / (FFT - 1) as f32;
                std::f32::consts::PI / FFT as f32 * (std::f32::consts::TAU * t).sin()
            })
            .collect();
        Self {
            p: OpticalFlowParams::default(),
            h,
            dh,
            history: VecDeque::new(),
            ema_top: 0.0,
            tex: None,
        }
    }
}

impl VizModule for OpticalFlow {
    fn id(&self) -> &'static str {
        "optical_flow"
    }
    fn label(&self) -> &'static str {
        "Optical Flow"
    }
    fn description(&self) -> &'static str {
        "Spectrogram tinted by instantaneous-frequency velocity. Glissando/vibrato become visible motion."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        let x = &ctx.mono;
        if x.len() < FFT || ctx.sample_rate == 0 {
            return;
        }
        let start = x.len() - FFT;
        let mut bh: Vec<Complex<f32>> = (0..FFT)
            .map(|n| Complex {
                re: x[start + n] * self.h[n],
                im: 0.0,
            })
            .collect();
        let mut bdh: Vec<Complex<f32>> = (0..FFT)
            .map(|n| Complex {
                re: x[start + n] * self.dh[n],
                im: 0.0,
            })
            .collect();
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT);
        fft.process(&mut bh);
        fft.process(&mut bdh);

        let half = FFT / 2;
        let ratio = (self.p.f_max / self.p.f_min.max(1.0)).max(1.0);
        let hz_per_bin = ctx.sample_rate as f32 / FFT as f32;
        // For each display row (log-f), find the nearest bin and read
        // its magnitude + IF velocity.
        let mut col = vec![(0.0_f32, 0.0_f32); ROWS];
        for (r, slot) in col.iter_mut().enumerate() {
            let frac = 1.0 - r as f32 / (ROWS - 1) as f32;
            let f = self.p.f_min * ratio.powf(frac);
            let k = (f / hz_per_bin).round() as usize;
            if k == 0 || k >= half {
                continue;
            }
            let mag2 = bh[k].norm_sqr();
            if mag2 < 1e-10 {
                continue;
            }
            // IF velocity in bins: corr = Im(Xdh·conj(Xh)/|Xh|²)·FFT/2π.
            let corr = (bdh[k] * bh[k].conj()).im / mag2;
            let vel = -corr * FFT as f32 / std::f32::consts::TAU;
            let mag = (10.0 * (mag2 + 1e-9).log10() * 0.05 + 1.0).clamp(0.0, 2.0);
            *slot = (mag, vel);
        }

        self.history.push_back(col);
        while self.history.len() > MAX_COLS {
            self.history.pop_front();
        }
        let cmax = self
            .history
            .back()
            .map(|c| c.iter().fold(0.05_f32, |a, &(m, _)| a.max(m)))
            .unwrap_or(1.0);
        if self.ema_top <= 0.0 {
            self.ema_top = cmax;
        } else {
            self.ema_top += 0.05 * (cmax - self.ema_top);
        }
        let top = self.ema_top.max(0.05);

        let cols = self.history.len().max(1);
        let mut img = egui::ColorImage::new([cols, ROWS], egui::Color32::BLACK);
        for (cx, column) in self.history.iter().enumerate() {
            for (r, &(mag, vel)) in column.iter().enumerate() {
                let v = (mag / top).clamp(0.0, 1.0);
                if v < 0.02 {
                    continue;
                }
                // Flow hue: 0 velocity → green (~0.33), up-glide → blue,
                // down-glide → red. tanh squashes outliers.
                let f = (vel * self.p.flow_gain * 0.05).tanh(); // [-1,1]
                let hue = (0.33 - f * 0.33).rem_euclid(1.0);
                let (rr, gg, bb) = hsv_to_rgb(hue, 0.85, v);
                img.pixels[r * cols + cx] = egui::Color32::from_rgb(rr, gg, bb);
            }
        }
        blit_image(
            painter,
            rect,
            &mut self.tex,
            "viz_optical_flow",
            img,
            egui::TextureOptions::LINEAR,
        );

        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            "Optical flow · hue = IF velocity (↑blue ↓red)",
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(210),
        );
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
            "Top of the log-frequency axis.",
            &mut p.f_max,
            2000.0..=22000.0,
        );
        slider(
            ui,
            "Flow gain",
            "Sensitivity of the hue mapping to frequency velocity.",
            &mut p.flow_gain,
            0.5..=8.0,
        );
    }
}
