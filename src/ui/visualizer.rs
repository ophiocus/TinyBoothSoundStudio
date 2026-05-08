//! Full-window audio-reactive visualizer (v0.4.11).
//!
//! Toggled via the `🌀` icon in the menu bar. When active, takes over
//! the central panel and renders one of four mathematically-grounded
//! modes driven by the master-bus sample tap (`PlayerState.output_viz`):
//!
//!   1. **Lissajous goniometer with phosphor trails** — XY plot of the
//!      most recent stereo samples (L on X, R on Y), drawn with an
//!      alpha gradient so newer samples glow and older ones fade.
//!      Reveals stereo image geometry: mono → vertical line, anti-
//!      phase → horizontal at 45°, full stereo → organic figure-8s.
//!
//!   2. **Spectral mandala** — radial FFT. Each bin gets a petal whose
//!      length is the bin's magnitude and whose hue tracks frequency
//!      (warm low → cool high). Mirrored across the X axis for the
//!      mandala-symmetry feel. Tonal balance becomes literally
//!      glanceable.
//!
//!   3. **Lorenz attractor (audio-modulated)** — integrate the Lorenz
//!      ODE with σ / ρ / β tugged in real time by spectral centroid /
//!      bass-band energy / treble-band energy. The strange attractor
//!      "breathes" with the music — chaos with structure.
//!
//!   4. **Chladni cymatics pattern** — superposition of sin·sin
//!      eigenmodes on a square plate, weighted by FFT bins. Renders
//!      the actual mathematical eigenmodes Ernst Chladni discovered in
//!      1787 by drawing a violin bow across a sand-dusted plate.
//!
//! All four run on egui's 2D painter — no GPU shaders, no texture
//! uploads, no extra dependencies beyond what's already in the tree.
//! Sample tap is read once per render via a brief Mutex lock; FFT
//! reuses the existing `analysis::spectrum` helper.

use crate::app::TinyBoothApp;
use eframe::egui;
use std::f32::consts::TAU;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VizMode {
    Lissajous,
    Mandala,
    Lorenz,
    Chladni,
}

impl VizMode {
    fn label(self) -> &'static str {
        match self {
            Self::Lissajous => "Lissajous",
            Self::Mandala => "Mandala",
            Self::Lorenz => "Lorenz",
            Self::Chladni => "Chladni",
        }
    }
    fn all() -> &'static [Self] {
        &[Self::Lissajous, Self::Mandala, Self::Lorenz, Self::Chladni]
    }
}

/// Persistent state for the visualizer — mostly the integrator state
/// for the Lorenz mode and a handful of cached FFT scratch buffers.
/// Lives on `TinyBoothApp` so it survives mode switches and close /
/// reopen of the canvas.
pub struct VisualizerState {
    pub mode: VizMode,
    /// Lorenz attractor state (x, y, z) — integrated continuously while
    /// the canvas is open in Lorenz mode. Reset to a small offset from
    /// origin on first use.
    lorenz_state: (f32, f32, f32),
    /// Trail of recent Lorenz points for drawing. Capped at
    /// LORENZ_TRAIL_LEN.
    lorenz_trail: Vec<(f32, f32, f32)>,
    /// Phase accumulator for Chladni modulation (small drift to
    /// prevent visual lock-in when the input is steady-state).
    chladni_phase: f32,
}

impl Default for VisualizerState {
    fn default() -> Self {
        Self {
            mode: VizMode::Lissajous,
            lorenz_state: (0.1, 0.0, 0.0),
            lorenz_trail: Vec::with_capacity(LORENZ_TRAIL_LEN),
            chladni_phase: 0.0,
        }
    }
}

const LORENZ_TRAIL_LEN: usize = 2000;
/// Steps integrated per render frame. Higher = more attractor advance
/// per frame = faster orbital motion. 12 looks lively at 30 fps.
const LORENZ_STEPS_PER_FRAME: usize = 12;
/// Lorenz integration time-step. 0.005 keeps RK4 stable across the
/// audio-modulated parameter range.
const LORENZ_DT: f32 = 0.005;

