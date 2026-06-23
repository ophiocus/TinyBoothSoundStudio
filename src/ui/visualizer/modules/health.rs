//! Health / info ribbons (#13) — scrolling spectral entropy, flatness,
//! centroid, and crest factor. Insight = inaudible structure mapped to a
//! preattentive channel: over-compression, monotony, and noisiness pop.

use crate::ui::visualizer::{slider, FrameCtx, VizModule};
use eframe::egui;
use std::collections::VecDeque;

const HIST: usize = 600;

#[derive(Debug, Clone)]
pub struct HealthParams {
    pub smoothing: f32,
}
impl Default for HealthParams {
    fn default() -> Self {
        Self { smoothing: 0.3 }
    }
}

#[derive(Default)]
pub struct Health {
    p: HealthParams,
    entropy: VecDeque<f32>,
    flatness: VecDeque<f32>,
    centroid: VecDeque<f32>,
    crest: VecDeque<f32>,
    sm: [f32; 4],
}

fn push_cap(q: &mut VecDeque<f32>, v: f32) {
    q.push_back(v);
    while q.len() > HIST {
        q.pop_front();
    }
}

impl VizModule for Health {
    fn id(&self) -> &'static str {
        "health"
    }
    fn label(&self) -> &'static str {
        "Health"
    }
    fn description(&self) -> &'static str {
        "Spectral entropy / flatness / centroid + crest factor ribbons. Surfaces defects at a glance."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        if ctx.spectrum.is_empty() {
            return;
        }
        let spec = &ctx.spectrum;
        let total: f32 = spec.iter().sum::<f32>().max(1e-9);
        // Spectral entropy (normalized 0..1).
        let mut h = 0.0_f32;
        for &m in spec {
            let p = m / total;
            if p > 1e-9 {
                h -= p * p.log2();
            }
        }
        let entropy = (h / (spec.len() as f32).log2()).clamp(0.0, 1.0);
        // Spectral flatness = geomean / mean (scale-invariant).
        let mut log_sum = 0.0_f32;
        let mut n_nz = 0u32;
        for &m in spec {
            if m > 1e-6 {
                log_sum += m.ln();
                n_nz += 1;
            }
        }
        let geomean = if n_nz > 0 {
            (log_sum / n_nz as f32).exp()
        } else {
            0.0
        };
        let mean = total / spec.len() as f32;
        let flatness = (geomean / mean.max(1e-9)).clamp(0.0, 1.0);
        let centroid = ctx.centroid.clamp(0.0, 1.0);
        // Crest factor proxy: peak / rms over the tap, mapped to 0..1
        // (smaller = more compressed). 1.0 ≈ crest 20 dB.
        let peak = ctx
            .samples
            .iter()
            .fold(0.0_f32, |a, &(l, r)| a.max(l.abs()).max(r.abs()));
        let crest_db = 20.0 * (peak.max(1e-5) / ctx.rms.max(1e-5)).log10();
        let crest = (crest_db / 20.0).clamp(0.0, 1.0);

        let alpha = 1.0 - self.p.smoothing.clamp(0.0, 0.95);
        self.sm[0] += alpha * (entropy - self.sm[0]);
        self.sm[1] += alpha * (flatness - self.sm[1]);
        self.sm[2] += alpha * (centroid - self.sm[2]);
        self.sm[3] += alpha * (crest - self.sm[3]);
        push_cap(&mut self.entropy, self.sm[0]);
        push_cap(&mut self.flatness, self.sm[1]);
        push_cap(&mut self.centroid, self.sm[2]);
        push_cap(&mut self.crest, self.sm[3]);

        let lanes: [(&str, &VecDeque<f32>, egui::Color32, f32); 4] = [
            (
                "entropy",
                &self.entropy,
                egui::Color32::from_rgb(120, 200, 255),
                self.sm[0],
            ),
            (
                "flatness (noisiness)",
                &self.flatness,
                egui::Color32::from_rgb(200, 160, 255),
                self.sm[1],
            ),
            (
                "centroid (brightness)",
                &self.centroid,
                egui::Color32::from_rgb(255, 200, 120),
                self.sm[2],
            ),
            (
                "crest (dynamics)",
                &self.crest,
                egui::Color32::from_rgb(150, 230, 170),
                self.sm[3],
            ),
        ];
        let lane_h = rect.height() / 4.0;
        for (li, (name, q, col, cur)) in lanes.iter().enumerate() {
            let top = rect.top() + li as f32 * lane_h;
            let lane = egui::Rect::from_min_max(
                egui::pos2(rect.left(), top + 2.0),
                egui::pos2(rect.right(), top + lane_h - 2.0),
            );
            painter.rect_filled(lane, 2.0, egui::Color32::from_rgb(16, 16, 22));
            let n = q.len();
            if n > 1 {
                let mut prev: Option<egui::Pos2> = None;
                for (i, &v) in q.iter().enumerate() {
                    let x = lane.left() + i as f32 / HIST as f32 * lane.width();
                    let y = lane.bottom() - v.clamp(0.0, 1.0) * lane.height();
                    let pt = egui::pos2(x, y);
                    if let Some(p) = prev {
                        painter.line_segment([p, pt], egui::Stroke::new(1.4, *col));
                    }
                    prev = Some(pt);
                }
            }
            painter.text(
                lane.left_top() + egui::vec2(6.0, 4.0),
                egui::Align2::LEFT_TOP,
                format!("{name}  {:.2}", cur),
                egui::FontId::monospace(11.0),
                *col,
            );
        }
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        slider(
            ui,
            "Smoothing",
            "EMA weight on the previous reading for each metric.",
            &mut self.p.smoothing,
            0.0..=0.9,
        );
        ui.label(
            egui::RichText::new(
                "Low crest = over-compressed. High flatness = noisy/AI-flat. \
                 Flat entropy = monotonous mix.",
            )
            .italics()
            .weak()
            .small(),
        );
    }
}
