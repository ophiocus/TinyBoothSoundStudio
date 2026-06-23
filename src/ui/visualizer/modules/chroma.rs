//! Chromagram + circle of fifths (#5) — fold the spectrum onto the 12
//! pitch classes, arrange them by the circle of fifths, and draw the
//! resultant "key vector" needle. Harmony made a glowing compass.

use crate::ui::visualizer::{bin_hz, hsv_to_rgb, slider, FrameCtx, VizModule};
use eframe::egui;
use std::f32::consts::TAU;

const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];
/// Circle-of-fifths order: index = position around the circle, value =
/// pitch class. C, G, D, A, E, B, F#, C#, G#, D#, A#, F.
const FIFTHS: [usize; 12] = [0, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10, 5];

#[derive(Debug, Clone)]
pub struct ChromaParams {
    pub smoothing: f32,
    pub gamma: f32,
}
impl Default for ChromaParams {
    fn default() -> Self {
        Self {
            smoothing: 0.6,
            gamma: 0.6,
        }
    }
}

#[derive(Default)]
pub struct Chroma {
    p: ChromaParams,
    chroma: [f32; 12],
}

impl VizModule for Chroma {
    fn id(&self) -> &'static str {
        "chroma"
    }
    fn label(&self) -> &'static str {
        "Chroma"
    }
    fn description(&self) -> &'static str {
        "Pitch-class energy on the circle of fifths + key-vector needle. Harmony at a glance."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        if ctx.spectrum.is_empty() || ctx.sample_rate == 0 {
            return;
        }
        let slen = ctx.spectrum.len();
        let mut raw = [0.0_f32; 12];
        for (i, &mag) in ctx.spectrum.iter().enumerate() {
            let hz = bin_hz(i, slen, ctx.sample_rate);
            if !(27.5..=8000.0).contains(&hz) {
                continue;
            }
            let pc = ((12.0 * (hz / 440.0).log2()).round() as i64).rem_euclid(12) as usize;
            raw[pc] += mag;
        }
        let maxv = raw.iter().cloned().fold(1e-6_f32, f32::max);
        let alpha = 1.0 - self.p.smoothing.clamp(0.0, 0.95);
        for (pc, &rv) in raw.iter().enumerate() {
            let v = (rv / maxv).clamp(0.0, 1.0);
            self.chroma[pc] += alpha * (v - self.chroma[pc]);
        }

        let centre = rect.center();
        let radius = rect.size().min_elem() * 0.40;
        let inner = radius * 0.30;

        // Resultant key vector over the fifths circle.
        let (mut kx, mut ky) = (0.0_f32, 0.0_f32);
        for (pos, &pc) in FIFTHS.iter().enumerate() {
            let ang = -TAU * 0.25 + TAU * pos as f32 / 12.0;
            kx += self.chroma[pc] * ang.cos();
            ky += self.chroma[pc] * ang.sin();
        }

        for (pos, &pc) in FIFTHS.iter().enumerate() {
            let ang = -TAU * 0.25 + TAU * pos as f32 / 12.0;
            let v = self.chroma[pc].powf(self.p.gamma);
            let len = inner + v * (radius - inner);
            let (r, g, b) = hsv_to_rgb(pos as f32 / 12.0, 0.7, 0.55 + 0.45 * v);
            let col = egui::Color32::from_rgb(r, g, b);
            let base = egui::pos2(centre.x + ang.cos() * inner, centre.y + ang.sin() * inner);
            let tip = egui::pos2(centre.x + ang.cos() * len, centre.y + ang.sin() * len);
            painter.line_segment([base, tip], egui::Stroke::new(10.0, col));
            let lab = egui::pos2(
                centre.x + ang.cos() * (radius + 14.0),
                centre.y + ang.sin() * (radius + 14.0),
            );
            painter.text(
                lab,
                egui::Align2::CENTER_CENTER,
                NAMES[pc],
                egui::FontId::monospace(12.0),
                egui::Color32::from_gray(190),
            );
        }
        painter.circle_stroke(
            centre,
            inner,
            egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
        );

        // Key needle.
        let klen = (kx * kx + ky * ky).sqrt();
        if klen > 1e-3 {
            let scale = radius * 0.9 / klen.max(1.0);
            let tip = egui::pos2(centre.x + kx * scale, centre.y + ky * scale);
            painter.line_segment(
                [centre, tip],
                egui::Stroke::new(2.5, egui::Color32::from_rgb(255, 240, 180)),
            );
            painter.circle_filled(tip, 4.0, egui::Color32::from_rgb(255, 240, 180));
        }

        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            "Chroma · circle of fifths",
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(160),
        );
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        slider(
            ui,
            "Smoothing",
            "EMA weight on the previous chroma frame.",
            &mut p.smoothing,
            0.0..=0.95,
        );
        slider(
            ui,
            "Gamma",
            "Display gamma on each pitch-class wedge. <1 lifts weaker notes.",
            &mut p.gamma,
            0.3..=1.5,
        );
    }
}
