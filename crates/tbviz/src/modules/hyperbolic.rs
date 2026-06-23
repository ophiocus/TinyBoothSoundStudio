//! Hyperbolic form browser (#18) — the song's structure is hierarchical,
//! and hierarchies fit negatively-curved space. We agglomeratively
//! cluster the chroma history into a tree, lay it out on the Poincaré
//! disk (depth → hyperbolic radius, so deeper branches get exponentially
//! more room), and let you Möbius-pan/zoom for infinite focus + context.

use crate::{bin_hz, hsv_to_rgb, slider, FrameCtx, VizModule};
use eframe::egui;
use std::collections::VecDeque;

const MAX_N: usize = 48;

#[derive(Debug, Clone)]
pub struct HyperbolicParams {
    pub hop_secs: f32,
    pub spread: f32,
}
impl Default for HyperbolicParams {
    fn default() -> Self {
        Self {
            hop_secs: 0.5,
            spread: 0.62,
        }
    }
}

struct Node {
    children: Vec<usize>,
    leaf: Option<usize>, // index into feats for time-colouring
    pos: (f32, f32),
    depth: usize,
}

pub struct Hyperbolic {
    p: HyperbolicParams,
    feats: VecDeque<[f32; 12]>,
    last_capture: f64,
    pan: (f32, f32),
    zoom: f32,
}
impl Default for Hyperbolic {
    fn default() -> Self {
        Self {
            p: HyperbolicParams::default(),
            feats: VecDeque::with_capacity(MAX_N),
            last_capture: 0.0,
            pan: (0.0, 0.0),
            zoom: 1.0,
        }
    }
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

fn feat_dist(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// Complex Möbius translation w = (z − p)/(1 − p̄·z), all in the disk.
fn mobius(z: (f32, f32), p: (f32, f32)) -> (f32, f32) {
    let (zx, zy) = z;
    let (px, py) = p;
    // numerator = z - p
    let nx = zx - px;
    let ny = zy - py;
    // denom = 1 - conj(p)*z = 1 - (px - i py)(zx + i zy)
    let dx = 1.0 - (px * zx + py * zy);
    let dy = -(px * zy - py * zx);
    let dd = (dx * dx + dy * dy).max(1e-6);
    ((nx * dx + ny * dy) / dd, (ny * dx - nx * dy) / dd)
}

impl Hyperbolic {
    /// Build an agglomerative (average-linkage) tree over the current
    /// feature frames; returns the node arena and the root index.
    fn build_tree(&self) -> (Vec<Node>, usize) {
        let feats: Vec<[f32; 12]> = self.feats.iter().copied().collect();
        let n = feats.len();
        let mut nodes: Vec<Node> = (0..n)
            .map(|i| Node {
                children: vec![],
                leaf: Some(i),
                pos: (0.0, 0.0),
                depth: 0,
            })
            .collect();
        // Active clusters: (node_idx, centroid, count).
        let mut active: Vec<(usize, [f32; 12], usize)> = (0..n).map(|i| (i, feats[i], 1)).collect();
        while active.len() > 1 {
            // Find closest pair.
            let mut bi = 0;
            let mut bj = 1;
            let mut best = f32::MAX;
            for a in 0..active.len() {
                for b in (a + 1)..active.len() {
                    let d = feat_dist(&active[a].1, &active[b].1);
                    if d < best {
                        best = d;
                        bi = a;
                        bj = b;
                    }
                }
            }
            let (ni, ci, cnt_i) = active[bi];
            let (nj, cj, cnt_j) = active[bj];
            let tot = (cnt_i + cnt_j) as f32;
            let mut merged = [0.0_f32; 12];
            for k in 0..12 {
                merged[k] = (ci[k] * cnt_i as f32 + cj[k] * cnt_j as f32) / tot;
            }
            let new_idx = nodes.len();
            nodes.push(Node {
                children: vec![ni, nj],
                leaf: None,
                pos: (0.0, 0.0),
                depth: 0,
            });
            // Remove bj then bi (bj > bi).
            active.remove(bj);
            active.remove(bi);
            active.push((new_idx, merged, cnt_i + cnt_j));
        }
        let root = active[0].0;
        (nodes, root)
    }

    /// Radial layout: assign each node an angle (DFS over a [lo,hi]
    /// range) and a hyperbolic radius from its depth.
    fn layout(&self, nodes: &mut [Node], root: usize) {
        // Depths via BFS.
        let mut stack = vec![(root, 0usize)];
        while let Some((idx, d)) = stack.pop() {
            nodes[idx].depth = d;
            let kids = nodes[idx].children.clone();
            for k in kids {
                stack.push((k, d + 1));
            }
        }
        // Angular ranges via recursion (iterative stack).
        let mut work = vec![(root, 0.0_f32, std::f32::consts::TAU)];
        let max_depth = nodes.iter().map(|n| n.depth).max().unwrap_or(1).max(1) as f32;
        while let Some((idx, lo, hi)) = work.pop() {
            let ang = 0.5 * (lo + hi);
            let depth = nodes[idx].depth as f32;
            // Hyperbolic radius: tanh grows toward the boundary with depth.
            let r = (depth / max_depth * self.p.spread * 2.5).tanh();
            nodes[idx].pos = (r * ang.cos(), r * ang.sin());
            let kids = nodes[idx].children.clone();
            let k = kids.len();
            if k > 0 {
                let step = (hi - lo) / k as f32;
                for (ci, &child) in kids.iter().enumerate() {
                    let clo = lo + step * ci as f32;
                    work.push((child, clo, clo + step));
                }
            }
        }
    }
}

impl VizModule for Hyperbolic {
    fn id(&self) -> &'static str {
        "hyperbolic"
    }
    fn label(&self) -> &'static str {
        "Hyperbolic"
    }
    fn description(&self) -> &'static str {
        "Poincaré-disk tree of the chroma history. Drag to Möbius-pan; scroll to zoom. Focus + context."
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

