//! Lorenz attractor (audio-modulated). RK4 integration of the Lorenz
//! ODE with σ/ρ/β tugged by spectral centroid + RMS.

use crate::{hsv_to_rgb, slider, FrameCtx, VizModule};
use eframe::egui;

#[derive(Debug, Clone)]
pub struct LorenzParams {
    pub sigma_base: f32,
    pub rho_base: f32,
    pub beta_base: f32,
    pub centroid_drive: f32,
    pub rms_rho_drive: f32,
    pub rms_beta_drive: f32,
    pub dt: f32,
    pub steps_per_frame: usize,
    pub trail_length: usize,
    pub stroke_width: f32,
}
impl Default for LorenzParams {
    fn default() -> Self {
        Self {
            sigma_base: 10.0,
            rho_base: 26.0,
            beta_base: 8.0 / 3.0,
            centroid_drive: 6.0,
            rms_rho_drive: 12.0,
            rms_beta_drive: 0.6,
            dt: 0.005,
            steps_per_frame: 12,
            trail_length: 2000,
            stroke_width: 1.4,
        }
    }
}

pub struct Lorenz {
    p: LorenzParams,
    state: (f32, f32, f32),
    trail: Vec<(f32, f32, f32)>,
}
impl Default for Lorenz {
    fn default() -> Self {
        Self {
            p: LorenzParams::default(),
            state: (0.1, 0.0, 0.0),
            trail: Vec::new(),
        }
    }
}

impl VizModule for Lorenz {
    fn id(&self) -> &'static str {
        "lorenz"
    }
    fn label(&self) -> &'static str {
        "Lorenz"
    }
    fn description(&self) -> &'static str {
        "Audio-modulated Lorenz attractor. RK4 ODE integration. σ/ρ/β tugged by spectrum."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        let p = self.p.clone();
        if self.trail.capacity() < p.trail_length {
            self.trail.reserve(p.trail_length - self.trail.capacity());
        }
        let rms = ctx.rms;
        let centroid = ctx.centroid;
        let sigma = p.sigma_base + centroid.clamp(0.0, 1.0) * p.centroid_drive;
        let rho = p.rho_base + rms.clamp(0.0, 0.4) * p.rms_rho_drive;
        let beta = p.beta_base + rms.clamp(0.0, 0.4) * p.rms_beta_drive;

        for _ in 0..p.steps_per_frame {
            self.state = rk4_lorenz(self.state, sigma, rho, beta, p.dt);
            if self.trail.len() >= p.trail_length {
                self.trail.remove(0);
            }
            self.trail.push(self.state);
        }

        let (mut min_x, mut max_x, mut min_z, mut max_z) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &(x, _y, z) in &self.trail {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_z = min_z.min(z);
            max_z = max_z.max(z);
        }
        let span_x = (max_x - min_x).max(1e-3);
        let span_z = (max_z - min_z).max(1e-3);
        let scale = (rect.width() / span_x).min(rect.height() / span_z) * 0.92;
        let project = |x: f32, z: f32| -> egui::Pos2 {
            egui::pos2(
                rect.center().x + (x - 0.5 * (min_x + max_x)) * scale,
                rect.center().y - (z - 0.5 * (min_z + max_z)) * scale,
            )
        };

        let n = self.trail.len();
        for i in 1..n {
            let (x1, _, z1) = self.trail[i - 1];
            let (x2, _, z2) = self.trail[i];
            let t = i as f32 / n as f32;
            let col = hsv_to_rgb(0.7 - t * 0.7, 0.6, 0.85 + 0.15 * t);
            let alpha = 50 + (205.0 * t) as u8;
            let stroke = egui::Stroke::new(
                p.stroke_width,
                egui::Color32::from_rgba_unmultiplied(col.0, col.1, col.2, alpha),
            );
            painter.line_segment([project(x1, z1), project(x2, z2)], stroke);
        }

        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!(
                "Lorenz · σ={sigma:.1}  ρ={rho:.1}  β={beta:.2}    \
                 centroid={centroid:.2}  rms={rms:.3}"
            ),
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(120),
        );
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        ui.label(egui::RichText::new("ODE base parameters").strong().small());
        slider(
            ui,
            "σ base",
            "Baseline σ in the Lorenz ODE. Default 10 is the canonical chaotic regime. \
             Spectral centroid offsets this in real time by ±centroid_drive.",
            &mut p.sigma_base,
            4.0..=20.0,
        );
        slider(
            ui,
            "ρ base",
            "Baseline ρ. 28 is canonical chaos; below ~24 the orbit collapses to fixed points.",
            &mut p.rho_base,
            20.0..=40.0,
        );
        slider(
            ui,
            "β base",
            "Baseline β. 8/3 ≈ 2.667 is canonical.",
            &mut p.beta_base,
            1.0..=5.0,
        );
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Audio coupling").strong().small());
        slider(
            ui,
            "Centroid → σ",
            "How much normalised spectral centroid (0..1) shifts σ. Larger = more visible \
             response to brightness changes in the audio.",
            &mut p.centroid_drive,
            0.0..=12.0,
        );
        slider(
            ui,
            "RMS → ρ",
            "How much loudness (RMS, clamped to [0, 0.4]) shifts ρ.",
            &mut p.rms_rho_drive,
            0.0..=20.0,
        );
        slider(
            ui,
            "RMS → β",
            "How much loudness shifts β.",
            &mut p.rms_beta_drive,
            0.0..=2.0,
        );
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Integration").strong().small());
        slider(
            ui,
            "dt",
            "RK4 timestep. 0.005 is stable across the parameter range we explore. \
             Larger dt = faster orbit, less stable.",
            &mut p.dt,
            0.001..=0.02,
        );
        slider(
            ui,
            "Steps / frame",
            "How many integration steps per render frame. Higher = faster orbital motion.",
            &mut p.steps_per_frame,
            1..=64,
        );
        slider(
            ui,
            "Trail length",
            "How many recent points to draw as the trail.",
            &mut p.trail_length,
            100..=8000,
        );
        slider(
            ui,
            "Stroke width",
            "Trail line thickness in pixels.",
            &mut p.stroke_width,
            0.5..=4.0,
        );
    }
}

