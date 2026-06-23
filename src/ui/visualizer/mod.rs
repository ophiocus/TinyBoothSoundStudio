//! Full-window audio-reactive visualizer — **modular** architecture
//! (refactored v0.4.53 from the v0.4.11 monolith).
//!
//! Every visualization is a self-contained [`VizModule`]: it owns its
//! own parameters, persistent state, draw routine, and config UI. The
//! shell here is mode-agnostic — it builds one shared [`FrameCtx`] per
//! frame (the realized "TinyOutput contract": stereo samples + the
//! common analyses every engine wants), then hands it to whichever
//! module is active.
//!
//! ## Adding a new module
//!
//! 1. Create `modules/<name>.rs` with a struct that holds the module's
//!    params + state and `impl VizModule for <Name>`.
//! 2. `pub mod <name>;` + `pub use <name>::<Name>;` in `modules/mod.rs`.
//! 3. Add one line to [`default_modules`].
//!
//! That's it — the tab bar, config panel, canvas dispatch, and the
//! cross-cutting coherence HUD all pick it up automatically. The 18
//! research engines in `docs/research/sound-visualization-engines.md`
//! are each a future `VizModule` over this same `FrameCtx`.

pub mod modules;

use crate::app::TinyBoothApp;
use eframe::egui;
use std::collections::VecDeque;

// ── Live cross-band coherence HUD (Phase 4, v0.4.38) ────────────────
//
// A cross-cutting overlay (not a mode): each frame it bins the master-
// bus spectrum into log-spaced bands, keeps a rolling history of per-
// band energy, and reports the mean pairwise Pearson correlation — the
// same AI-audio fingerprint metric the telemetry analyzer computes per
// track, estimated live. Tiers reuse the shared `telemetry::COH_*`
// thresholds so the verdict matches the Mix-tab pill.

/// Number of log-spaced bands the live-coherence HUD tracks.
const COH_VIZ_BANDS: usize = 6;
/// Upper edges (Hz) of the first `COH_VIZ_BANDS - 1` bands; the final
/// band runs from the last edge up to Nyquist.
const COH_VIZ_EDGES: [f32; COH_VIZ_BANDS - 1] = [150.0, 400.0, 1000.0, 2500.0, 6000.0];
/// Frames of band-energy history (~3 s at the 30 fps canvas cadence).
const COH_VIZ_HISTORY: usize = 90;
/// Minimum history before we trust the estimate enough to display.
const COH_VIZ_MIN_FRAMES: usize = 16;
/// EMA smoothing applied to the displayed value so it doesn't jitter.
const COH_VIZ_SMOOTH: f32 = 0.1;

/// Per-frame shared analysis handed to the active module — the realized
/// "TinyOutput contract". Everything here is derived once from the
/// master-bus sample tap so modules don't each recompute the FFT.
pub struct FrameCtx<'a> {
    /// Raw interleaved-as-tuples stereo tap `(L, R)`.
    pub samples: &'a [(f32, f32)],
    pub sample_rate: u32,
    /// `ui.input(|i| i.time)` — wall-clock seconds, monotonic.
    pub time: f64,
    /// Frame delta in seconds (`stable_dt`) for frame-rate-correct work.
    /// Part of the contract surface — consumed by future engines (the
    /// reassignment / adaptive-display modules), not the ported five.
    #[allow(dead_code)]
    pub dt: f32,
    /// Mono sum `0.5·(L+R)`. Contract surface for engines that want the
    /// raw mono signal rather than the precomputed spectrum.
    #[allow(dead_code)]
    pub mono: Vec<f32>,
    /// Magnitude spectrum of `mono` (0..Nyquist), via `analysis::spectrum`.
    pub spectrum: Vec<f32>,
    /// RMS over the tap window.
    pub rms: f32,
    /// Normalized spectral centroid in `[0, 1]`.
    pub centroid: f32,
}

