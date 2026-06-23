//! `tbviz` — the TinyBooth visualizer engine, extracted as a standalone
//! library so **TinyBooth Sound Studio** and **TinyAmp** share one
//! modular `VizModule` set with zero drift.
//!
//! Every visualization is a self-contained [`VizModule`]: it owns its
//! params, persistent state, `draw`, and `config_ui`. The host builds a
//! per-frame [`FrameCtx`] from a stereo sample tap (the "TinyOutput
//! contract") and calls [`show`]; the engine is otherwise host-agnostic
//! — it knows nothing about projects, players, or files.
//!
//! ## Adding a new module
//! 1. `modules/<name>.rs` with a struct + `impl VizModule`.
//! 2. `pub mod <name>; pub use <name>::<Name>;` in `modules/mod.rs`.
//! 3. One line in [`default_modules`].

pub mod modules;

use eframe::egui;
use rustfft::{num_complex::Complex, FftPlanner};
use std::collections::VecDeque;

// ── Live cross-band coherence HUD ───────────────────────────────────
// Thresholds mirror TinyBooth's `telemetry::COH_*` so the visualizer's
// live verdict matches the Mix-tab pill. Duplicated here (rather than
// depending on the app) to keep `tbviz` host-agnostic.
const COH_AI_MAX: f32 = 0.35;
const COH_CLEAN_MIN: f32 = 0.55;
const COH_VIZ_BANDS: usize = 6;
const COH_VIZ_EDGES: [f32; COH_VIZ_BANDS - 1] = [150.0, 400.0, 1000.0, 2500.0, 6000.0];
const COH_VIZ_HISTORY: usize = 90;
const COH_VIZ_MIN_FRAMES: usize = 16;
const COH_VIZ_SMOOTH: f32 = 0.1;

/// Hann-windowed log-magnitude spectrum (0..1), `fft_size/2` bins,
/// DC dropped at consumption. Self-contained copy of the app's
/// `analysis::spectrum` so `tbviz` owns its own DSP.
pub fn spectrum(samples: &[f32]) -> Vec<f32> {
    if samples.len() < 64 {
        return Vec::new();
    }
    let fft_size = samples.len().next_power_of_two().clamp(512, 4096);
    let take = fft_size.min(samples.len());
    let start = samples.len() - take;
    let mut buf: Vec<Complex<f32>> = (0..fft_size)
        .map(|i| {
            let x = if i < take { samples[start + i] } else { 0.0 };
            let w = if i < take {
                let t = i as f32 / (take.max(2) - 1) as f32;
                0.5 - 0.5 * (std::f32::consts::TAU * t).cos()
            } else {
                0.0
            };
            Complex { re: x * w, im: 0.0 }
        })
        .collect();
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(fft_size);
    fft.process(&mut buf);
    let n = fft_size / 2;
    let scale = 4.0 / fft_size as f32;
    let mut out = Vec::with_capacity(n);
    for bin in &buf[..n] {
        let mag = (bin.re * bin.re + bin.im * bin.im).sqrt() * scale;
        let db = 20.0 * (mag + 1e-9).log10();
        out.push(((db + 90.0) / 100.0).clamp(0.0, 1.0));
    }
    out
}

/// Per-frame shared analysis handed to the active module — the realized
/// "TinyOutput contract". Built once from the stereo tap so modules
/// don't each recompute the FFT.
pub struct FrameCtx<'a> {
    pub samples: &'a [(f32, f32)],
    pub sample_rate: u32,
    pub time: f64,
    #[allow(dead_code)]
    pub dt: f32,
    #[allow(dead_code)]
    pub mono: Vec<f32>,
    pub spectrum: Vec<f32>,
    pub rms: f32,
    pub centroid: f32,
}

impl<'a> FrameCtx<'a> {
    /// Build the context from a raw stereo tap + the frame clock.
    pub fn build(samples: &'a [(f32, f32)], sample_rate: u32, time: f64, dt: f32) -> Self {
        let mono: Vec<f32> = samples.iter().map(|(l, r)| 0.5 * (l + r)).collect();
        let spectrum = spectrum(&mono);
        let rms = {
            let s: f32 = samples.iter().map(|(l, r)| 0.5 * (l * l + r * r)).sum();
            (s / samples.len().max(1) as f32).sqrt()
        };
        let centroid = centroid_from_spectrum(&spectrum);
        Self {
            samples,
            sample_rate,
            time,
            dt,
            mono,
            spectrum,
            rms,
            centroid,
        }
    }
}

/// One pluggable visualization. Owns its params + persistent state.
pub trait VizModule {
    #[allow(dead_code)]
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, ctx: &FrameCtx<'_>);
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
        Box::new(modules::Tda::default()),
        Box::new(modules::OpticalFlow::default()),
        Box::new(modules::Saliency::default()),
        Box::new(modules::Hyperbolic::default()),
        Box::new(modules::Particles::default()),
        Box::new(modules::Health::default()),
        Box::new(modules::Mandala::default()),
        Box::new(modules::Lorenz::default()),
        Box::new(modules::Chladni::default()),
        Box::new(modules::OnionSkin::default()),
    ]
}

