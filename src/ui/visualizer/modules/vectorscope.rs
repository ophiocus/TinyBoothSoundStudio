//! Vectorscope + correlation meter (#10) — mid/side goniometer with a
//! running inter-channel correlation readout. The phase-problem X-ray:
//! anti-phase content collapses to a horizontal smear, mono to vertical.

use crate::ui::visualizer::{slider, FrameCtx, VizModule};
use eframe::egui;
use std::f32::consts::FRAC_1_SQRT_2;

#[derive(Debug, Clone)]
pub struct VectorscopeParams {
    pub subsample: usize,
    pub scale: f32,
    pub alpha_floor: u8,
}
impl Default for VectorscopeParams {
    fn default() -> Self {
        Self {
            subsample: 512,
            scale: 0.42,
            alpha_floor: 24,
        }
    }
}

#[derive(Default)]
pub struct Vectorscope {
    p: VectorscopeParams,
    corr: f32,
}

impl VizModule for Vectorscope {
    fn id(&self) -> &'static str {
        "vectorscope"
    }
    fn label(&self) -> &'static str {
        "Vectorscope"
    }
    fn description(&self) -> &'static str {
        "Mid/side goniometer + correlation meter. Mono-compatibility and phase problems at a glance."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        let samples = ctx.samples;
        if samples.len() < 2 {
            return;
        }
        let centre = rect.center();
        let scale = rect.size().min_elem() * self.p.scale;

        // Running Pearson correlation between L and R.
        let mut sl = 0.0;
        let mut sr = 0.0;
        let mut sll = 0.0;
        let mut srr = 0.0;
        let mut slr = 0.0;
        for &(l, r) in samples {
            sl += l;
            sr += r;
            sll += l * l;
            srr += r * r;
            slr += l * r;
        }
        let nn = samples.len() as f32;
        let cov = slr / nn - (sl / nn) * (sr / nn);
        let vl = (sll / nn - (sl / nn).powi(2)).max(0.0);
        let vr = (srr / nn - (sr / nn).powi(2)).max(0.0);
        let denom = (vl * vr).sqrt();
        let inst = if denom > 1e-9 {
            (cov / denom).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.corr += 0.1 * (inst - self.corr);

        // Guides: vertical = mono (in-phase), horizontal = anti-phase.
        let g = egui::Color32::from_gray(40);
        painter.line_segment(
            [
                egui::pos2(centre.x, centre.y - scale),
                egui::pos2(centre.x, centre.y + scale),
            ],
            egui::Stroke::new(1.0, g),
        );
        painter.line_segment(
            [
                egui::pos2(centre.x - scale, centre.y),
                egui::pos2(centre.x + scale, centre.y),
            ],
            egui::Stroke::new(1.0, g),
        );

        // Mid/side rotation: M = (L+R)/√2 on Y (up), S = (L−R)/√2 on X.
        let stride = (samples.len() / self.p.subsample.max(16)).max(1);
        let n = samples.len() / stride;
        let base = egui::Color32::from_rgb(120, 230, 160);
        for i in 1..n {
            let (l1, r1) = samples[(i - 1) * stride];
            let (l2, r2) = samples[i * stride];
            let p1 = egui::pos2(
                centre.x + (l1 - r1) * FRAC_1_SQRT_2 * scale,
                centre.y - (l1 + r1) * FRAC_1_SQRT_2 * scale,
            );
            let p2 = egui::pos2(
                centre.x + (l2 - r2) * FRAC_1_SQRT_2 * scale,
                centre.y - (l2 + r2) * FRAC_1_SQRT_2 * scale,
            );
            let t = i as f32 / n as f32;
            let a = self.p.alpha_floor as i32
                + ((255 - self.p.alpha_floor as i32) * i as i32 / n.max(1) as i32);
            painter.line_segment(
                [p1, p2],
                egui::Stroke::new(
                    1.4,
                    egui::Color32::from_rgba_unmultiplied(
                        base.r(),
                        base.g(),
                        base.b(),
                        a.clamp(0, 255) as u8,
                    ),
                ),
            );
            let _ = t;
        }

        // Correlation meter bar at the bottom (−1 red … +1 green).
        let bar = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 12.0, rect.bottom() - 22.0),
            egui::pos2(rect.right() - 12.0, rect.bottom() - 14.0),
        );
        painter.rect_filled(bar, 2.0, egui::Color32::from_gray(30));
        let cx = bar.left() + (self.corr * 0.5 + 0.5) * bar.width();
        let col = if self.corr < 0.0 {
            egui::Color32::from_rgb(220, 90, 90)
        } else {
            egui::Color32::from_rgb(120, 210, 150)
        };
        painter.line_segment(
            [
                egui::pos2(cx, bar.top() - 3.0),
                egui::pos2(cx, bar.bottom() + 3.0),
            ],
            egui::Stroke::new(2.5, col),
        );
        painter.text(
            egui::pos2(bar.center().x, bar.top() - 8.0),
            egui::Align2::CENTER_BOTTOM,
            format!("correlation {:+.2}", self.corr),
            egui::FontId::monospace(11.0),
            col,
        );

        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            "Vectorscope · M↑ / S→",
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(120),
        );
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        slider(
            ui,
            "Subsample",
            "Points sampled from the buffer.",
            &mut p.subsample,
            64..=2048,
        );
        slider(
            ui,
            "Scale",
            "Plot scale as a fraction of the canvas.",
            &mut p.scale,
            0.20..=0.55,
        );
        slider(
            ui,
            "Alpha floor",
            "Minimum trail alpha (phosphor persistence).",
            &mut p.alpha_floor,
            0..=200,
        );
    }
}