/// One pluggable visualization. Owns its params + persistent state.
pub trait VizModule {
    /// Stable identifier (snake_case) — for tests + future
    /// remember-last-active-mode persistence.
    #[allow(dead_code)]
    fn id(&self) -> &'static str;
    /// Short tab label.
    fn label(&self) -> &'static str;
    /// One-line description shown as a tab tooltip + config header.
    fn description(&self) -> &'static str;
    /// Render into `rect` using the shared per-frame context.
    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>);
    /// Render this module's parameter controls into the config panel.
    fn config_ui(&mut self, ui: &mut egui::Ui);
}

/// The built-in module set, in tab order. Add new engines here.
pub fn default_modules() -> Vec<Box<dyn VizModule>> {
    vec![
        Box::new(modules::Spectrogram::default()),
        Box::new(modules::Reassigned::default()),
        Box::new(modules::SpectrumBars::default()),
        Box::new(modules::Chroma::default()),
        Box::new(modules::Similarity::default()),
        Box::new(modules::Vectorscope::default()),
        Box::new(modules::Lissajous::default()),
        Box::new(modules::PhasePortrait::default()),
        Box::new(modules::Recurrence::default()),
        Box::new(modules::Som::default()),
        Box::new(modules::Particles::default()),
        Box::new(modules::Health::default()),
        Box::new(modules::Mandala::default()),
        Box::new(modules::Lorenz::default()),
        Box::new(modules::Chladni::default()),
        Box::new(modules::OnionSkin::default()),
    ]
}

/// Persistent visualizer state. Lives on `TinyBoothApp`.
pub struct VisualizerState {
    /// The registered modules, in tab order.
    pub modules: Vec<Box<dyn VizModule>>,
    /// Index into `modules` of the active mode.
    pub active: usize,
    pub config_open: bool,

    // Cross-cutting coherence overlay (applies over any module).
    pub coherence_hud: bool,
    coh_history: VecDeque<[f32; COH_VIZ_BANDS]>,
    coh_live: f32,
    coh_primed: bool,
}

impl Default for VisualizerState {
    fn default() -> Self {
        Self {
            modules: default_modules(),
            active: 0,
            config_open: true,
            coherence_hud: true,
            coh_history: VecDeque::with_capacity(COH_VIZ_HISTORY),
            coh_live: 0.0,
            coh_primed: false,
        }
    }
}

/// Render the visualizer. Called from `app::update` when
/// `show_visualizer` is true.
pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    // ── Top bar: mode switcher + close ─────────────────────────────
    ui.horizontal(|ui| {
        ui.heading("🌀  Visualizer");
        let toggle_label = if app.visualizer.config_open {
            "◀ Hide config"
        } else {
            "▶ Show config"
        };
        if ui
            .button(toggle_label)
            .on_hover_text("Show or hide the per-mode parameter panel.")
            .clicked()
        {
            app.visualizer.config_open = !app.visualizer.config_open;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("✖  Close").clicked() {
                app.show_visualizer = false;
            }
            ui.add_space(8.0);
            for i in 0..app.visualizer.modules.len() {
                let selected = app.visualizer.active == i;
                let label = app.visualizer.modules[i].label();
                let desc = app.visualizer.modules[i].description();
                if ui
                    .selectable_label(selected, label)
                    .on_hover_text(desc)
                    .clicked()
                {
                    app.visualizer.active = i;
                }
            }
        });
    });
    ui.separator();

    // ── Left config panel ──────────────────────────────────────────
    if app.visualizer.config_open {
        egui::SidePanel::left("viz_config_panel")
            .resizable(true)
            .default_width(280.0)
            .min_width(220.0)
            .show_inside(ui, |ui| {
                config_panel(app, ui);
            });
    }

    // ── Main canvas (right of the optional sidebar) ────────────────
    egui::CentralPanel::default()
        .frame(egui::Frame::none())
        .show_inside(ui, |ui| {
            canvas(app, ui);
        });

    // 30 fps repaint cadence for the canvas.
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
}

