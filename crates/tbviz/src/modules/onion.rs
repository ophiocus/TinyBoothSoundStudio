//! Onion Skin — multi-timescale trajectory through (spectral_centroid,
//! RMS) feature space. Layers a bright note-scale recent trail, a
//! phrase-scale ghost, and a session-scale residency watermark. The
//! first mode designed against the "memoryless visualisation is
//! sterile" critique.

use crate::{hsv_to_rgb, slider, FrameCtx, VizModule};
use eframe::egui;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct OnionSkinParams {
    pub recent_trail_len: usize,
    pub ghost_seconds: f32,
    pub watermark_grid: usize,
    pub trail_hue: f32,
    pub show_future: bool,
    pub future_length: usize,
    pub smoothing: f32,
    pub show_axes: bool,
}
impl Default for OnionSkinParams {
    fn default() -> Self {
        Self {
            recent_trail_len: 256,
            ghost_seconds: 30.0,
            watermark_grid: 64,
            trail_hue: 0.55,
            show_future: false,
            future_length: 16,
            smoothing: 0.5,
            show_axes: true,
        }
    }
}

pub struct OnionSkin {
    p: OnionSkinParams,
    recent: VecDeque<(f32, f32, f64)>,
    ghost: VecDeque<(f32, f32)>,
    watermark: Vec<f32>,
    smoothed: (f32, f32),
    last_ghost_time: f64,
    total_seconds: f32,
}
impl Default for OnionSkin {
    fn default() -> Self {
        Self {
            p: OnionSkinParams::default(),
            recent: VecDeque::with_capacity(512),
            ghost: VecDeque::with_capacity(512),
            watermark: Vec::new(),
            smoothed: (0.5, 0.0),
            last_ghost_time: 0.0,
            total_seconds: 0.0,
        }
    }
}

