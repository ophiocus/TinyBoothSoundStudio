//! Spectral saliency (#17, model-free) — the honest cousin of neural
//! attention. An Itti–Koch center-surround over the spectrum plus a
//! temporal-novelty term highlights *what stands out* — the bins your
//! ear is drawn to — without a trained model. Bright = salient.
//!
//! (The neural Grad-CAM / RAVE variants of #17 need a pretrained model
//! that isn't available offline; this delivers the same question —
//! "what is the signal emphasising right now?" — from first principles.)

use crate::{blit_image, magma, slider, FrameCtx, VizModule};
use eframe::egui;
use std::collections::VecDeque;

const MAX_COLS: usize = 600;
const ROWS: usize = 256;

#[derive(Debug, Clone)]
pub struct SaliencyParams {
    pub f_min: f32,
    pub f_max: f32,
    pub surround: usize,
    pub temporal: f32,
}
impl Default for SaliencyParams {
    fn default() -> Self {
        Self {
            f_min: 40.0,
            f_max: 16_000.0,
            surround: 6,
            temporal: 0.5,
        }
    }
}

#[derive(Default)]
pub struct Saliency {
    p: SaliencyParams,
    prev: Vec<f32>,
    history: VecDeque<Vec<f32>>,
    ema_top: f32,
    tex: Option<egui::TextureHandle>,
}

impl VizModule for Saliency {
    fn id(&self) -> &'static str {
        "saliency"
    }
    fn label(&self) -> &'static str {
        "Saliency"
    }
    fn description(&self) -> &'static str {
        "Center-surround + temporal-novelty saliency — what stands out, model-free (#17 cousin)."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        let spec = &ctx.spectrum;
        if spec.is_empty() || ctx.sample_rate == 0 {
            return;
        }
        let slen = spec.len();
        let s = self.p.surround.max(1);
        // Center-surround: bin minus the mean of its neighbourhood,
        // rectified. Plus temporal novelty vs the previous frame.
        let same_len = self.prev.len() == slen;
        let mut sal = vec![0.0_f32; slen];
        for k in 0..slen {
            let lo = k.saturating_sub(s);
            let hi = (k + s + 1).min(slen);
            let sum: f32 = spec[lo..hi].iter().sum();
            let surround = (sum - spec[k]) / (hi - lo - 1).max(1) as f32;
            let cs = (spec[k] - surround).max(0.0);
            let temporal = if same_len {
                (spec[k] - self.prev[k]).max(0.0)
            } else {
                0.0
            };
            sal[k] = cs + self.p.temporal * temporal;
        }
        self.prev = spec.clone();

        // Map to ROWS log-f rows.
        let ratio = (self.p.f_max / self.p.f_min.max(1.0)).max(1.0);
        let mut col = vec![0.0_f32; ROWS];
        for (r, slot) in col.iter_mut().enumerate() {
            let frac = 1.0 - r as f32 / (ROWS - 1) as f32;
            let f = self.p.f_min * ratio.powf(frac);
            let bin = (f * (2 * slen) as f32 / ctx.sample_rate as f32) as usize;
            *slot = sal.get(bin).copied().unwrap_or(0.0);
        }
        self.history.push_back(col);
        while self.history.len() > MAX_COLS {
            self.history.pop_front();
        }
        let cmax = self
            .history
            .back()
            .map(|c| c.iter().cloned().fold(0.02_f32, f32::max))
            .unwrap_or(1.0);
        if self.ema_top <= 0.0 {
            self.ema_top = cmax;
        } else {
            self.ema_top += 0.08 * (cmax - self.ema_top);
        }
        let top = self.ema_top.max(0.02);

        let cols = self.history.len().max(1);
        let mut img = egui::ColorImage::new([cols, ROWS], egui::Color32::BLACK);
        for (cx, column) in self.history.iter().enumerate() {
            for (r, &v) in column.iter().enumerate() {
                let nv = (v / top).clamp(0.0, 1.0).powf(0.6);
                let (rr, gg, bb) = magma(nv);
                img.pixels[r * cols + cx] = egui::Color32::from_rgb(rr, gg, bb);
            }
        }
        blit_image(
            painter,
            rect,
            &mut self.tex,
            "viz_saliency",
            img,
            egui::TextureOptions::LINEAR,
        );

        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            "Saliency · center-surround + novelty (model-free)",
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
            "Surround",
            "Half-width (bins) of the center-surround neighbourhood.",
            &mut p.surround,
            1..=24,
        );
        slider(
            ui,
            "Temporal",
            "Weight of the temporal-novelty (onset) term vs spatial contrast.",
            &mut p.temporal,
            0.0..=2.0,
        );
    }
}