// ───────────────────── canvas (right panel) ─────────────────────

fn canvas(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    let samples: Vec<(f32, f32)> = if let Some(player) = app.player.as_ref() {
        player.state.output_viz.lock().iter().copied().collect()
    } else {
        Vec::new()
    };

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
        return;
    }

    // Build the shared per-frame context once (the TinyOutput contract).
    let sample_rate = app
        .player
        .as_ref()
        .map(|p| p.state.sample_rate)
        .unwrap_or(0);
    let (time, dt) = ui.ctx().input(|i| (i.time, i.stable_dt));
    let mono: Vec<f32> = samples.iter().map(|(l, r)| 0.5 * (l + r)).collect();
    let spectrum = crate::analysis::spectrum(&mono);
    let rms = {
        let s: f32 = samples.iter().map(|(l, r)| 0.5 * (l * l + r * r)).sum();
        (s / samples.len().max(1) as f32).sqrt()
    };
    let centroid = centroid_from_spectrum(&spectrum);
    let ctx = FrameCtx {
        samples: &samples,
        sample_rate,
        time,
        dt,
        mono,
        spectrum,
        rms,
        centroid,
    };

    // Live coherence estimate updates before the mode draws so the HUD
    // (rendered last, on top) reflects the current frame.
    update_live_coherence(&mut app.visualizer, &ctx);

    let active = app
        .visualizer
        .active
        .min(app.visualizer.modules.len().saturating_sub(1));
    if let Some(module) = app.visualizer.modules.get_mut(active) {
        module.draw(&painter, rect, &ctx);
    }

    if app.visualizer.coherence_hud && app.visualizer.coh_primed {
        draw_coherence_hud(&painter, rect, app.visualizer.coh_live);
    }
}

/// Normalized spectral centroid in `[0, 1]` from a magnitude spectrum.
/// Matches the legacy `spectral_centroid` exactly (weighted bin index /
/// total, divided by bin count).
fn centroid_from_spectrum(spectrum: &[f32]) -> f32 {
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

/// Update the rolling live cross-band coherence estimate from the shared
/// spectrum. Mirrors the analyzer's pairwise-Pearson approach over a
/// short rolling window instead of the full STFT.
fn update_live_coherence(state: &mut VisualizerState, ctx: &FrameCtx<'_>) {
    if ctx.sample_rate == 0 || ctx.samples.len() < 256 {
        return;
    }
    let spec = &ctx.spectrum;
    if spec.is_empty() {
        return;
    }
    // `spectrum` returns the first half of the FFT (0..Nyquist), so the
    // Hz of bin `i` is `i * Nyquist / spec.len()`.
    let hz_per_bin = (ctx.sample_rate as f32 * 0.5) / spec.len() as f32;
    let mut energy = [0.0f32; COH_VIZ_BANDS];
    for (i, mag) in spec.iter().enumerate() {
        let hz = i as f32 * hz_per_bin;
        let band = COH_VIZ_EDGES
            .iter()
            .position(|&e| hz < e)
            .unwrap_or(COH_VIZ_BANDS - 1);
        energy[band] += *mag;
    }
    state.coh_history.push_back(energy);
    while state.coh_history.len() > COH_VIZ_HISTORY {
        state.coh_history.pop_front();
    }
    let n = state.coh_history.len();
    if n < COH_VIZ_MIN_FRAMES {
        return;
    }

    let mut means = [0.0f32; COH_VIZ_BANDS];
    for row in &state.coh_history {
        for b in 0..COH_VIZ_BANDS {
            means[b] += row[b];
        }
    }
    for m in &mut means {
        *m /= n as f32;
    }
    let mut var = [0.0f32; COH_VIZ_BANDS];
    let mut cov = [[0.0f32; COH_VIZ_BANDS]; COH_VIZ_BANDS];
    for row in &state.coh_history {
        let mut d = [0.0f32; COH_VIZ_BANDS];
        for b in 0..COH_VIZ_BANDS {
            d[b] = row[b] - means[b];
            var[b] += d[b] * d[b];
        }
        for a in 0..COH_VIZ_BANDS {
            for b in (a + 1)..COH_VIZ_BANDS {
                cov[a][b] += d[a] * d[b];
            }
        }
    }
    let mut sum_corr = 0.0f32;
    let mut pairs = 0u32;
    for a in 0..COH_VIZ_BANDS {
        for b in (a + 1)..COH_VIZ_BANDS {
            let denom = (var[a] * var[b]).sqrt();
            if denom > 1e-9 {
                sum_corr += cov[a][b] / denom;
                pairs += 1;
            }
        }
    }
    if pairs == 0 {
        return;
    }
    let inst = (sum_corr / pairs as f32).clamp(0.0, 1.0);
    if state.coh_primed {
        state.coh_live += COH_VIZ_SMOOTH * (inst - state.coh_live);
    } else {
        state.coh_live = inst;
        state.coh_primed = true;
    }
}

/// Draw the live coherence badge in the top-right corner of the canvas.
fn draw_coherence_hud(painter: &egui::Painter, rect: egui::Rect, coh: f32) {
    use crate::telemetry::{COH_AI_MAX, COH_CLEAN_MIN};
    let (label, color) = if coh < COH_AI_MAX {
        (
            format!("Band Coh {coh:.2}  AI"),
            egui::Color32::from_rgb(230, 150, 230),
        )
    } else if coh >= COH_CLEAN_MIN {
        (
            format!("Band Coh {coh:.2}  ≈"),
            egui::Color32::from_rgb(150, 230, 190),
        )
    } else {
        (format!("Band Coh {coh:.2}"), egui::Color32::from_gray(200))
    };
    let galley = painter.layout_no_wrap(label, egui::FontId::monospace(13.0), color);
    let pad = egui::vec2(8.0, 4.0);
    let size = galley.size() + pad * 2.0;
    let top_right = egui::pos2(rect.max.x - 12.0, rect.min.y + 12.0);
    let chip = egui::Rect::from_min_size(egui::pos2(top_right.x - size.x, top_right.y), size);
    painter.rect_filled(
        chip,
        4.0,
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 170),
    );
    painter.galley(chip.min + pad, galley, color);
}