/// Render the full-window visualizer. Called from `app::update` when
/// `show_visualizer` is true. Caller has already filled in the central
/// panel area; we paint over it.
pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    // ── Mode switcher row ─────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.heading("🌀  Visualizer");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("✖  Close").clicked() {
                app.show_visualizer = false;
            }
            ui.add_space(8.0);
            for &m in VizMode::all() {
                if ui
                    .selectable_label(app.visualizer.mode == m, m.label())
                    .clicked()
                {
                    app.visualizer.mode = m;
                }
            }
        });
    });
    ui.separator();

    // ── Acquire audio data ────────────────────────────────────────
    // Snapshot the master-bus sample tap in one shot. If no player
    // exists (no project loaded, no audio device, etc.) fall back to
    // an empty slice — every mode handles "no samples" gracefully.
    let samples: Vec<(f32, f32)> = if let Some(player) = app.player.as_ref() {
        player.state.output_viz.lock().iter().copied().collect()
    } else {
        Vec::new()
    };

    // ── Allocate canvas ───────────────────────────────────────────
    let avail = ui.available_size();
    let canvas_size = egui::vec2(avail.x.max(200.0), avail.y.max(200.0));
    let (rect, _resp) = ui.allocate_exact_size(canvas_size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(8, 8, 12));

    if samples.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "no audio yet — hit ▶ on the Mix tab to feed the visualizer",
            egui::FontId::proportional(14.0),
            egui::Color32::from_gray(120),
        );
        // Still keep the canvas repainting so the moment audio arrives
        // we render it.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
        return;
    }

    // Run the chosen mode.
    match app.visualizer.mode {
        VizMode::Lissajous => draw_lissajous(&painter, rect, &samples),
        VizMode::Mandala => draw_mandala(&painter, rect, &samples),
        VizMode::Lorenz => draw_lorenz(&painter, rect, &samples, &mut app.visualizer),
        VizMode::Chladni => draw_chladni(&painter, rect, &samples, &mut app.visualizer),
    }

    // Repaint at ~30 fps — the visualizer is a continuous animation.
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
}

// ───────────────────── 1. Lissajous goniometer ─────────────────────

fn draw_lissajous(painter: &egui::Painter, rect: egui::Rect, samples: &[(f32, f32)]) {
    let centre = rect.center();
    let scale = rect.size().min_elem() * 0.45;

    // Subsample to keep the polyline draw cheap. 512 points is more
    // than enough resolution for the eye at the scales we render.
    const SUBSAMPLE_TARGET: usize = 512;
    let stride = (samples.len() / SUBSAMPLE_TARGET).max(1);
    let n = samples.len() / stride;

    // Phosphor gradient: oldest → faint, newest → bright. egui
    // doesn't have per-segment-vertex colours so we draw segments
    // individually. With ~512 points the cost is trivial.
    let phosphor_base = egui::Color32::from_rgb(120, 230, 160);
    for i in 1..n {
        let (l1, r1) = samples[(i - 1) * stride];
        let (l2, r2) = samples[i * stride];
        let p1 = egui::pos2(centre.x + l1 * scale, centre.y - r1 * scale);
        let p2 = egui::pos2(centre.x + l2 * scale, centre.y - r2 * scale);
        // Alpha ramps from 30 (oldest) to 255 (newest).
        let alpha = 30 + (225 * i as u32 / n.max(1) as u32) as u8;
        let col = egui::Color32::from_rgba_unmultiplied(
            phosphor_base.r(),
            phosphor_base.g(),
            phosphor_base.b(),
            alpha,
        );
        painter.line_segment([p1, p2], egui::Stroke::new(1.5, col));
    }

    // Crosshair guides — visual reference for "perfect mono / anti-
    // phase" angles. Faint so they don't fight the trail.
    let g = egui::Color32::from_gray(40);
    painter.line_segment(
        [
            egui::pos2(centre.x - scale, centre.y),
            egui::pos2(centre.x + scale, centre.y),
        ],
        egui::Stroke::new(1.0, g),
    );
    painter.line_segment(
        [
            egui::pos2(centre.x, centre.y - scale),
            egui::pos2(centre.x, centre.y + scale),
        ],
        egui::Stroke::new(1.0, g),
    );
    painter.text(
        egui::pos2(centre.x + scale - 16.0, centre.y - scale + 12.0),
        egui::Align2::RIGHT_TOP,
        "Lissajous · L↔R phase",
        egui::FontId::monospace(11.0),
        egui::Color32::from_gray(110),
    );
}