        // Pointer interaction: drag to pan, scroll to zoom.
        let (hover, down, delta, scroll) = painter.ctx().input(|i| {
            (
                i.pointer.hover_pos(),
                i.pointer.primary_down(),
                i.pointer.delta(),
                i.raw_scroll_delta.y,
            )
        });
        let disk_r = rect.size().min_elem() * 0.46;
        let centre = rect.center();
        if let Some(h) = hover {
            if rect.contains(h) {
                if down {
                    self.pan.0 = (self.pan.0 - delta.x / disk_r * 0.5).clamp(-0.9, 0.9);
                    self.pan.1 = (self.pan.1 - delta.y / disk_r * 0.5).clamp(-0.9, 0.9);
                }
                if scroll.abs() > 0.0 {
                    self.zoom = (self.zoom * (1.0 + scroll * 0.001)).clamp(0.4, 4.0);
                }
            }
        }

        // Disk boundary.
        painter.circle_stroke(
            centre,
            disk_r,
            egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
        );

        let n = self.feats.len();
        if n < 3 {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "building hyperbolic tree… (needs a few seconds of audio)",
                egui::FontId::monospace(12.0),
                egui::Color32::from_gray(130),
            );
            return;
        }

        let (mut nodes, root) = self.build_tree();
        self.layout(&mut nodes, root);

        let to_screen = |z: (f32, f32)| -> egui::Pos2 {
            let m = mobius(z, self.pan);
            egui::pos2(
                centre.x + m.0 * disk_r * self.zoom,
                centre.y + m.1 * disk_r * self.zoom,
            )
        };

        // Edges.
        for (idx, node) in nodes.iter().enumerate() {
            let a = to_screen(node.pos);
            for &c in &node.children {
                let b = to_screen(nodes[c].pos);
                painter.line_segment([a, b], egui::Stroke::new(1.0, egui::Color32::from_gray(90)));
            }
            let _ = idx;
        }
        // Nodes: leaves coloured by time (hue along the trail).
        for node in &nodes {
            let pt = to_screen(node.pos);
            if let Some(li) = node.leaf {
                let t = li as f32 / n as f32;
                let (r, g, b) = hsv_to_rgb(0.55 + t * 0.4, 0.7, 0.95);
                painter.circle_filled(pt, 3.5, egui::Color32::from_rgb(r, g, b));
            } else {
                painter.circle_filled(pt, 1.6, egui::Color32::from_gray(150));
            }
        }

        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!(
                "Hyperbolic · {n} frames · zoom {:.1}× · drag to pan",
                self.zoom
            ),
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(190),
        );
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        slider(
            ui,
            "Hop (s)",
            "Seconds between captured chroma frames.",
            &mut p.hop_secs,
            0.1..=2.0,
        );
        slider(
            ui,
            "Spread",
            "How fast depth pushes nodes toward the disk boundary.",
            &mut p.spread,
            0.3..=1.0,
        );
        if ui.button("Reset view").clicked() {
            self.pan = (0.0, 0.0);
            self.zoom = 1.0;
        }
        if ui.button("Clear history").clicked() {
            self.feats.clear();
        }
    }
}