fn rk4_lorenz(s: (f32, f32, f32), sigma: f32, rho: f32, beta: f32, dt: f32) -> (f32, f32, f32) {
    let f = |x: f32, y: f32, z: f32| -> (f32, f32, f32) {
        (sigma * (y - x), x * (rho - z) - y, x * y - beta * z)
    };
    let (x, y, z) = s;
    let (k1x, k1y, k1z) = f(x, y, z);
    let (k2x, k2y, k2z) = f(x + 0.5 * dt * k1x, y + 0.5 * dt * k1y, z + 0.5 * dt * k1z);
    let (k3x, k3y, k3z) = f(x + 0.5 * dt * k2x, y + 0.5 * dt * k2y, z + 0.5 * dt * k2z);
    let (k4x, k4y, k4z) = f(x + dt * k3x, y + dt * k3y, z + dt * k3z);
    (
        x + dt * (k1x + 2.0 * k2x + 2.0 * k3x + k4x) / 6.0,
        y + dt * (k1y + 2.0 * k2y + 2.0 * k3y + k4y) / 6.0,
        z + dt * (k1z + 2.0 * k2z + 2.0 * k3z + k4z) / 6.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rk4_lorenz_is_stable_on_default_params() {
        let mut s = (0.1, 0.0, 0.0);
        for _ in 0..10_000 {
            s = rk4_lorenz(s, 10.0, 28.0, 8.0 / 3.0, 0.005);
        }
        assert!(s.0.abs() < 50.0 && s.1.abs() < 50.0 && s.2.abs() < 60.0);
    }
}