/// Persistent visualizer state. The host owns one of these.
pub struct VisualizerState {
    pub modules: Vec<Box<dyn VizModule>>,
    pub active: usize,
    pub config_open: bool,
    /// Set true when the in-engine Close button is pressed; the host
    /// reads + resets it to hide the visualizer.
    pub close_requested: bool,
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
            close_requested: false,
            coherence_hud: true,
            coh_history: VecDeque::with_capacity(COH_VIZ_HISTORY),
            coh_live: 0.0,
            coh_primed: false,
        }
    }
}

/// Render the full visualizer (top bar + optional config panel + canvas)
/// into the current `ui`, driven by the supplied stereo tap. The host
/// provides `samples` (interleaved-as-tuples) and `sample_rate`; the
/// engine builds the `FrameCtx` itself. Sets `state.close_requested`
/// if the user clicks Close.
pub fn show(
    state: &mut VisualizerState,
    ui: &mut egui::Ui,
    samples: &[(f32, f32)],
    sample_rate: u32,
) {
    ui.horizontal(|ui| {
        ui.heading("🌀  Visualizer");
        let toggle_label = if state.config_open {
            "◀ Hide config"
        } else {
            "▶ Show config"
        };
        if ui
            .button(toggle_label)
            .on_hover_text("Show or hide the per-mode parameter panel.")
            .clicked()
        {
            state.config_open = !state.config_open;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("✖  Close").clicked() {
                state.close_requested = true;
            }
            ui.add_space(8.0);
            for i in 0..state.modules.len() {
                let selected = state.active == i;
                let label = state.modules[i].label();
                let desc = state.modules[i].description();
                if ui
                    .selectable_label(selected, label)
                    .on_hover_text(desc)
                    .clicked()
                {
                    state.active = i;
                }
            }
        });
    });
    ui.separator();

    if state.config_open {
        egui::SidePanel::left("viz_config_panel")
            .resizable(true)
            .default_width(280.0)
            .min_width(220.0)
            .show_inside(ui, |ui| {
                config_panel(state, ui);
            });
    }

    egui::CentralPanel::default()
        .frame(egui::Frame::none())
        .show_inside(ui, |ui| {
            canvas(state, ui, samples, sample_rate);
        });

    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
}

fn canvas(
    state: &mut VisualizerState,
    ui: &mut egui::Ui,
    samples: &[(f32, f32)],
    sample_rate: u32,
) {
    let avail = ui.available_size();
    let canvas_size = egui::vec2(avail.x.max(200.0), avail.y.max(200.0));
    let (rect, _resp) = ui.allocate_exact_size(canvas_size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(8, 8, 12));

    if samples.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "no audio yet — start playback to feed the visualizer",
            egui::FontId::proportional(14.0),
            egui::Color32::from_gray(120),
        );
        return;
    }

    let (time, dt) = ui.ctx().input(|i| (i.time, i.stable_dt));
    let ctx = FrameCtx::build(samples, sample_rate, time, dt);
    update_live_coherence(state, &ctx);

    let active = state.active.min(state.modules.len().saturating_sub(1));
    if let Some(module) = state.modules.get_mut(active) {
        module.draw(&painter, rect, &ctx);
    }

    if state.coherence_hud && state.coh_primed {
        draw_coherence_hud(&painter, rect, state.coh_live);
    }
}

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

fn update_live_coherence(state: &mut VisualizerState, ctx: &FrameCtx<'_>) {
    if ctx.sample_rate == 0 || ctx.samples.len() < 256 {
        return;
    }
    let spec = &ctx.spectrum;
    if spec.is_empty() {
        return;
    }
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

fn draw_coherence_hud(painter: &egui::Painter, rect: egui::Rect, coh: f32) {
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

fn config_panel(state: &mut VisualizerState, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        let active = state.active.min(state.modules.len().saturating_sub(1));
        ui.heading("Parameters");
        let desc = state
            .modules
            .get(active)
            .map(|m| m.description())
            .unwrap_or("");
        ui.label(egui::RichText::new(desc).italics().weak());
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);

        ui.checkbox(&mut state.coherence_hud, "Live coherence HUD")
            .on_hover_text(
                "Overlay a live cross-band coherence readout — the AI-audio \
                 fingerprint metric — in the top-right of the canvas.",
            );
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        if let Some(module) = state.modules.get_mut(active) {
            module.config_ui(ui);
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(6.0);
        ui.collapsing("About this mode", |ui| {
            ui.label(
                egui::RichText::new(
                    "Hover any control for a one-line explanation. For the design \
                     of each engine see docs/research/sound-visualization-engines.md.",
                )
                .italics()
                .weak(),
            );
        });
    });
}

// ── shared helpers used by modules ──────────────────────────────────

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

pub(crate) fn bin_hz(i: usize, spectrum_len: usize, sample_rate: u32) -> f32 {
    if spectrum_len == 0 {
        return 0.0;
    }
    let fft_size = (spectrum_len * 2) as f32;
    i as f32 * sample_rate as f32 / fft_size
}

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
    fn default_modules_are_unique() {
        let mods = default_modules();
        assert!(mods.len() >= 20);
        let mut ids: Vec<&str> = mods.iter().map(|m| m.id()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "module ids must be unique");
    }

    #[test]
    fn centroid_of_empty_is_zero() {
        assert_eq!(centroid_from_spectrum(&[]), 0.0);
    }
}
