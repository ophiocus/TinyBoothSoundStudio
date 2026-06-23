//! Topological data analysis — SW1PerS periodicity barcode (#15).
//! Periodicity is a *hole* in the right space: a sliding-window
//! embedding of a periodic signal traces a loop, i.e. a persistent H₁
//! class. We embed the signal, build a Vietoris–Rips complex on the
//! sphere-normalized point cloud, compute H₁ persistence by the standard
//! Z/2 matrix reduction, and draw the barcode + a periodicity score.
//!
//! A clean tone grows one long bar; noise none; a tritone an extra hole.

use crate::{slider, FrameCtx, VizModule};
use eframe::egui;
use std::collections::HashMap;

const N: usize = 32; // point-cloud size (keeps Rips tractable per frame)

#[derive(Debug, Clone)]
pub struct TdaParams {
    pub tau_ms: f32,
    pub window_dims: usize,
    pub recompute_secs: f32,
}
impl Default for TdaParams {
    fn default() -> Self {
        Self {
            tau_ms: 2.0,
            window_dims: 20,
            recompute_secs: 0.25,
        }
    }
}

#[derive(Default)]
pub struct Tda {
    p: TdaParams,
    bars: Vec<(f32, f32)>,
    cloud2d: Vec<(f32, f32)>,
    max_filt: f32,
    last_compute: f64,
}

/// Compute the finite H₁ persistence bars of the Vietoris–Rips complex
/// of `n` points given a symmetric distance matrix `d`. Standard Z/2
/// boundary-matrix reduction restricted to the H₁ (edge→triangle) pair.
/// Returns `(birth, death)` pairs (filtration values).
#[allow(clippy::needless_range_loop)] // symmetric index access into `d`
pub fn rips_h1(d: &[Vec<f32>], n: usize) -> Vec<(f32, f32)> {
    if n < 3 {
        return vec![];
    }
    // Edges sorted by length; index = filtration order.
    let mut edges: Vec<(f32, usize, usize)> = Vec::with_capacity(n * (n - 1) / 2);
    for i in 0..n {
        for j in (i + 1)..n {
            edges.push((d[i][j], i, j));
        }
    }
    edges.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut eidx: HashMap<(usize, usize), usize> = HashMap::with_capacity(edges.len());
    for (k, &(_, i, j)) in edges.iter().enumerate() {
        eidx.insert((i, j), k);
    }
    let edge_len = |k: usize| edges[k].0;

    // Triangles: filtration = max of the 3 edge lengths.
    let mut tris: Vec<(f32, [usize; 3])> = Vec::new();
    for i in 0..n {
        for j in (i + 1)..n {
            for k in (j + 1)..n {
                let e0 = eidx[&(i, j)];
                let e1 = eidx[&(i, k)];
                let e2 = eidx[&(j, k)];
                let f = edge_len(e0).max(edge_len(e1)).max(edge_len(e2));
                tris.push((f, [e0, e1, e2]));
            }
        }
    }
    tris.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Reduction: column = sorted edge indices; pivot = max index.
    let mut low: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut bars = Vec::new();
    for (filt, e) in &tris {
        let mut col: Vec<usize> = e.to_vec();
        col.sort_unstable();
        while let Some(&piv) = col.last() {
            if let Some(other) = low.get(&piv) {
                col = sym_diff(&col, other);
            } else {
                break;
            }
        }
        if let Some(&piv) = col.last() {
            low.insert(piv, col.clone());
            let birth = edge_len(piv);
            if *filt > birth {
                bars.push((birth, *filt));
            }
        }
    }
    bars
}

/// Symmetric difference of two sorted index lists (Z/2 column add).
fn sym_diff(a: &[usize], b: &[usize]) -> Vec<usize> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => {
                out.push(a[i]);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                out.push(b[j]);
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                i += 1;
                j += 1;
            }
        }
    }
    out.extend_from_slice(&a[i..]);
    out.extend_from_slice(&b[j..]);
    out
}

