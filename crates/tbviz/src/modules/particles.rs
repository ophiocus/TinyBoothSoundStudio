//! Particle flow (#11) — a flow field advects particles; band energy
//! drives speed, onsets inject bursts, spectral centroid sets hue.
//! Sound's energy made visible as emergent motion.

use crate::{hsv_to_rgb, slider, FrameCtx, VizModule};
use eframe::egui;

#[derive(Debug, Clone)]
pub struct ParticlesParams {
    pub count: usize,
    pub flow_scale: f32,
    pub speed: f32,
    pub trail: f32,
}
impl Default for ParticlesParams {
    fn default() -> Self {
        Self {
            count: 1200,
            flow_scale: 2.4,
            speed: 0.35,
            trail: 0.6,
        }
    }
}

#[derive(Clone, Copy)]
struct P {
    x: f32,
    y: f32,
    px: f32,
    py: f32,
}

pub struct Particles {
    p: ParticlesParams,
    parts: Vec<P>,
    rng: u32,
    prev_spec: Vec<f32>,
}
impl Default for Particles {
    fn default() -> Self {
        Self {
            p: ParticlesParams::default(),
            parts: Vec::new(),
            rng: 0x12345678,
            prev_spec: Vec::new(),
        }
    }
}

impl Particles {
    fn rand(&mut self) -> f32 {
        // xorshift32 → [0,1)
        let mut s = self.rng;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.rng = s;
        (s as f32 / u32::MAX as f32).clamp(0.0, 1.0)
    }
}

/// Cheap divergence-free-ish flow angle from a sum of sinusoids
/// (a curl-noise stand-in): smooth, swirling, animated by `time`.
fn flow_angle(x: f32, y: f32, t: f32, scale: f32) -> f32 {
    let a = (x * scale + t * 0.3).sin() + (y * scale * 1.3 - t * 0.21).cos();
    let b = (y * scale - t * 0.27).sin() + (x * scale * 0.8 + t * 0.17).cos();
    a.atan2(b) + t * 0.05
}

impl VizModule for Particles {
    fn id(&self) -> &'static str {
        "particles"
    }
    fn label(&self) -> &'static str {
        "Particle Flow"
    }
    fn description(&self) -> &'static str {
        "Curl-noise flow field driven by band energy + onsets. Energy as emergent motion."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        let want = self.p.count.max(16);
        if self.parts.len() != want {
            self.parts.clear();
            for _ in 0..want {
                let x = self.rand();
                let y = self.rand();
                self.parts.push(P { x, y, px: x, py: y });
            }
        }

        // Onset detection: half-wave-rectified spectral flux.
        let mut flux = 0.0_f32;
        if self.prev_spec.len() == ctx.spectrum.len() {
            for (a, b) in ctx.spectrum.iter().zip(self.prev_spec.iter()) {
                let d = a - b;
                if d > 0.0 {
                    flux += d;
                }
            }
            flux /= ctx.spectrum.len().max(1) as f32;
        }
        self.prev_spec = ctx.spectrum.clone();
        let onset = (flux * 20.0).clamp(0.0, 1.0);

        // Faint backdrop fade for trails (rect, low alpha).
        painter.rect_filled(
            rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(8, 8, 12, (255.0 * (1.0 - self.p.trail)) as u8),
        );

        let dt = ctx.dt.clamp(0.0, 0.05);
        let t = ctx.time as f32;
        let speed = self.p.speed * (0.3 + ctx.rms.clamp(0.0, 0.4) * 4.0);
        let hue = (0.55 + ctx.centroid * 0.5).rem_euclid(1.0);

        for part in &mut self.parts {
            let ang = flow_angle(part.x, part.y, t, self.p.flow_scale);
            part.px = part.x;
            part.py = part.y;
            part.x += ang.cos() * speed * dt;
            part.y += ang.sin() * speed * dt;
            // Onset burst: push radially out from centre.
            if onset > 0.2 {
                let dx = part.x - 0.5;
                let dy = part.y - 0.5;
                let r = (dx * dx + dy * dy).sqrt().max(1e-3);
                part.x += dx / r * onset * 0.02;
                part.y += dy / r * onset * 0.02;
            }
            // Wrap.
            part.x = part.x.rem_euclid(1.0);
            part.y = part.y.rem_euclid(1.0);

            let to_screen = |fx: f32, fy: f32| {
                egui::pos2(
                    rect.left() + fx * rect.width(),
                    rect.top() + fy * rect.height(),
                )
            };
            // Skip the seam segment on wrap.
            if (part.x - part.px).abs() < 0.5 && (part.y - part.py).abs() < 0.5 {
                let (r, g, b) = hsv_to_rgb(hue, 0.7, 0.9);
                painter.line_segment(
                    [to_screen(part.px, part.py), to_screen(part.x, part.y)],
                    egui::Stroke::new(1.2, egui::Color32::from_rgba_unmultiplied(r, g, b, 180)),
                );
            }
        }

        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!("Particle flow · {want} · onset {onset:.2}"),
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(140),
        );
        // Keep animating.
        painter.ctx().request_repaint();
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        slider(
            ui,
            "Count",
            "Number of particles.",
            &mut p.count,
            200..=4000,
        );
        slider(
            ui,
            "Flow scale",
            "Spatial frequency of the flow field (more = tighter swirls).",
            &mut p.flow_scale,
            0.5..=6.0,
        );
        slider(
            ui,
            "Speed",
            "Base advection speed (scaled by loudness).",
            &mut p.speed,
            0.05..=1.0,
        );
        slider(
            ui,
            "Trail",
            "Trail persistence (higher = longer streaks).",
            &mut p.trail,
            0.0..=0.95,
        );
    }
}