impl VizModule for OnionSkin {
    fn id(&self) -> &'static str {
        "onion_skin"
    }
    fn label(&self) -> &'static str {
        "Onion Skin"
    }
    fn description(&self) -> &'static str {
        "Multi-timescale trajectory through (centroid, loudness) space. Layers note + phrase + session memory."
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>) {
        let p = self.p.clone();
        let now = ctx.time;

        // ── Compute current feature point ──────────────────────────
        let rms = ctx.rms;
        let centroid = ctx.centroid.clamp(0.0, 1.0);
        // Heavy EMA reduces note-level jitter so we read TRAJECTORY,
        // not flicker. The user-facing 'smoothing' is the OLD weight,
        // so α = 1 - smoothing.
        let alpha = 1.0 - p.smoothing.clamp(0.0, 0.95);
        self.smoothed.0 = (1.0 - alpha) * self.smoothed.0 + alpha * centroid;
        self.smoothed.1 = (1.0 - alpha) * self.smoothed.1 + alpha * rms;
        let (cx, cy) = self.smoothed;

        // ── Update temporal layers ─────────────────────────────────
        self.total_seconds += 0.033;

        self.recent.push_back((cx, cy, now));
        while self.recent.len() > p.recent_trail_len {
            self.recent.pop_front();
        }

        if now - self.last_ghost_time > 0.25 {
            self.ghost.push_back((cx, cy));
            self.last_ghost_time = now;
            let max_ghost = (p.ghost_seconds * 4.0) as usize;
            while self.ghost.len() > max_ghost {
                self.ghost.pop_front();
            }
        }

        let wg = p.watermark_grid.max(8);
        if self.watermark.len() != wg * wg {
            self.watermark = vec![0.0; wg * wg];
        }
        let wcx = (cx.clamp(0.0, 1.0) * (wg - 1) as f32) as usize;
        // RMS gets log-warped: the perceptually-relevant range is ~0..0.4,
        // but we want fine resolution near zero where most music lives.
        let wcy_norm = (cy.clamp(0.0, 0.5) / 0.5).powf(0.5);
        let wcy = (wcy_norm * (wg - 1) as f32) as usize;
        let widx = wcy.min(wg - 1) * wg + wcx.min(wg - 1);
        if let Some(c) = self.watermark.get_mut(widx) {
            *c += 0.033;
        }

        // ── Project feature space → screen ─────────────────────────
        let pad = 36.0;
        let plot_rect = rect.shrink(pad);
        let project = |fx: f32, fy: f32| -> egui::Pos2 {
            let x = plot_rect.left() + fx.clamp(0.0, 1.0) * plot_rect.width();
            let y = plot_rect.bottom() - (fy.clamp(0.0, 0.5) / 0.5) * plot_rect.height();
            egui::pos2(x, y)
        };

        // ── Layer 1: watermark heatmap (faintest) ──────────────────
        let max_w = self.watermark.iter().cloned().fold(0.0_f32, f32::max);
        if max_w > 1e-6 {
            let cell_w = plot_rect.width() / wg as f32;
            let cell_h = plot_rect.height() / wg as f32;
            for gy in 0..wg {
                for gx in 0..wg {
                    let v = self.watermark[gy * wg + gx] / max_w;
                    if v < 0.02 {
                        continue;
                    }
                    let intensity = (v * 0.45).clamp(0.0, 1.0);
                    let col = egui::Color32::from_rgba_unmultiplied(
                        (intensity * 80.0) as u8,
                        (intensity * 30.0) as u8,
                        (intensity * 100.0) as u8,
                        (intensity * 255.0) as u8,
                    );
                    let r = egui::Rect::from_min_size(
                        egui::pos2(
                            plot_rect.left() + gx as f32 * cell_w,
                            plot_rect.bottom() - (gy + 1) as f32 * cell_h,
                        ),
                        egui::vec2(cell_w + 0.5, cell_h + 0.5),
                    );
                    painter.rect_filled(r, 0.0, col);
                }
            }
        }

        // ── Layer 2: ghost trail ───────────────────────────────────
        let ghost_n = self.ghost.len();
        if ghost_n > 1 {
            for i in 1..ghost_n {
                let (a_x, a_y) = self.ghost[i - 1];
                let (b_x, b_y) = self.ghost[i];
                let t = i as f32 / ghost_n as f32;
                let alpha = 30 + (60.0 * t) as u8;
                let col = egui::Color32::from_rgba_unmultiplied(140, 160, 200, alpha);
                painter.line_segment(
                    [project(a_x, a_y), project(b_x, b_y)],
                    egui::Stroke::new(1.0, col),
                );
            }
        }

        // ── Layer 3: bright recent trail ───────────────────────────
        let recent: Vec<(f32, f32)> = self.recent.iter().map(|&(x, y, _)| (x, y)).collect();
        let n = recent.len();
        if n > 1 {
            for i in 1..n {
                let (a_x, a_y) = recent[i - 1];
                let (b_x, b_y) = recent[i];
                let t = i as f32 / n as f32;
                let hue = (p.trail_hue + t * 0.15).rem_euclid(1.0);
                let (r, g, b) = hsv_to_rgb(hue, 0.7, 0.9);
                let alpha = 40 + (215.0 * t) as u8;
                let col = egui::Color32::from_rgba_unmultiplied(r, g, b, alpha);
                painter.line_segment(
                    [project(a_x, a_y), project(b_x, b_y)],
                    egui::Stroke::new(1.5 + 1.5 * t, col),
                );
            }
            let (hx, hy) = recent[n - 1];
            let head = project(hx, hy);
            painter.circle_filled(head, 4.0, egui::Color32::WHITE);
            painter.circle_stroke(
                head,
                8.0,
                egui::Stroke::new(
                    1.5,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 90),
                ),
            );
        }

        // ── Layer 4 (optional): anticipated future ─────────────────
        if p.show_future && n > 16 {
            let recent_dir: (f32, f32) = {
                let mut dx = 0.0;
                let mut dy = 0.0;
                for i in (n - 16)..n {
                    let (a_x, a_y) = recent[i - 1];
                    let (b_x, b_y) = recent[i];
                    dx += b_x - a_x;
                    dy += b_y - a_y;
                }
                (dx / 16.0, dy / 16.0)
            };
            let (mut fx, mut fy) = recent[n - 1];
            for i in 0..p.future_length {
                let alpha = 100 - (i as i32 * 6).clamp(0, 100);
                let col =
                    egui::Color32::from_rgba_unmultiplied(255, 220, 80, alpha.clamp(0, 255) as u8);
                let nfx = fx + recent_dir.0;
                let nfy = fy + recent_dir.1;
                painter.line_segment(
                    [project(fx, fy), project(nfx, nfy)],
                    egui::Stroke::new(1.0, col),
                );
                fx = nfx;
                fy = nfy;
                if !(0.0..=1.0).contains(&fx) {
                    break;
                }
            }
        }

        // ── Axes ───────────────────────────────────────────────────
        if p.show_axes {
            let axis = egui::Color32::from_gray(80);
            painter.line_segment(
                [
                    egui::pos2(plot_rect.left(), plot_rect.bottom()),
                    egui::pos2(plot_rect.right(), plot_rect.bottom()),
                ],
                egui::Stroke::new(1.0, axis),
            );
            painter.line_segment(
                [
                    egui::pos2(plot_rect.left(), plot_rect.top()),
                    egui::pos2(plot_rect.left(), plot_rect.bottom()),
                ],
                egui::Stroke::new(1.0, axis),
            );
            painter.text(
                egui::pos2(plot_rect.left() + 4.0, plot_rect.bottom() - 4.0),
                egui::Align2::LEFT_BOTTOM,
                "soft · dark",
                egui::FontId::monospace(10.0),
                egui::Color32::from_gray(140),
            );
            painter.text(
                egui::pos2(plot_rect.right() - 4.0, plot_rect.bottom() - 4.0),
                egui::Align2::RIGHT_BOTTOM,
                "soft · bright",
                egui::FontId::monospace(10.0),
                egui::Color32::from_gray(140),
            );
            painter.text(
                egui::pos2(plot_rect.left() + 4.0, plot_rect.top() + 4.0),
                egui::Align2::LEFT_TOP,
                "loud · dark",
                egui::FontId::monospace(10.0),
                egui::Color32::from_gray(140),
            );
            painter.text(
                egui::pos2(plot_rect.right() - 4.0, plot_rect.top() + 4.0),
                egui::Align2::RIGHT_TOP,
                "loud · bright",
                egui::FontId::monospace(10.0),
                egui::Color32::from_gray(140),
            );
        }
        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!(
                "Onion Skin · trail {} · ghost {:.0}s · session {:.0}s",
                recent.len(),
                p.ghost_seconds,
                self.total_seconds
            ),
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(160),
        );
    }

    fn config_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.p;
        ui.label(
            egui::RichText::new(
                "Multi-timescale trajectory: bright recent trail, fading ghost \
                 of the medium past, time-weighted watermark of the whole session.",
            )
            .italics()
            .weak(),
        );
        ui.add_space(6.0);
        slider(
            ui,
            "Recent trail",
            "Number of points in the bright 'now' trail. Each point is one render \
             frame, so at 30 fps this is roughly the seconds × 30.",
            &mut p.recent_trail_len,
            32..=512,
        );
        slider(
            ui,
            "Ghost seconds",
            "How many seconds of medium-past data to retain as the dim ghost trail. \
             Phrase / section timescale.",
            &mut p.ghost_seconds,
            5.0..=120.0,
        );
        slider(
            ui,
            "Watermark grid",
            "Resolution of the session-wide residency heatmap. 64×64 is the default. \
             Higher = finer, but quadratic in compute and memory.",
            &mut p.watermark_grid,
            32..=128,
        );
        slider(
            ui,
            "Smoothing",
            "EMA on the (centroid, RMS) point before plotting. Higher = smoother \
             trajectory, less note-level jitter. 0.5 is balanced.",
            &mut p.smoothing,
            0.0..=0.95,
        );
        slider(
            ui,
            "Trail hue",
            "Base hue for the recent-trail gradient.",
            &mut p.trail_hue,
            0.0..=1.0,
        );
        ui.checkbox(&mut p.show_axes, "Show axis labels")
            .on_hover_text(
                "Label the X and Y axes with 'soft / loud' and 'dark / bright' \
                 so the listener can orient at a glance.",
            );
        ui.checkbox(&mut p.show_future, "Anticipated future")
            .on_hover_text(
                "EXPERIMENTAL — extrapolate where the trajectory is heading via \
                 linear extension of the recent direction. Off by default; the \
                 prediction breaks down across phrase boundaries.",
            );
        if p.show_future {
            slider(
                ui,
                "Future segments",
                "How many dashed segments to project forward. More = longer \
                 prediction horizon (less reliable).",
                &mut p.future_length,
                4..=64,
            );
        }
    }
}
