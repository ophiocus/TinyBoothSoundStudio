//! Recurrence plot (#6) — `R(i,j) = Θ(ε − ‖v_i − v_j‖)` over a delay
//! embedding of the signal. Diagonal stripes = periodicity, blocks =
//! stationarity, fading texture = drift, dust = chaos/noise.

use crate::{blit_image, magma, slider, FrameCtx, VizModule};
use eframe::egui;

const N: usize = 200;

#[derive(Debug, Clone)]
pub struct RecurrenceParams {
    pub tau_ms: f32,
    /// Recurrence threshold as a percentile of all pairwise distances.
    pub eps_pct: f32,
}
impl Default for RecurrenceParams {
    fn default() -> Self {
        Self {
            tau_ms: 3.0,
            eps_pct: 0.12,
        }
    }
}

#[derive(Default)]
pub struct Recurrence {
    p: RecurrenceParams,
    tex: Option<egui::TextureHandle>,
}

impl VizModule for Recurrence {
    fn id(&self) -> &'static str {
        "recurrence"
    }
    fn label(&self) -> &'static str {
        "Recurrence"
    }
    fn description(&self) -> &'static str {
        "Recurrence plot of a delay embedding. Diagonals = periodicity, dust = chaos."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        let x = &ctx.mono;
        if x.len() < 64 || ctx.sample_rate == 0 {
            return;
        }
        let tau = ((self.p.tau_ms * 0.001 * ctx.sample_rate as f32).round() as usize)
            .clamp(1, x.len() / 4);
        let span = x.len().saturating_sub(2 * tau);
        if span < N {
            return;
        }
        // Sample N embedded points (m=3) evenly across the window.
        let stride = (span / N).max(1);
        let mut v = Vec::with_capacity(N);
        for k in 0..N {
            let i = k * stride;
            v.push([x[i], x[i + tau], x[i + 2 * tau]]);
        }
        // Pairwise distances (upper triangle) → percentile threshold.
        let mut dists = Vec::with_capacity(N * N / 2);
        let dist = |a: &[f32; 3], b: &[f32; 3]| -> f32 {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        for i in 0..N {
            for j in (i + 1)..N {
                dists.push(dist(&v[i], &v[j]));
            }
        }
        if dists.is_empty() {
            return;
        }
        dists.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let eps =
            dists[((dists.len() as f32 * self.p.eps_pct) as usize).min(dists.len() - 1)].max(1e-6);

        let mut img = egui::ColorImage::new([N, N], egui::Color32::BLACK);
        for i in 0..N {
            for j in 0..N {
                let d = dist(&v[i], &v[j]);
                if d <= eps {
                    let t = 1.0 - d / eps;
                    let (r, g, b) = magma(0.3 + 0.7 * t);
                    img.pixels[i * N + j] = egui::Color32::from_rgb(r, g, b);
                }
            }
        }
        blit_image(
            painter,
            rect,
            &mut self.tex,
            "viz_recurrence",
            img,
            egui::TextureOptions::NEAREST,
        );
        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!(
                "Recurrence · {N}×{N} · τ={:.1} ms · ε@{:.0}%",
                self.p.tau_ms,
                self.p.eps_pct * 100.0
            ),
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(200),
        );
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        slider(
            ui,
            "Delay τ (ms)",
            "Embedding delay.",
            &mut p.tau_ms,
            0.2..=20.0,
        );
        slider(
            ui,
            "Threshold %",
            "Recurrence threshold ε as a percentile of all pairwise distances. \
             Smaller = sparser, sharper diagonals.",
            &mut p.eps_pct,
            0.02..=0.40,
        );
    }
}
