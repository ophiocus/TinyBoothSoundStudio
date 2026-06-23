//! Reassigned spectrogram (#3) — sharpen the spectrogram by relocating
//! each bin's energy to its channelized instantaneous frequency
//! (derived from the phase via a derivative-of-window STFT). Tonal
//! components snap from a soft smear to an etched line.

use crate::{blit_image, magma, slider, FrameCtx, VizModule};
use eframe::egui;
use rustfft::{num_complex::Complex, FftPlanner};
use std::collections::VecDeque;

const FFT: usize = 2048;
const MAX_COLS: usize = 600;
const ROWS: usize = 256;

#[derive(Debug, Clone)]
pub struct ReassignedParams {
    pub f_min: f32,
    pub f_max: f32,
    pub gamma: f32,
    pub reassign: bool,
}
impl Default for ReassignedParams {
    fn default() -> Self {
        Self {
            f_min: 40.0,
            f_max: 16_000.0,
            gamma: 0.6,
            reassign: true,
        }
    }
}

pub struct Reassigned {
    p: ReassignedParams,
    h: Vec<f32>,
    dh: Vec<f32>,
    history: VecDeque<Vec<f32>>, // each column: ROWS log-f magnitudes (0..1)
    ema_top: f32,
    tex: Option<egui::TextureHandle>,
}
impl Default for Reassigned {
    fn default() -> Self {
        // Hann window + its analytic derivative (per-sample).
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
            p: ReassignedParams::default(),
            h,
            dh,
            history: VecDeque::new(),
            ema_top: 0.0,
            tex: None,
        }
    }
}

impl VizModule for Reassigned {
    fn id(&self) -> &'static str {
        "reassigned"
    }
    fn label(&self) -> &'static str {
        "Reassigned"
    }
    fn description(&self) -> &'static str {
        "Phase-reassigned spectrogram — energy snapped to its true instantaneous frequency."
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
        // Accumulate energy into ROWS log-f bins.
        let ratio = (self.p.f_max / self.p.f_min.max(1.0)).max(1.0);
        let mut col = vec![0.0_f32; ROWS];
        let hz_per_bin = ctx.sample_rate as f32 / FFT as f32;
        for k in 1..half {
            let mag2 = bh[k].norm_sqr();
            if mag2 < 1e-10 {
                continue;
            }
            // Channelized instantaneous frequency: ω̂ = ω_k − Im(Xdh·conj(Xh)/|Xh|²).
            let corr = if self.p.reassign {
                (bdh[k] * bh[k].conj()).im / mag2
            } else {
                0.0
            };
            // corr is rad/sample → bin offset = corr·FFT/2π.
            let kf = k as f32 - corr * FFT as f32 / std::f32::consts::TAU;
            let hz = kf.max(0.0) * hz_per_bin;
            if hz < self.p.f_min || hz > self.p.f_max {
                continue;
            }
            let frac = (hz / self.p.f_min).log(ratio).clamp(0.0, 1.0);
            let row = ((1.0 - frac) * (ROWS - 1) as f32) as usize; // top = high
            col[row.min(ROWS - 1)] += mag2;
        }
        // Normalize this column to 0..1 (log).
        for v in &mut col {
            *v = (10.0 * (*v + 1e-9).log10() * 0.1 + 1.0).clamp(0.0, 4.0);
        }

        self.history.push_back(col);
        while self.history.len() > MAX_COLS {
            self.history.pop_front();
        }
        // Auto top.
        let cmax = self
            .history
            .back()
            .map(|c| c.iter().cloned().fold(0.05_f32, f32::max))
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
            for (r, &v) in column.iter().enumerate() {
                let nv = (v / top).clamp(0.0, 1.0).powf(self.p.gamma);
                let (rr, gg, bb) = magma(nv);
                img.pixels[r * cols + cx] = egui::Color32::from_rgb(rr, gg, bb);
            }
        }
        blit_image(
            painter,
            rect,
            &mut self.tex,
            "viz_reassigned",
            img,
            egui::TextureOptions::LINEAR,
        );

        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!(
                "Reassigned · {} · {:.0}–{:.0} Hz",
                if self.p.reassign {
                    "sharpened"
                } else {
                    "raw STFT"
                },
                self.p.f_min,
                self.p.f_max
            ),
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(200),
        );
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        ui.checkbox(&mut p.reassign, "Reassign (A/B)")
            .on_hover_text(
                "Toggle the phase reassignment off to see the blurry baseline it sharpens.",
            );
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
            "Gamma",
            "Display gamma. <1 lifts quiet detail.",
            &mut p.gamma,
            0.3..=1.5,
        );
    }
}