impl Tda {
    #[allow(clippy::needless_range_loop)] // symmetric distance-matrix writes
    fn recompute(&mut self, ctx: &FrameCtx<'_>) {
        let x = &ctx.mono;
        let tau = ((self.p.tau_ms * 0.001 * ctx.sample_rate as f32).round() as usize).max(1);
        let m = self.p.window_dims.max(3);
        let span = x.len().saturating_sub(m * tau);
        if span < N {
            return;
        }
        let stride = (span / N).max(1);
        // Embed → mean-center → sphere-normalize.
        let mut pts: Vec<Vec<f32>> = Vec::with_capacity(N);
        for p in 0..N {
            let s = p * stride;
            let mut v: Vec<f32> = (0..=m).map(|k| x[s + k * tau]).collect();
            let mean = v.iter().sum::<f32>() / v.len() as f32;
            for vi in &mut v {
                *vi -= mean;
            }
            let norm = v.iter().map(|a| a * a).sum::<f32>().sqrt().max(1e-6);
            for vi in &mut v {
                *vi /= norm;
            }
            pts.push(v);
        }
        // Distance matrix.
        let mut d = vec![vec![0.0_f32; N]; N];
        let mut maxd = 1e-6_f32;
        for i in 0..N {
            for j in (i + 1)..N {
                let dist = pts[i]
                    .iter()
                    .zip(pts[j].iter())
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum::<f32>()
                    .sqrt();
                d[i][j] = dist;
                d[j][i] = dist;
                maxd = maxd.max(dist);
            }
        }
        self.bars = rips_h1(&d, N);
        self.bars.sort_by(|a, b| {
            (b.1 - b.0)
                .partial_cmp(&(a.1 - a.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self.max_filt = maxd;
        // 2-D cloud projection (first two embedding dims, centred).
        self.cloud2d = pts
            .iter()
            .map(|v| (*v.first().unwrap_or(&0.0), *v.get(1).unwrap_or(&0.0)))
            .collect();
    }
}

impl VizModule for Tda {
    fn id(&self) -> &'static str {
        "tda"
    }
    fn label(&self) -> &'static str {
        "Topology"
    }
    fn description(&self) -> &'static str {
        "SW1PerS persistence barcode — is this a note? Periodicity as a persistent H₁ hole."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        if ctx.mono.len() < 256 || ctx.sample_rate == 0 {
            return;
        }
        if ctx.time - self.last_compute >= self.p.recompute_secs as f64 {
            self.recompute(ctx);
            self.last_compute = ctx.time;
        }

        // Periodicity score: dominant H₁ persistence, normalized by √3
        // (the diameter of the unit-sphere embedding).
        let dom = self.bars.first().map(|&(b, dth)| dth - b).unwrap_or(0.0);
        let score = (dom / 3.0_f32.sqrt()).clamp(0.0, 1.0);

        // Barcode (top area).
        let bc = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 12.0, rect.top() + 40.0),
            egui::pos2(rect.right() - 12.0, rect.top() + rect.height() * 0.6),
        );
        let maxf = self.max_filt.max(1e-3);
        let nbars = self.bars.len().min(18);
        let row_h = (bc.height() / nbars.max(1) as f32).min(14.0);
        for (i, &(b, dth)) in self.bars.iter().take(nbars).enumerate() {
            let y = bc.top() + i as f32 * row_h + row_h * 0.5;
            let x0 = bc.left() + b / maxf * bc.width();
            let x1 = bc.left() + dth / maxf * bc.width();
            let pers = (dth - b) / maxf;
            let col = if i == 0 {
                egui::Color32::from_rgb(120, 220, 255)
            } else {
                egui::Color32::from_rgba_unmultiplied(180, 180, 200, (80.0 + 175.0 * pers) as u8)
            };
            painter.line_segment(
                [egui::pos2(x0, y), egui::pos2(x1, y)],
                egui::Stroke::new((row_h * 0.5).max(2.0), col),
            );
        }

        // Point cloud (bottom-left inset).
        let inset = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 12.0, rect.bottom() - rect.height() * 0.34),
            egui::pos2(rect.left() + rect.height() * 0.34, rect.bottom() - 12.0),
        );
        painter.rect_stroke(
            inset,
            2.0,
            egui::Stroke::new(0.5, egui::Color32::from_gray(50)),
        );
        let pscale = inset.size().min_elem() * 0.45;
        let pc = inset.center();
        let mut prev: Option<egui::Pos2> = None;
        for (k, &(a, b)) in self.cloud2d.iter().enumerate() {
            let pt = egui::pos2(pc.x + a * pscale, pc.y - b * pscale);
            painter.circle_filled(pt, 1.6, egui::Color32::from_rgb(120, 220, 255));
            if let Some(pp) = prev {
                painter.line_segment(
                    [pp, pt],
                    egui::Stroke::new(0.5, egui::Color32::from_gray(70)),
                );
            }
            prev = Some(pt);
            let _ = k;
        }

        let verdict = if score > 0.6 {
            "strongly periodic (a note)"
        } else if score > 0.3 {
            "quasi-periodic"
        } else {
            "aperiodic / noisy"
        };
        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!(
                "Topology · H₁ score {score:.2} · {verdict} · {} bars",
                self.bars.len()
            ),
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(210),
        );
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        slider(
            ui,
            "Delay τ (ms)",
            "Sliding-window delay between embedding samples.",
            &mut p.tau_ms,
            0.2..=10.0,
        );
        slider(
            ui,
            "Window dims",
            "Embedding window length M (more = bigger loops).",
            &mut p.window_dims,
            6..=40,
        );
        slider(
            ui,
            "Recompute (s)",
            "Seconds between barcode recomputations (the Rips complex is the heavy step).",
            &mut p.recompute_secs,
            0.1..=1.0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::needless_range_loop)]
    fn dist_matrix(pts: &[(f32, f32)]) -> Vec<Vec<f32>> {
        let n = pts.len();
        let mut d = vec![vec![0.0; n]; n];
        for i in 0..n {
            for j in (i + 1)..n {
                let dd = ((pts[i].0 - pts[j].0).powi(2) + (pts[i].1 - pts[j].1).powi(2)).sqrt();
                d[i][j] = dd;
                d[j][i] = dd;
            }
        }
        d
    }

    #[test]
    fn circle_has_one_dominant_h1_bar() {
        // Points evenly on a unit circle → one long-lived H₁ class.
        let n = 16;
        let pts: Vec<(f32, f32)> = (0..n)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / n as f32;
                (a.cos(), a.sin())
            })
            .collect();
        let bars = rips_h1(&dist_matrix(&pts), n);
        let max_pers = bars.iter().map(|&(b, d)| d - b).fold(0.0_f32, f32::max);
        // The circle's hole persists across a wide filtration range.
        assert!(
            max_pers > 1.0,
            "expected a long H1 bar for a circle; got {max_pers}"
        );
    }

    #[test]
    fn collapsed_points_have_no_persistent_h1() {
        // A tight blob → no significant cycle.
        let n = 16;
        let pts: Vec<(f32, f32)> = (0..n).map(|i| (0.001 * i as f32, 0.0)).collect();
        let bars = rips_h1(&dist_matrix(&pts), n);
        let max_pers = bars.iter().map(|&(b, d)| d - b).fold(0.0_f32, f32::max);
        assert!(
            max_pers < 0.5,
            "a line should have no big H1 hole; got {max_pers}"
        );
    }
}
