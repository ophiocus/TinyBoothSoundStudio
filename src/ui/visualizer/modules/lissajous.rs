//! Lissajous goniometer with phosphor trails — L vs R XY plot.
//! Phase relationships at the sample timescale.

use crate::ui::visualizer::{slider, FrameCtx, VizModule};
use eframe::egui;

#[derive(Debug, Clone)]
pub struct LissajousParams {
    pub subsample_target: usize,
    pub alpha_floor: u8,
    pub scale_factor: f32,
    pub stroke_width: f32,
    pub show_guides: bool,
}
impl Default for LissajousParams {
    fn default() -> Self {
        Self {
            subsample_target: 512,
            alpha_floor: 30,
            scale_factor: 0.45,
            stroke_width: 1.5,
            show_guides: true,
        }
    }
}

#[derive(Default)]
pub struct Lissajous {
    p: LissajousParams,
}

impl VizModule for Lissajous {
    fn id(&self) -> &'static str {
        "lissajous"
    }
    fn label(&self) -> &'static str {
        "Lissajous"
    }
    fn description(&self) -> &'static str {
        "L vs R XY plot with phosphor trails. Phase relationships at the sample timescale."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        let p = &self.p;
        let samples = ctx.samples;
        let centre = rect.center();
        let scale = rect.size().min_elem() * p.scale_factor;

        let stride = (samples.len() / p.subsample_target).max(1);
        let n = samples.len() / stride;
        let phosphor_base = egui::Color32::from_rgb(120, 230, 160);
        for i in 1..n {
            let (l1, r1) = samples[(i - 1) * stride];
            let (l2, r2) = samples[i * stride];
            let p1 = egui::pos2(centre.x + l1 * scale, centre.y - r1 * scale);
            let p2 = egui::pos2(centre.x + l2 * scale, centre.y - r2 * scale);
            let alpha_range = 255 - p.alpha_floor as i32;
            let alpha = p.alpha_floor as i32 + alpha_range * i as i32 / n.max(1) as i32;
            let col = egui::Color32::from_rgba_unmultiplied(
                phosphor_base.r(),
                phosphor_base.g(),
                phosphor_base.b(),
                alpha.clamp(0, 255) as u8,
            );
            painter.line_segment([p1, p2], egui::Stroke::new(p.stroke_width, col));
        }

        if p.show_guides {
            let g = egui::Color32::from_gray(40);
            painter.line_segment(
                [
                    egui::pos2(centre.x - scale, centre.y),
                    egui::pos2(centre.x + scale, centre.y),
                ],
                egui::Stroke::new(1.0, g),
            );
            painter.line_segment(
                [
                    egui::pos2(centre.x, centre.y - scale),
                    egui::pos2(centre.x, centre.y + scale),
                ],
                egui::Stroke::new(1.0, g),
            );
        }
        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            "Lissajous · L↔R phase",
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(110),
        );
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        slider(
            ui,
            "Subsample",
            "Number of points sampled from the input buffer for the polyline. \
             Higher = smoother curve, more draw calls.",
            &mut p.subsample_target,
            64..=2048,
        );
        slider(
            ui,
            "Alpha floor",
            "Minimum trail alpha. Higher = older samples are more visible \
             (longer phosphor persistence). 30 is the original default.",
            &mut p.alpha_floor,
            0..=200,
        );
        slider(
            ui,
            "Scale",
            "Plot scale as a fraction of the canvas's shorter dimension. \
             0.45 leaves room for the crosshair guides at the edges.",
            &mut p.scale_factor,
            0.10..=0.60,
        );
        slider(
            ui,
            "Stroke width",
            "Line thickness for the trail in pixels.",
            &mut p.stroke_width,
            0.5..=4.0,
        );
        ui.checkbox(&mut p.show_guides, "Show guides")
            .on_hover_text(
                "Draw the horizontal + vertical reference lines through the centre. \
                 Useful for spotting mono content (vertical) vs anti-phase (45°).",
            );
    }
}
