//! Self-similarity matrix + Foote novelty (#5) — the song's self-
//! portrait. Cosine similarity of the chroma trajectory against itself;
//! repeats become off-diagonal stripes, sections become blocks.

use crate::{bin_hz, blit_image, magma, slider, FrameCtx, VizModule};
use eframe::egui;
use std::collections::VecDeque;

const MAX_N: usize = 240;

#[derive(Debug, Clone)]
pub struct SimilarityParams {
    /// Seconds between captured feature frames (decimation).
    pub hop_secs: f32,
    pub novelty_kernel: usize,
}
impl Default for SimilarityParams {
    fn default() -> Self {
        Self {
            hop_secs: 0.5,
            novelty_kernel: 8,
        }
    }
}

#[derive(Default)]
pub struct Similarity {
    p: SimilarityParams,
    feats: VecDeque<[f32; 12]>,
    last_capture: f64,
    tex: Option<egui::TextureHandle>,
}

fn chroma_of(ctx: &FrameCtx<'_>) -> [f32; 12] {
    let mut c = [0.0_f32; 12];
    let slen = ctx.spectrum.len();
    for (i, &mag) in ctx.spectrum.iter().enumerate() {
        let hz = bin_hz(i, slen, ctx.sample_rate);
        if !(27.5..=8000.0).contains(&hz) {
            continue;
        }
        let pc = ((12.0 * (hz / 440.0).log2()).round() as i64).rem_euclid(12) as usize;
        c[pc] += mag;
    }
    let norm = c.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-6);
    for v in &mut c {
        *v /= norm;
    }
    c
}

fn cosine(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| x * y)
        .sum::<f32>()
        .clamp(0.0, 1.0)
}

impl VizModule for Similarity {
    fn id(&self) -> &'static str {
        "similarity"
    }
    fn label(&self) -> &'static str {
        "Self-Similarity"
    }
    fn description(&self) -> &'static str {
        "Cosine self-similarity of the chroma trajectory + Foote novelty. Song form as geometry."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        if ctx.spectrum.is_empty() || ctx.sample_rate == 0 {
            return;
        }
        if ctx.time - self.last_capture >= self.p.hop_secs as f64 {
            self.feats.push_back(chroma_of(ctx));
            while self.feats.len() > MAX_N {
                self.feats.pop_front();
            }
            self.last_capture = ctx.time;
        }
        let n = self.feats.len();
        if n < 2 {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "building self-similarity… (needs a few seconds of audio)",
                egui::FontId::monospace(12.0),
                egui::Color32::from_gray(130),
            );
            return;
        }
        let feats: Vec<[f32; 12]> = self.feats.iter().copied().collect();
        let mut img = egui::ColorImage::new([n, n], egui::Color32::BLACK);
        for i in 0..n {
            for j in 0..n {
                let s = cosine(&feats[i], &feats[j]);
                let (r, g, b) = magma(s.powf(1.5));
                img.pixels[i * n + j] = egui::Color32::from_rgb(r, g, b);
            }
        }
        blit_image(
            painter,
            rect,
            &mut self.tex,
            "viz_ssm",
            img,
            egui::TextureOptions::LINEAR,
        );

        // Foote novelty: checkerboard correlation along the diagonal.
        let l = self.p.novelty_kernel.max(2).min(n / 2);
        if n > 2 * l {
            let mut nov = vec![0.0_f32; n];
            let mut maxn = 1e-6_f32;
            for c in l..(n - l) {
                let mut acc = 0.0_f32;
                for a in 0..l {
                    for b in 0..l {
                        // four quadrants: ++ / -- positive, +- / -+ negative
                        acc += cosine(&feats[c - 1 - a], &feats[c - 1 - b]);
                        acc += cosine(&feats[c + a], &feats[c + b]);
                        acc -= cosine(&feats[c - 1 - a], &feats[c + b]);
                        acc -= cosine(&feats[c + a], &feats[c - 1 - b]);
                    }
                }
                nov[c] = acc.max(0.0);
                maxn = maxn.max(nov[c]);
            }
            let strip = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.bottom() - 36.0),
                egui::pos2(rect.right(), rect.bottom() - 4.0),
            );
            painter.rect_filled(
                strip,
                2.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 150),
            );
            let mut prev: Option<egui::Pos2> = None;
            for (c, &v) in nov.iter().enumerate() {
                let x = strip.left() + c as f32 / n as f32 * strip.width();
                let y = strip.bottom() - (v / maxn) * strip.height();
                let pt = egui::pos2(x, y);
                if let Some(pp) = prev {
                    painter.line_segment(
                        [pp, pt],
                        egui::Stroke::new(1.4, egui::Color32::from_rgb(255, 220, 120)),
                    );
                }
                prev = Some(pt);
            }
        }

        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!(
                "Self-similarity · {n}×{n} frames · hop {:.1}s",
                self.p.hop_secs
            ),
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(200),
        );
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        slider(
            ui,
            "Hop (s)",
            "Seconds between captured feature frames. Larger = longer history, coarser.",
            &mut p.hop_secs,
            0.1..=2.0,
        );
        slider(
            ui,
            "Novelty kernel",
            "Half-width of the Foote checkerboard kernel (section-boundary detector).",
            &mut p.novelty_kernel,
            2..=24,
        );
        if ui.button("Clear history").clicked() {
            self.feats.clear();
        }
    }
}