// ───────────────────── config panel ─────────────────────

fn config_panel(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        let active = app
            .visualizer
            .active
            .min(app.visualizer.modules.len().saturating_sub(1));
        ui.heading("Parameters");
        let desc = app
            .visualizer
            .modules
            .get(active)
            .map(|m| m.description())
            .unwrap_or("");
        ui.label(egui::RichText::new(desc).italics().weak());
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);

        // Global overlay (applies to every mode).
        ui.checkbox(&mut app.visualizer.coherence_hud, "Live coherence HUD")
            .on_hover_text(
                "Overlay a live cross-band coherence readout — the AI-audio \
                 fingerprint metric — in the top-right of the canvas. Low \
                 (pink AI) = bands wobble independently; high (green ≈) = \
                 bands move together like a real recording. Same thresholds \
                 as the Mix-tab pill.",
            );
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        if let Some(module) = app.visualizer.modules.get_mut(active) {
            module.config_ui(ui);
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(6.0);
        ui.collapsing("About this mode", |ui| {
            ui.label(
                egui::RichText::new(
                    "Hover any control for a one-line explanation. \
                     Defaults reproduce the original hard-coded behaviour. \
                     For the design philosophy behind the modes — and the \
                     research backlog of new engines — see \
                     `docs/research/sound-visualization-engines.md`.",
                )
                .italics()
                .weak(),
            );
        });
    });
}

// ───────────────────── shared helpers (used by modules) ─────────────

/// Labelled slider row used by every module's `config_ui`.
pub(crate) fn slider<T: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    label: &str,
    help: &str,
    value: &mut T,
    range: std::ops::RangeInclusive<T>,
) {
    ui.horizontal(|ui| {
        ui.label(label).on_hover_text(help);
        ui.add(egui::Slider::new(value, range));
    });
}