// ───────────────────── 2. Spectral mandala ─────────────────────

fn draw_mandala(painter: &egui::Painter, rect: egui::Rect, samples: &[(f32, f32)]) {
    let centre = rect.center();
    let max_radius = rect.size().min_elem() * 0.45;
    let inner = max_radius * 0.18;

    // Mono-mix the most recent window for a clean FFT.
    let mono: Vec<f32> = samples.iter().map(|(l, r)| 0.5 * (l + r)).collect();
    let spectrum = crate::analysis::spectrum(&mono);
    if spectrum.is_empty() {
        return;
    }

    // Drop the lowest bin (DC) and the very top (Nyquist hash).
    let usable = &spectrum[1..(spectrum.len() - 1).max(1)];
    let n = usable.len().min(256);
    if n == 0 {
        return;
    }

    let bin_arc = TAU / n as f32; // half a circle gets mirrored below
    for (i, &mag) in usable.iter().take(n).enumerate() {
        // Two angles — one above the X-axis, one below — for mandala
        // mirror symmetry.
        let angle_top = -TAU * 0.25 + bin_arc * i as f32 * 0.5;
        let angle_bot = -TAU * 0.25 - bin_arc * i as f32 * 0.5;
        let length = mag.clamp(0.0, 1.0) * (max_radius - inner);
        // Hue: low bins warm (red/orange), high bins cool (cyan/violet).
        let t = i as f32 / n as f32;
        let col = hsv_to_rgb(0.95 - t * 0.85, 0.85, 0.95);
        let col = egui::Color32::from_rgba_unmultiplied(col.0, col.1, col.2, 220);
        let stroke = egui::Stroke::new(2.0, col);
        for &angle in &[angle_top, angle_bot] {
            let pa = egui::pos2(
                centre.x + angle.cos() * inner,
                centre.y + angle.sin() * inner,
            );
            let pb = egui::pos2(
                centre.x + angle.cos() * (inner + length),
                centre.y + angle.sin() * (inner + length),
            );
            painter.line_segment([pa, pb], stroke);
        }
    }

    // Inner ring + label.
    painter.circle_stroke(
        centre,
        inner,
        egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
    );
    painter.text(
        rect.left_top() + egui::vec2(12.0, 12.0),
        egui::Align2::LEFT_TOP,
        "Mandala · radial FFT",
        egui::FontId::monospace(11.0),
        egui::Color32::from_gray(110),
    );
}

// ───────────────────── 3. Lorenz attractor (audio-modulated) ─────────────────────

