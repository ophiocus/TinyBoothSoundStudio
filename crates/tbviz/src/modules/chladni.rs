//! Chladni cymatics pattern — superposition of sin·sin eigenmodes
//! weighted by FFT bands (Chladni 1787).

use crate::{hsv_to_rgb, slider, FrameCtx, VizModule};
use eframe::egui;

#[derive(Debug, Clone)]
pub struct ChladniParams {
    pub grid_size: usize,
    pub phase_speed: f32,
    pub intensity: f32,
    pub hue_positive: f32,
    pub hue_negative: f32,
}
impl Default for ChladniParams {
    fn default() -> Self {
        Self {
            grid_size: 64,
            phase_speed: 0.02,
            intensity: 0.5,
            hue_positive: 0.55,
            hue_negative: 0.05,
        }
    }
}

#[derive(Default)]
pub struct Chladni {
    p: ChladniParams,
    phase: f32,
}

const MODES: &[(u32, u32)] = &[
    (1, 2),
    (2, 1),
    (2, 3),
    (3, 2),
    (1, 4),
    (4, 1),
    (3, 3),
    (2, 5),
    (5, 2),
    (4, 4),
];

impl VizModule for Chladni {
    fn id(&self) -> &'static str {
        "chladni"
    }
    fn label(&self) -> &'static str {
        "Chladni"
    }
    fn description(&self) -> &'static str {
        "10-mode eigenfunction superposition (Chladni 1787). FFT bands weight each mode."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        let p = self.p.clone();
        let spectrum = &ctx.spectrum;
        if spectrum.is_empty() {
            return;
        }

        let bins_per_mode = (spectrum.len() / MODES.len()).max(1);
        let amplitudes: Vec<f32> = MODES
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let start = i * bins_per_mode;
                let end = (start + bins_per_mode).min(spectrum.len());
                spectrum[start..end].iter().sum::<f32>() / (end - start) as f32
            })
            .collect();

        self.phase += p.phase_speed;
        let phase = self.phase;

        let grid = p.grid_size;
        let cell_w = rect.width() / grid as f32;
        let cell_h = rect.height() / grid as f32;
        for gy in 0..grid {
            for gx in 0..grid {
                let x = gx as f32 / (grid - 1).max(1) as f32;
                let y = gy as f32 / (grid - 1).max(1) as f32;
                let mut psi = 0.0_f32;
                for ((m, n), &amp) in MODES.iter().zip(amplitudes.iter()) {
                    let mf = *m as f32;
                    let nf = *n as f32;
                    let mode = (std::f32::consts::PI * mf * x).sin()
                        * (std::f32::consts::PI * nf * y).sin();
                    let mode_swap = (std::f32::consts::PI * nf * x).sin()
                        * (std::f32::consts::PI * mf * y).sin();
                    psi += amp * (mode - mode_swap) * (phase * (mf + nf) * 0.13).cos();
                }
                let mag = psi.abs().clamp(0.0, 2.0) * p.intensity;
                let hue_t = if psi >= 0.0 {
                    p.hue_positive
                } else {
                    p.hue_negative
                };
                let (r, g, b) = hsv_to_rgb(hue_t, 0.85, mag.clamp(0.0, 1.0));
                if (r as u32 + g as u32 + b as u32) < 24 {
                    continue;
                }
                let cell = egui::Rect::from_min_size(
                    egui::pos2(
                        rect.left() + gx as f32 * cell_w,
                        rect.top() + gy as f32 * cell_h,
                    ),
                    egui::vec2(cell_w + 0.5, cell_h + 0.5),
                );
                painter.rect_filled(cell, 0.0, egui::Color32::from_rgb(r, g, b));
            }
        }
        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!("Chladni · {} modes · grid {}²", MODES.len(), grid),
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(180),
        );
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        slider(
            ui,
            "Grid size",
            "Side length of the rendering grid in cells. The field is evaluated at \
             each cell — 64 is fast and sharp; higher gets pretty but quadratic in cost.",
            &mut p.grid_size,
            16..=128,
        );
        slider(
            ui,
            "Phase speed",
            "How fast the modulation phase drifts each frame. Keeps the figure \
             animated even on steady-state input.",
            &mut p.phase_speed,
            0.0..=0.10,
        );
        slider(
            ui,
            "Intensity",
            "Overall brightness multiplier. Higher = more saturated patterns; \
             too high and the figure clips to white.",
            &mut p.intensity,
            0.1..=2.0,
        );
        slider(
            ui,
            "Hue (positive)",
            "Hue used where ψ > 0 (the bright lobes). 0.55 is teal.",
            &mut p.hue_positive,
            0.0..=1.0,
        );
        slider(
            ui,
            "Hue (negative)",
            "Hue used where ψ < 0 (the dark lobes — Chladni's sand lines).",
            &mut p.hue_negative,
            0.0..=1.0,
        );
    }
}
