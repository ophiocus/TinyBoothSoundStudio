//! SOM timbre map (#9) — a self-organizing map streams the live feature
//! vector onto a 2-D grid that discovers its own axes. The U-matrix
//! shows cluster structure; a comet marks where the current sound sits.
//! The autonomous-dimensioning showpiece.

use crate::{blit_image, magma, slider, FrameCtx, VizModule};
use eframe::egui;
use std::collections::VecDeque;

const K: usize = 24; // grid side
const D: usize = 7; // feature dims

#[derive(Debug, Clone)]
pub struct SomParams {
    pub learn_rate: f32,
    pub radius: f32,
}
impl Default for SomParams {
    fn default() -> Self {
        Self {
            learn_rate: 0.08,
            radius: 3.0,
        }
    }
}

pub struct Som {
    p: SomParams,
    weights: Vec<[f32; D]>, // K*K nodes
    rng: u32,
    trail: VecDeque<(f32, f32)>,
    tex: Option<egui::TextureHandle>,
}
impl Default for Som {
    fn default() -> Self {
        Self {
            p: SomParams::default(),
            weights: Vec::new(),
            rng: 0x9e3779b9,
            trail: VecDeque::with_capacity(64),
            tex: None,
        }
    }
}

impl Som {
    fn rand(&mut self) -> f32 {
        let mut s = self.rng;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.rng = s;
        s as f32 / u32::MAX as f32
    }
}

/// 7-D timbre feature: centroid, flatness, rms, + 4 log-band energies.
fn feature(ctx: &FrameCtx<'_>) -> [f32; D] {
    let spec = &ctx.spectrum;
    let slen = spec.len().max(1);
    let band = |a: f32, b: f32| -> f32 {
        let i0 = (a * slen as f32) as usize;
        let i1 = ((b * slen as f32) as usize).max(i0 + 1).min(slen);
        let s: f32 = spec[i0..i1].iter().sum();
        (s / (i1 - i0) as f32).clamp(0.0, 1.0)
    };
    let total: f32 = spec.iter().sum::<f32>().max(1e-9);
    let mut log_sum = 0.0;
    let mut nz = 0u32;
    for &m in spec {
        if m > 1e-6 {
            log_sum += m.ln();
            nz += 1;
        }
    }
    let flatness = if nz > 0 {
        ((log_sum / nz as f32).exp() / (total / slen as f32).max(1e-9)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    [
        ctx.centroid.clamp(0.0, 1.0),
        flatness,
        ctx.rms.clamp(0.0, 0.4) / 0.4,
        band(0.0, 0.08),
        band(0.08, 0.25),
        band(0.25, 0.5),
        band(0.5, 1.0),
    ]
}

fn dist2(a: &[f32; D], b: &[f32; D]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y) * (x - y)).sum()
}

impl VizModule for Som {
    fn id(&self) -> &'static str {
        "som"
    }
    fn label(&self) -> &'static str {
        "Timbre Map"
    }
    fn description(&self) -> &'static str {
        "Self-organizing map of live timbre features. Discovers its own axes; comet = current sound."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        if ctx.spectrum.is_empty() {
            return;
        }
        if self.weights.len() != K * K {
            self.weights = (0..K * K)
                .map(|_| {
                    let mut w = [0.0; D];
                    for v in &mut w {
                        *v = self.rand();
                    }
                    w
                })
                .collect();
        }

        let f = feature(ctx);
        // Best matching unit.
        let mut bmu = 0;
        let mut best = f32::MAX;
        for (i, w) in self.weights.iter().enumerate() {
            let d = dist2(&f, w);
            if d < best {
                best = d;
                bmu = i;
            }
        }
        let (bx, by) = ((bmu % K) as f32, (bmu / K) as f32);
        // Neighborhood update.
        let lr = self.p.learn_rate;
        let rad = self.p.radius.max(0.5);
        for gy in 0..K {
            for gx in 0..K {
                let dd = (gx as f32 - bx).powi(2) + (gy as f32 - by).powi(2);
                let h = (-dd / (2.0 * rad * rad)).exp();
                if h < 0.01 {
                    continue;
                }
                let w = &mut self.weights[gy * K + gx];
                for k in 0..D {
                    w[k] += lr * h * (f[k] - w[k]);
                }
            }
        }

        // U-matrix: each node's mean distance to its 4-neighbours.
        let mut umax = 1e-6_f32;
        let mut umat = vec![0.0_f32; K * K];
        for gy in 0..K {
            for gx in 0..K {
                let mut acc = 0.0;
                let mut cnt = 0;
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let nx = gx as i32 + dx;
                    let ny = gy as i32 + dy;
                    if nx >= 0 && nx < K as i32 && ny >= 0 && ny < K as i32 {
                        acc += dist2(
                            &self.weights[gy * K + gx],
                            &self.weights[ny as usize * K + nx as usize],
                        )
                        .sqrt();
                        cnt += 1;
                    }
                }
                let v = acc / cnt.max(1) as f32;
                umat[gy * K + gx] = v;
                umax = umax.max(v);
            }
        }
        let mut img = egui::ColorImage::new([K, K], egui::Color32::BLACK);
        for (i, &uv) in umat.iter().enumerate() {
            let (r, g, b) = magma((uv / umax).powf(0.7));
            img.pixels[i] = egui::Color32::from_rgb(r, g, b);
        }
        blit_image(
            painter,
            rect,
            &mut self.tex,
            "viz_som",
            img,
            egui::TextureOptions::LINEAR,
        );

        // Comet for the BMU trajectory.
        let to_screen = |gx: f32, gy: f32| {
            egui::pos2(
                rect.left() + (gx + 0.5) / K as f32 * rect.width(),
                rect.top() + (gy + 0.5) / K as f32 * rect.height(),
            )
        };
        self.trail.push_back((bx, by));
        while self.trail.len() > 48 {
            self.trail.pop_front();
        }
        let tn = self.trail.len();
        let mut prev: Option<egui::Pos2> = None;
        for (i, &(gx, gy)) in self.trail.iter().enumerate() {
            let pt = to_screen(gx, gy);
            let t = i as f32 / tn.max(1) as f32;
            if let Some(pp) = prev {
                painter.line_segment(
                    [pp, pt],
                    egui::Stroke::new(
                        1.0 + 2.0 * t,
                        egui::Color32::from_rgba_unmultiplied(
                            120,
                            220,
                            255,
                            (60.0 + 195.0 * t) as u8,
                        ),
                    ),
                );
            }
            prev = Some(pt);
        }
        painter.circle_filled(to_screen(bx, by), 5.0, egui::Color32::WHITE);

        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!("Timbre map · SOM {K}×{K} · {D}-D features"),
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(200),
        );
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        slider(
            ui,
            "Learn rate",
            "SOM update step. Higher = faster adaptation, less stable map.",
            &mut p.learn_rate,
            0.01..=0.3,
        );
        slider(
            ui,
            "Radius",
            "Neighbourhood radius (grid cells) for each update.",
            &mut p.radius,
            0.5..=8.0,
        );
        if ui.button("Reset map").clicked() {
            self.weights.clear();
            self.trail.clear();
        }
    }
}