fn draw_lorenz(
    painter: &egui::Painter,
    rect: egui::Rect,
    samples: &[(f32, f32)],
    state: &mut VisualizerState,
) {
    // Audio-side scalars driving the ODE parameters.
    //   • mean RMS → overall energy → scales β
    //   • spectral centroid → scales σ
    //   • peak / variance → scales ρ
    let rms = {
        let s: f32 = samples.iter().map(|(l, r)| 0.5 * (l * l + r * r)).sum();
        (s / samples.len().max(1) as f32).sqrt()
    };
    let centroid = spectral_centroid(samples);

    // Map audio scalars onto the canonical Lorenz region. Defaults
    // (10, 28, 8/3) are the chaotic regime; we breathe ~±15% around
    // them so the attractor stays in the same topological zone but
    // the orbit shape evolves.
    let sigma = 10.0 + centroid.clamp(0.0, 1.0) * 6.0;
    let rho = 26.0 + rms.clamp(0.0, 0.4) * 12.0;
    let beta = 8.0 / 3.0 + rms.clamp(0.0, 0.4) * 0.6;

    // Integrate. Simple RK4 on the Lorenz system for stability across
    // the parameter range we explore.
    for _ in 0..LORENZ_STEPS_PER_FRAME {
        state.lorenz_state = rk4_lorenz(state.lorenz_state, sigma, rho, beta, LORENZ_DT);
        if state.lorenz_trail.len() >= LORENZ_TRAIL_LEN {
            state.lorenz_trail.remove(0);
        }
        state.lorenz_trail.push(state.lorenz_state);
    }

    // Project (x, z) → screen, with z flipped because screen Y goes
    // down. Auto-fit the trail's extent so the viewer always sees the
    // attractor centred regardless of parameter drift.
    let (mut min_x, mut max_x, mut min_z, mut max_z) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for &(x, _y, z) in &state.lorenz_trail {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_z = min_z.min(z);
        max_z = max_z.max(z);
    }
    let span_x = (max_x - min_x).max(1e-3);
    let span_z = (max_z - min_z).max(1e-3);
    let pad = 0.92;
    let scale = (rect.width() / span_x).min(rect.height() / span_z) * pad;
    let project = |x: f32, z: f32| -> egui::Pos2 {
        egui::pos2(
            rect.center().x + (x - 0.5 * (min_x + max_x)) * scale,
            rect.center().y - (z - 0.5 * (min_z + max_z)) * scale,
        )
    };

    let n = state.lorenz_trail.len();
    for i in 1..n {
        let (x1, _, z1) = state.lorenz_trail[i - 1];
        let (x2, _, z2) = state.lorenz_trail[i];
        // Trail colour: hue rotates with i (deep violet → magenta →
        // amber → cyan), alpha grows toward the head.
        let t = i as f32 / n as f32;
        let col = hsv_to_rgb(0.7 - t * 0.7, 0.6, 0.85 + 0.15 * t);
        let alpha = 50 + (205.0 * t) as u8;
        let stroke = egui::Stroke::new(
            1.4,
            egui::Color32::from_rgba_unmultiplied(col.0, col.1, col.2, alpha),
        );
        painter.line_segment([project(x1, z1), project(x2, z2)], stroke);
    }

    painter.text(
        rect.left_top() + egui::vec2(12.0, 12.0),
        egui::Align2::LEFT_TOP,
        format!(
            "Lorenz · σ={sigma:.1}  ρ={rho:.1}  β={beta:.2}    \
             driven by ▷ centroid={centroid:.2}  rms={rms:.3}"
        ),
        egui::FontId::monospace(11.0),
        egui::Color32::from_gray(120),
    );
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

// ───────────────────── 4. Chladni cymatics pattern ─────────────────────

fn draw_chladni(
    painter: &egui::Painter,
    rect: egui::Rect,
    samples: &[(f32, f32)],
    state: &mut VisualizerState,
) {
    // Spectrum drives the eigenmode amplitudes. We pick a small set of
    // (m, n) modes — the lowest dozen are visually distinct and
    // computationally cheap to evaluate at every grid cell.
    let mono: Vec<f32> = samples.iter().map(|(l, r)| 0.5 * (l + r)).collect();
    let spectrum = crate::analysis::spectrum(&mono);
    if spectrum.is_empty() {
        return;
    }

    // Selected modes — Chladni's classic figures, the ones that draw
    // the recognisable cross / star / lattice patterns. Each (m, n)
    // pair gets its amplitude from a different spectral band so bass
    // drives slow patterns and treble drives fine lattices.
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

    // Build amplitudes from spectrum. Map mode-index to a bin range.
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

    // Slow phase drift so steady-state input still produces motion.
    state.chladni_phase += 0.02;
    let phase = state.chladni_phase;

    // Coarse grid keeps painter calls bounded. 64×64 = 4096 cells per
    // frame — egui handles that comfortably as small rect_filleds.
    const GRID: usize = 64;
    let cell_w = rect.width() / GRID as f32;
    let cell_h = rect.height() / GRID as f32;

    for gy in 0..GRID {
        for gx in 0..GRID {
            let x = gx as f32 / (GRID - 1) as f32;
            let y = gy as f32 / (GRID - 1) as f32;
            let mut psi = 0.0_f32;
            for ((m, n), &amp) in MODES.iter().zip(amplitudes.iter()) {
                let mf = *m as f32;
                let nf = *n as f32;
                let mode =
                    (std::f32::consts::PI * mf * x).sin() * (std::f32::consts::PI * nf * y).sin();
                let mode_swap =
                    (std::f32::consts::PI * nf * x).sin() * (std::f32::consts::PI * mf * y).sin();
                psi += amp * (mode - mode_swap) * (phase * (mf + nf) * 0.13).cos();
            }
            // Map |ψ| → brightness; sign → hue swing.
            let mag = psi.abs().clamp(0.0, 2.0) * 0.5;
            let hue_t = if psi >= 0.0 { 0.55 } else { 0.05 };
            let (r, g, b) = hsv_to_rgb(hue_t, 0.85, mag.clamp(0.0, 1.0));
            // Skip nearly-black cells — fewer painter calls, same
            // visible result against the dark canvas.
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
        "Chladni · 10-mode eigenfunction superposition (bands → amplitudes)",
        egui::FontId::monospace(11.0),
        egui::Color32::from_gray(180),
    );
}

// ───────────────────── helpers ─────────────────────

/// Spectral centroid normalised to [0, 1] across the spectrum.
fn spectral_centroid(samples: &[(f32, f32)]) -> f32 {
    let mono: Vec<f32> = samples.iter().map(|(l, r)| 0.5 * (l + r)).collect();
    let spectrum = crate::analysis::spectrum(&mono);
    if spectrum.is_empty() {
        return 0.0;
    }
    let mut weighted = 0.0;
    let mut total = 0.0;
    for (i, &m) in spectrum.iter().enumerate() {
        weighted += i as f32 * m;
        total += m;
    }
    if total < 1e-6 {
        0.0
    } else {
        (weighted / total) / spectrum.len() as f32
    }
}

/// HSV (in [0,1]³) → 8-bit RGB. Hand-rolled because we only need
/// scalar conversion and don't want to pull a colour crate.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = (h.fract() + 1.0).fract() * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (rp, gp, bp) = if h < 1.0 {
        (c, x, 0.0)
    } else if h < 2.0 {
        (x, c, 0.0)
    } else if h < 3.0 {
        (0.0, c, x)
    } else if h < 4.0 {
        (0.0, x, c)
    } else if h < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let m = v - c;
    (
        ((rp + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((gp + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((bp + m) * 255.0).clamp(0.0, 255.0) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rk4_lorenz_is_stable_on_default_params() {
        // 10 000 RK4 steps from a small offset should stay bounded
        // (the Lorenz attractor's region is roughly |x| < 25, etc.).
        let mut s = (0.1, 0.0, 0.0);
        for _ in 0..10_000 {
            s = rk4_lorenz(s, 10.0, 28.0, 8.0 / 3.0, 0.005);
        }
        assert!(s.0.abs() < 50.0 && s.1.abs() < 50.0 && s.2.abs() < 60.0);
    }

    #[test]
    fn hsv_red_renders_red() {
        let (r, g, b) = hsv_to_rgb(0.0, 1.0, 1.0);
        assert_eq!((r, g, b), (255, 0, 0));
    }

    #[test]
    fn hsv_cyan_renders_cyan() {
        let (r, g, b) = hsv_to_rgb(0.5, 1.0, 1.0);
        assert_eq!((r, g, b), (0, 255, 255));
    }

    #[test]
    fn hsv_zero_value_is_black_regardless_of_hue() {
        for h in [0.0, 0.25, 0.5, 0.75] {
            assert_eq!(hsv_to_rgb(h, 1.0, 0.0), (0, 0, 0));
        }
    }
}
