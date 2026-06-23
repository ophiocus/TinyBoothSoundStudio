//! Phase portrait (#1/#6) — delay-coordinate embedding `x(t)` vs
//! `x(t+τ)` of the mono signal, drawn as a glowing orbit. A periodic
//! tone closes into a clean loop; noise fills the plane; chaos knots.

use crate::ui::visualizer::{hsv_to_rgb, slider, FrameCtx, VizModule};
use eframe::egui;

#[derive(Debug, Clone)]
pub struct PhasePortraitParams {
    /// Delay τ in milliseconds.
    pub tau_ms: f32,
    pub points: usize,
    pub scale: f32,
    pub hue: f32,
}
impl Default for PhasePortraitParams {
    fn default() -> Self {
        Self {
            tau_ms: 3.0,
            points: 1500,
            scale: 0.42,
            hue: 0.55,
        }
    }
}

#[derive(Default)]
pub struct PhasePortrait {
    p: PhasePortraitParams,
}

impl VizModule for PhasePortrait {
    fn id(&self) -> &'static str {
        "phase_portrait"
    }
    fn label(&self) -> &'static str {
        "Phase Portrait"
    }
    fn description(&self) -> &'static str {
        "Delay embedding x(t) vs x(t+τ). Periodic = closed loop, chaos = knot, noise = cloud."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        let x = &ctx.mono;
        if x.len() < 64 || ctx.sample_rate == 0 {
            return;
        }
        let tau = ((self.p.tau_ms * 0.001 * ctx.sample_rate as f32).round() as usize)
            .clamp(1, x.len() / 2);
        let avail = x.len() - tau;
        let want = self.p.points.min(avail);
        let start = avail.saturating_sub(want);

        let centre = rect.center();
        let scale = rect.size().min_elem() * self.p.scale;
        // Normalize by the window's peak so the orbit fills the frame.
        let peak = x[start..].iter().fold(1e-4_f32, |a, &v| a.max(v.abs()));
        let s = scale / peak;

        let mut prev: Option<egui::Pos2> = None;
        for i in start..avail {
            let px = centre.x + x[i] * s;
            let py = centre.y - x[i + tau] * s;
            let p = egui::pos2(px, py);
            if let Some(q) = prev {
                let t = (i - start) as f32 / want.max(1) as f32;
                let (r, g, b) = hsv_to_rgb(self.p.hue + t * 0.12, 0.7, 0.9);
                let a = 40 + (200.0 * t) as u8;
                painter.line_segment(
                    [q, p],
                    egui::Stroke::new(1.3, egui::Color32::from_rgba_unmultiplied(r, g, b, a)),
                );
            }
            prev = Some(p);
        }

        let gd = egui::Color32::from_gray(36);
        painter.line_segment(
            [
                egui::pos2(centre.x - scale, centre.y),
                egui::pos2(centre.x + scale, centre.y),
            ],
            egui::Stroke::new(1.0, gd),
        );
        painter.line_segment(
            [
                egui::pos2(centre.x, centre.y - scale),
                egui::pos2(centre.x, centre.y + scale),
            ],
            egui::Stroke::new(1.0, gd),
        );
        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!("Phase portrait · τ={:.1} ms ({tau} samp)", self.p.tau_ms),
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(150),
        );
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        slider(
            ui,
            "Delay τ (ms)",
            "Delay between the two embedding axes. Sweep to unfold the orbit; \
             a good τ (first mutual-information minimum) opens the attractor.",
            &mut p.tau_ms,
            0.2..=20.0,
        );
        slider(
            ui,
            "Points",
            "How many embedded points to draw.",
            &mut p.points,
            200..=4000,
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
            "Hue",
            "Base hue of the orbit gradient.",
            &mut p.hue,
            0.0..=1.0,
        );
    }
}