/// HSV → RGB (`h,s,v` in `[0,1]`). Shared across modules.
pub(crate) fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
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

/// Perceptually-ordered **magma** colormap (`t` in `[0,1]` → RGB).
/// Monotonic in luminance — the honest choice for magnitude (see the
/// colour-science engine in `docs/research/sound-visualization-engines.md`).
/// Linear interpolation over matplotlib's magma control points; great on
/// the dark canvas.
pub(crate) fn magma(t: f32) -> (u8, u8, u8) {
    const STOPS: [(f32, f32, f32); 11] = [
        (0.0, 0.0, 0.015),
        (0.06, 0.023, 0.18),
        (0.20, 0.018, 0.40),
        (0.34, 0.075, 0.50),
        (0.47, 0.14, 0.51),
        (0.58, 0.20, 0.49),
        (0.71, 0.27, 0.44),
        (0.85, 0.36, 0.36),
        (0.95, 0.52, 0.30),
        (0.98, 0.71, 0.40),
        (0.99, 0.92, 0.66),
    ];
    let t = t.clamp(0.0, 1.0) * (STOPS.len() - 1) as f32;
    let i = (t as usize).min(STOPS.len() - 2);
    let f = t - i as f32;
    let a = STOPS[i];
    let b = STOPS[i + 1];
    let lerp = |x: f32, y: f32| ((x + (y - x) * f) * 255.0).clamp(0.0, 255.0) as u8;
    (lerp(a.0, b.0), lerp(a.1, b.1), lerp(a.2, b.2))
}

/// Upload `image` to a reused texture handle and stretch it over `rect`.
/// Modules that render a 2-D field (spectrogram, similarity matrix,
/// recurrence plot, …) build a `ColorImage` once per frame and blit it
/// rather than emitting tens of thousands of `rect_filled` calls.
pub(crate) fn blit_image(
    painter: &egui::Painter,
    rect: egui::Rect,
    tex: &mut Option<egui::TextureHandle>,
    name: &str,
    image: egui::ColorImage,
    options: egui::TextureOptions,
) {
    let handle = match tex {
        Some(h) => {
            h.set(image, options);
            h.clone()
        }
        None => {
            let h = painter.ctx().load_texture(name, image, options);
            *tex = Some(h.clone());
            h
        }
    };
    painter.image(
        handle.id(),
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

/// Hz at magnitude-spectrum bin `i`, given the spectrum length and the
/// source sample rate. `analysis::spectrum` returns `fft_size/2` bins
/// (DC..Nyquist), so `fft_size = 2·len` and bin width = `sr / fft_size`.
pub(crate) fn bin_hz(i: usize, spectrum_len: usize, sample_rate: u32) -> f32 {
    if spectrum_len == 0 {
        return 0.0;
    }
    let fft_size = (spectrum_len * 2) as f32;
    i as f32 * sample_rate as f32 / fft_size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_red_renders_red() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), (255, 0, 0));
    }

    #[test]
    fn hsv_cyan_renders_cyan() {
        assert_eq!(hsv_to_rgb(0.5, 1.0, 1.0), (0, 255, 255));
    }

    #[test]
    fn hsv_zero_value_is_black_regardless_of_hue() {
        for h in [0.0, 0.25, 0.5, 0.75] {
            assert_eq!(hsv_to_rgb(h, 1.0, 0.0), (0, 0, 0));
        }
    }

    #[test]
    fn default_modules_are_unique() {
        let mods = default_modules();
        assert!(mods.len() >= 11);
        let mut ids: Vec<&str> = mods.iter().map(|m| m.id()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "module ids must be unique");
    }

    #[test]
    fn centroid_of_empty_is_zero() {
        assert_eq!(centroid_from_spectrum(&[]), 0.0);
        assert_eq!(centroid_from_spectrum(&[0.0, 0.0, 0.0]), 0.0);
    }
}
