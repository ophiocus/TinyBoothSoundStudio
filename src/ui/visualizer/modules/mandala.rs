//! Spectral mandala — radial FFT. Magnitude as petal length, hue
//! tracking frequency, mirrored across X for symmetry. Note-timescale
//! tonal balance.

use crate::ui::visualizer::{hsv_to_rgb, slider, FrameCtx, VizModule};
use eframe::egui;
use std::f32::consts::TAU;

#[derive(Debug, Clone)]
pub struct MandalaParams {
    pub bin_count: usize,
    pub inner_radius_frac: f32,
    pub hue_start: f32,
    pub hue_range: f32,
    pub saturation: f32,
    pub value: f32,
    pub stroke_width: f32,
    pub smoothing: f32,
}
impl Default for MandalaParams {
    fn default() -> Self {
        Self {
            bin_count: 256,
            inner_radius_frac: 0.18,
            hue_start: 0.95,
            hue_range: 0.85,
            saturation: 0.85,
            value: 0.95,
            stroke_width: 2.0,
            smoothing: 0.4,
        }
    }
}

#[derive(Default)]
pub struct Mandala {
    p: MandalaParams,
    /// Exponentially-averaged spectrum for temporal smoothing.
    smoothed_spectrum: Vec<f32>,
}

impl VizModule for Mandala {
    fn id(&self) -> &'static str {
        "mandala"
    }
    fn label(&self) -> &'static str {
        "Mandala"
    }
    fn description(&self) -> &'static str {
        "Radial FFT. Frequencies arranged around the centre. Note-timescale tonal balance."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        let p = self.p.clone();
        let centre = rect.center();
        let max_radius = rect.size().min_elem() * 0.45;
        let inner = max_radius * p.inner_radius_frac;

        let raw_spectrum = &ctx.spectrum;
        if raw_spectrum.is_empty() {
            return;
        }

        // Temporal smoothing: shown[i] = (1-α)·shown[i] + α·raw[i].
        // The user-set smoothing is the WEIGHT on the OLD value, so
        // α = 1 - smoothing.
        if self.smoothed_spectrum.len() != raw_spectrum.len() {
            self.smoothed_spectrum = raw_spectrum.clone();
        }
        let alpha = 1.0 - p.smoothing.clamp(0.0, 0.99);
        for (s, &r) in self.smoothed_spectrum.iter_mut().zip(raw_spectrum.iter()) {
            *s = (1.0 - alpha) * *s + alpha * r;
        }

        let usable = &self.smoothed_spectrum[1..(self.smoothed_spectrum.len() - 1).max(1)];
        let n = usable.len().min(p.bin_count);
        if n == 0 {
            return;
        }

        let bin_arc = TAU / n as f32;
        for (i, &mag) in usable.iter().take(n).enumerate() {
            let angle_top = -TAU * 0.25 + bin_arc * i as f32 * 0.5;
            let angle_bot = -TAU * 0.25 - bin_arc * i as f32 * 0.5;
            let length = mag.clamp(0.0, 1.0) * (max_radius - inner);
            let t = i as f32 / n as f32;
            let hue = (p.hue_start - t * p.hue_range).rem_euclid(1.0);
            let col = hsv_to_rgb(hue, p.saturation, p.value);
            let col = egui::Color32::from_rgba_unmultiplied(col.0, col.1, col.2, 220);
            let stroke = egui::Stroke::new(p.stroke_width, col);
            for &angle in &[angle_top, angle_bot] {
                let pa = egui::pos2(
                    centre.x + angle.cos() * inner,
                    centre.y + angle.sin() * inner,
                );
                let pb = egui::pos2(
                    centre.x + angle.cos() * (inner + length),
                    centre.y + angle.sin() * (inner + length),
                );
                painter.line_segment([pa, pb], stroke);
            }
        }

        painter.circle_stroke(
            centre,
            inner,
            egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
        );
        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!(
                "Mandala · {} bins · smoothing α={:.2}",
                n,
                1.0 - p.smoothing
            ),
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(110),
        );
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        slider(
            ui,
            "Bins",
            "Number of FFT bins drawn as petals. Higher = finer resolution, \
             more draw calls. The actual FFT runs at a fixed size; this just \
             caps how many bins we plot.",
            &mut p.bin_count,
            32..=512,
        );
        slider(
            ui,
            "Smoothing",
            "Exponential moving average on the spectrum: shown[i] = (1−α) × shown[i] + α × spec[i]. \
             0 = no smoothing (jittery, responds instantly). 1 = fully smoothed (frozen). \
             The visible jerkiness on AI-generated audio is partly diagnostic of \
             band-decorrelated micro-flicker.",
            &mut p.smoothing,
            0.0..=0.95,
        );
        slider(
            ui,
            "Inner radius",
            "Inner radius as a fraction of the outer radius. Larger = more empty \
             centre, shorter petals.",
            &mut p.inner_radius_frac,
            0.05..=0.50,
        );
        slider(
            ui,
            "Hue start",
            "Hue offset (0–1) for the lowest-frequency bin. Other bins rotate \
             around the colour wheel from there.",
            &mut p.hue_start,
            0.0..=1.0,
        );
        slider(
            ui,
            "Hue range",
            "Total fraction of the colour wheel the bins span. 0.85 stops just \
             short of wrapping all the way back to red.",
            &mut p.hue_range,
            0.1..=1.0,
        );
        slider(
            ui,
            "Saturation",
            "HSV saturation for the petal hue.",
            &mut p.saturation,
            0.0..=1.0,
        );
        slider(
            ui,
            "Brightness",
            "HSV value (brightness) for the petal hue.",
            &mut p.value,
            0.0..=1.0,
        );
        slider(
            ui,
            "Stroke width",
            "Petal line thickness in pixels.",
            &mut p.stroke_width,
            0.5..=6.0,
        );
    }
}
