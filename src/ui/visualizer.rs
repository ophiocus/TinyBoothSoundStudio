//! Full-window audio-reactive visualizer (v0.4.11+, parametrised in v0.4.12).
//!
//! Toggled via the `🌀` icon in the menu bar. When active, takes over
//! the central panel and renders one of five mathematically-grounded
//! modes driven by the master-bus sample tap (`PlayerState.output_viz`):
//!
//!   1. **Lissajous goniometer with phosphor trails** — XY plot of
//!      stereo samples (L on X, R on Y) with alpha-decay trails.
//!   2. **Spectral mandala** — radial FFT, magnitude as petal length,
//!      hue tracking frequency, mirrored across X for symmetry.
//!   3. **Lorenz attractor (audio-modulated)** — RK4 integration of
//!      the Lorenz ODE with σ/ρ/β tugged by spectral centroid + RMS.
//!   4. **Chladni cymatics pattern** — superposition of sin·sin
//!      eigenmodes weighted by FFT bands.
//!   5. **Onion Skin (multi-timescale trajectory)** — v0.4.12, the
//!      novel mode. Plots `(spectral_centroid, RMS)` as motion
//!      through 2D feature space with three layers of temporal
//!      memory (recent trail, medium ghost, session-wide watermark).
//!      The first mode that genuinely shows trajectory across
//!      multiple timescales rather than a derivative-of-NOW snapshot.
//!      See `docs/sound-vision-philosophy.md` for the full design
//!      reasoning.
//!
//! All five run on egui's 2D painter — no GPU shaders, no texture
//! uploads, no extra deps beyond what's already in the tree.
//!
//! v0.4.12 also adds a collapsible left-side **config panel** that
//! exposes every per-mode parameter as a tooltip-annotated slider /
//! checkbox, plus per-mode temporal smoothing for the modes that
//! benefit from it (Mandala, Onion Skin).

use crate::app::TinyBoothApp;
use eframe::egui;
use std::f32::consts::TAU;

// ── Live cross-band coherence HUD (Phase 4, v0.4.38) ────────────────
//
// The visualizer canvas overlays a live readout of the same AI-audio
// fingerprint metric the telemetry analyzer computes per-track at save
// (`telemetry::compute_cross_band_coherence`). Where the analyzer runs
// once over the whole STFT, the HUD estimates it continuously: each
// frame it bins the master-bus spectrum into log-spaced bands, keeps a
// rolling history of per-band energy, and reports the mean pairwise
// Pearson correlation across that history. Same number, live — so you
// can hear *and* see a stem's coherence as it plays. Tiers reuse the
// shared `telemetry::COH_*` thresholds so the verdict matches the
// Mix-tab pill and Project-Health column.

/// Number of log-spaced bands the live-coherence HUD tracks.
const COH_VIZ_BANDS: usize = 6;
/// Upper edges (Hz) of the first `COH_VIZ_BANDS - 1` bands; the final
/// band runs from the last edge up to Nyquist. Roughly octave-spaced,
/// mirroring the analyzer's band layout.
const COH_VIZ_EDGES: [f32; COH_VIZ_BANDS - 1] = [150.0, 400.0, 1000.0, 2500.0, 6000.0];
/// Frames of band-energy history (~3 s at the 30 fps canvas cadence)
/// feeding the live pairwise-correlation estimate.
const COH_VIZ_HISTORY: usize = 90;
/// Minimum history before we trust the estimate enough to display.
const COH_VIZ_MIN_FRAMES: usize = 16;
/// EMA smoothing applied to the displayed value so it doesn't jitter.
const COH_VIZ_SMOOTH: f32 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VizMode {
    Lissajous,
    Mandala,
    Lorenz,
    Chladni,
    OnionSkin,
}

impl VizMode {
    fn label(self) -> &'static str {
        match self {
            Self::Lissajous => "Lissajous",
            Self::Mandala => "Mandala",
            Self::Lorenz => "Lorenz",
            Self::Chladni => "Chladni",
            Self::OnionSkin => "Onion Skin",
        }
    }
    fn all() -> &'static [Self] {
        &[
            Self::Lissajous,
            Self::Mandala,
            Self::Lorenz,
            Self::Chladni,
            Self::OnionSkin,
        ]
    }
    fn description(self) -> &'static str {
        match self {
            Self::Lissajous => "L vs R XY plot with phosphor trails. Phase relationships at the sample timescale.",
            Self::Mandala => "Radial FFT. Frequencies arranged around the centre. Note-timescale tonal balance.",
            Self::Lorenz => "Audio-modulated Lorenz attractor. RK4 ODE integration. σ/ρ/β tugged by spectrum.",
            Self::Chladni => "10-mode eigenfunction superposition (Chladni 1787). FFT bands weight each mode.",
            Self::OnionSkin => "Multi-timescale trajectory through (centroid, loudness) space. Layers note + phrase + session memory. The first mode designed to address `docs/sound-vision-philosophy.md`'s critique of memoryless visualisation.",
        }
    }
}

/// Per-mode parameters. Every numeric value the modes use is exposed
/// here; the config panel reads / mutates these. Defaults match the
/// values the v0.4.11 hard-coded modes shipped with, so behaviour is
/// unchanged on first launch.
#[derive(Debug, Clone, Default)]
pub struct VisualizerParams {
    pub lissajous: LissajousParams,
    pub mandala: MandalaParams,
    pub lorenz: LorenzParams,
    pub chladni: ChladniParams,
    pub onion: OnionSkinParams,
}

#[derive(Debug, Clone)]
pub struct LissajousParams {
    pub subsample_target: usize,
    pub alpha_floor: u8,
    pub scale_factor: f32,
    pub stroke_width: f32,
    pub show_guides: bool,
}
impl Default for LissajousParams {
    fn default() -> Self {
        Self {
            subsample_target: 512,
            alpha_floor: 30,
            scale_factor: 0.45,
            stroke_width: 1.5,
            show_guides: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MandalaParams {
    pub bin_count: usize,
    pub inner_radius_frac: f32,
    pub hue_start: f32,
    pub hue_range: f32,
    pub saturation: f32,
    pub value: f32,
    pub stroke_width: f32,
    pub smoothing: f32,
}
impl Default for MandalaParams {
    fn default() -> Self {
        Self {
            bin_count: 256,
            inner_radius_frac: 0.18,
            hue_start: 0.95,
            hue_range: 0.85,
            saturation: 0.85,
            value: 0.95,
            stroke_width: 2.0,
            smoothing: 0.4,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LorenzParams {
    pub sigma_base: f32,
    pub rho_base: f32,
    pub beta_base: f32,
    pub centroid_drive: f32,
    pub rms_rho_drive: f32,
    pub rms_beta_drive: f32,
    pub dt: f32,
    pub steps_per_frame: usize,
    pub trail_length: usize,
    pub stroke_width: f32,
}
impl Default for LorenzParams {
    fn default() -> Self {
        Self {
            sigma_base: 10.0,
            rho_base: 26.0,
            beta_base: 8.0 / 3.0,
            centroid_drive: 6.0,
            rms_rho_drive: 12.0,
            rms_beta_drive: 0.6,
            dt: 0.005,
            steps_per_frame: 12,
            trail_length: 2000,
            stroke_width: 1.4,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChladniParams {
    pub grid_size: usize,
    pub phase_speed: f32,
    pub intensity: f32,
    pub hue_positive: f32,
    pub hue_negative: f32,
}
impl Default for ChladniParams {
    fn default() -> Self {
        Self {
            grid_size: 64,
            phase_speed: 0.02,
            intensity: 0.5,
            hue_positive: 0.55,
            hue_negative: 0.05,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OnionSkinParams {
    /// Length of the bright "now" trail in samples. ~2 s at 48k = 96000;
    /// we cap at the size of the audio tap (4096) and just take what
    /// we get.
    pub recent_trail_len: usize,
    /// How many seconds of the medium-past ghost trail to retain.
    pub ghost_seconds: f32,
    /// Watermark grid resolution. Higher = finer heatmap, slower draw.
    pub watermark_grid: usize,
    /// Hue rotation start for the recent trail (HSV in [0,1]).
    pub trail_hue: f32,
    /// Anticipated-future projection on/off + length in dashed segments.
    pub show_future: bool,
    pub future_length: usize,
    /// Smoothing factor for the (centroid, RMS) point before plotting.
    /// Reduces note-level jitter so the trajectory reads as motion
    /// rather than flicker. 0.0 = off, 1.0 = frozen.
    pub smoothing: f32,
    /// Show axis labels on/off.
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

/// Persistent visualizer state. Lives on `TinyBoothApp`.
pub struct VisualizerState {
    pub mode: VizMode,
    pub params: VisualizerParams,
    pub config_open: bool,

    // Lorenz integrator
    lorenz_state: (f32, f32, f32),
    lorenz_trail: Vec<(f32, f32, f32)>,

    // Chladni phase drift
    chladni_phase: f32,

    // Mandala temporal smoothing — exponentially-averaged spectrum
    smoothed_spectrum: Vec<f32>,

    // Onion Skin state
    /// Recent trail of (centroid, rms) samples. Updated every frame.
    onion_recent: std::collections::VecDeque<(f32, f32, f64)>,
    /// Medium-past trail, decimated.
    onion_ghost: std::collections::VecDeque<(f32, f32)>,
    /// Watermark grid: each cell counts time-weighted residency.
    onion_watermark: Vec<f32>,
    /// Smoothed (centroid, rms) point.
    onion_smoothed: (f32, f32),
    /// Last frame time so we can decimate to the ghost trail.
    onion_last_ghost_time: f64,
    /// Total time the canvas has been collecting watermark data,
    /// for normalising the colour scale.
    onion_total_seconds: f32,

    // Live coherence HUD (Phase 4, v0.4.38)
    /// Toggle for the canvas coherence overlay.
    pub coherence_hud: bool,
    /// Rolling per-band energy history feeding the live estimate.
    coh_history: std::collections::VecDeque<[f32; COH_VIZ_BANDS]>,
    /// EMA-smoothed live coherence value shown in the HUD.
    coh_live: f32,
    /// False until the EMA has been seeded with its first real reading,
    /// so the HUD doesn't flash "AI" from a cold 0.0 start.
    coh_primed: bool,
}

impl Default for VisualizerState {
    fn default() -> Self {
        Self {
            mode: VizMode::Lissajous,
            params: VisualizerParams::default(),
            config_open: true,
            lorenz_state: (0.1, 0.0, 0.0),
            lorenz_trail: Vec::new(),
            chladni_phase: 0.0,
            smoothed_spectrum: Vec::new(),
            onion_recent: std::collections::VecDeque::with_capacity(512),
            onion_ghost: std::collections::VecDeque::with_capacity(512),
            onion_watermark: Vec::new(),
            onion_smoothed: (0.5, 0.0),
            onion_last_ghost_time: 0.0,
            onion_total_seconds: 0.0,
            coherence_hud: true,
            coh_history: std::collections::VecDeque::with_capacity(COH_VIZ_HISTORY),
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
        // Config-panel toggle — collapses the left sidebar.
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
            for &m in VizMode::all() {
                if ui
                    .selectable_label(app.visualizer.mode == m, m.label())
                    .on_hover_text(m.description())
                    .clicked()
                {
                    app.visualizer.mode = m;
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

    // 30 fps repaint cadence for the canvas. The repaint fires from
    // `canvas` too; this is a belt-and-suspenders.
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

    let now = ui.ctx().input(|i| i.time);

    // Live coherence estimate updates before the mode draws so the HUD
    // (rendered last, on top) reflects the current frame.
    let sr = app
        .player
        .as_ref()
        .map(|p| p.state.sample_rate)
        .unwrap_or(0);
    update_live_coherence(&mut app.visualizer, &samples, sr);

    match app.visualizer.mode {
        VizMode::Lissajous => {
            draw_lissajous(&painter, rect, &samples, &app.visualizer.params.lissajous)
        }
        VizMode::Mandala => draw_mandala(&painter, rect, &samples, &mut app.visualizer),
        VizMode::Lorenz => draw_lorenz(&painter, rect, &samples, &mut app.visualizer),
        VizMode::Chladni => draw_chladni(&painter, rect, &samples, &mut app.visualizer),
        VizMode::OnionSkin => draw_onion_skin(&painter, rect, &samples, &mut app.visualizer, now),
    }

    if app.visualizer.coherence_hud && app.visualizer.coh_primed {
        draw_coherence_hud(&painter, rect, app.visualizer.coh_live);
    }
}

/// Update the rolling live cross-band coherence estimate from the
/// master-bus sample tap. See the `COH_VIZ_*` constants and the module
/// HUD note for the rationale; this mirrors the analyzer's pairwise-
/// Pearson approach over a short rolling window instead of the full
/// STFT.
fn update_live_coherence(state: &mut VisualizerState, samples: &[(f32, f32)], sr: u32) {
    if sr == 0 || samples.len() < 256 {
        return;
    }
    // Mono sum → spectrum.
    let mono: Vec<f32> = samples.iter().map(|(l, r)| 0.5 * (l + r)).collect();
    let spec = crate::analysis::spectrum(&mono);
    if spec.is_empty() {
        return;
    }
    // `spectrum` returns the first half of the FFT (0..Nyquist), so the
    // Hz of bin `i` is `i * Nyquist / spec.len()`.
    let hz_per_bin = (sr as f32 * 0.5) / spec.len() as f32;
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

    // Per-band means.
    let mut means = [0.0f32; COH_VIZ_BANDS];
    for row in &state.coh_history {
        for b in 0..COH_VIZ_BANDS {
            means[b] += row[b];
        }
    }
    for m in &mut means {
        *m /= n as f32;
    }
    // Variances + pairwise covariances over the window.
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
    // Mean of the 15 pairwise Pearson correlations.
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
        return; // every band flat this window — keep the last reading.
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
        (
            format!("Band Coh {coh:.2}"),
            egui::Color32::from_gray(200),
        )
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
        ui.heading("Parameters");
        ui.label(
            egui::RichText::new(app.visualizer.mode.description())
                .italics()
                .weak(),
        );
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

        match app.visualizer.mode {
            VizMode::Lissajous => lissajous_params_ui(ui, &mut app.visualizer.params.lissajous),
            VizMode::Mandala => mandala_params_ui(ui, &mut app.visualizer.params.mandala),
            VizMode::Lorenz => lorenz_params_ui(ui, &mut app.visualizer.params.lorenz),
            VizMode::Chladni => chladni_params_ui(ui, &mut app.visualizer.params.chladni),
            VizMode::OnionSkin => onion_skin_params_ui(ui, &mut app.visualizer.params.onion),
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(6.0);
        ui.collapsing("About this mode", |ui| {
            ui.label(
                egui::RichText::new(
                    "Hover any control for a one-line explanation. \
                     Defaults reproduce the v0.4.11 hard-coded behaviour. \
                     For the design philosophy behind the modes — \
                     including why most audio viz is sterile and what we're \
                     trying to fix — see `docs/sound-vision-philosophy.md`.",
                )
                .italics()
                .weak(),
            );
        });
    });
}

fn slider<T: egui::emath::Numeric>(
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

fn lissajous_params_ui(ui: &mut egui::Ui, p: &mut LissajousParams) {
    slider(
        ui,
        "Subsample",
        "Number of points sampled from the input buffer for the polyline. \
         Higher = smoother curve, more draw calls.",
        &mut p.subsample_target,
        64..=2048,
    );
    slider(
        ui,
        "Alpha floor",
        "Minimum trail alpha. Higher = older samples are more visible \
         (longer phosphor persistence). 30 is the v0.4.11 default.",
        &mut p.alpha_floor,
        0..=200,
    );
    slider(
        ui,
        "Scale",
        "Plot scale as a fraction of the canvas's shorter dimension. \
         0.45 leaves room for the crosshair guides at the edges.",
        &mut p.scale_factor,
        0.10..=0.60,
    );
    slider(
        ui,
        "Stroke width",
        "Line thickness for the trail in pixels.",
        &mut p.stroke_width,
        0.5..=4.0,
    );
    ui.checkbox(&mut p.show_guides, "Show guides")
        .on_hover_text(
            "Draw the horizontal + vertical reference lines through the centre. \
             Useful for spotting mono content (vertical) vs anti-phase (45°).",
        );
}

fn mandala_params_ui(ui: &mut egui::Ui, p: &mut MandalaParams) {
    slider(
        ui,
        "Bins",
        "Number of FFT bins drawn as petals. Higher = finer resolution, \
         more draw calls. The actual FFT runs at a fixed size; this just \
         caps how many bins we plot.",
        &mut p.bin_count,
        32..=512,
    );
    slider(
        ui,
        "Smoothing",
        "Exponential moving average on the spectrum: shown[i] = (1−α) × shown[i] + α × spec[i]. \
         0 = no smoothing (jittery, responds instantly). 1 = fully smoothed (frozen). \
         The visible jerkiness on AI-generated audio is partly diagnostic of \
         band-decorrelated micro-flicker; see docs/sound-vision-philosophy.md §V.",
        &mut p.smoothing,
        0.0..=0.95,
    );
    slider(
        ui,
        "Inner radius",
        "Inner radius as a fraction of the outer radius. Larger = more empty \
         centre, shorter petals.",
        &mut p.inner_radius_frac,
        0.05..=0.50,
    );
    slider(
        ui,
        "Hue start",
        "Hue offset (0–1) for the lowest-frequency bin. Other bins rotate \
         around the colour wheel from there.",
        &mut p.hue_start,
        0.0..=1.0,
    );
    slider(
        ui,
        "Hue range",
        "Total fraction of the colour wheel the bins span. 0.85 stops just \
         short of wrapping all the way back to red.",
        &mut p.hue_range,
        0.1..=1.0,
    );
    slider(
        ui,
        "Saturation",
        "HSV saturation for the petal hue.",
        &mut p.saturation,
        0.0..=1.0,
    );
    slider(
        ui,
        "Brightness",
        "HSV value (brightness) for the petal hue.",
        &mut p.value,
        0.0..=1.0,
    );
    slider(
        ui,
        "Stroke width",
        "Petal line thickness in pixels.",
        &mut p.stroke_width,
        0.5..=6.0,
    );
}

fn lorenz_params_ui(ui: &mut egui::Ui, p: &mut LorenzParams) {
    ui.label(egui::RichText::new("ODE base parameters").strong().small());
    slider(
        ui,
        "σ base",
        "Baseline σ in the Lorenz ODE. Default 10 is the canonical chaotic regime. \
         Spectral centroid offsets this in real time by ±centroid_drive.",
        &mut p.sigma_base,
        4.0..=20.0,
    );
    slider(
        ui,
        "ρ base",
        "Baseline ρ. 28 is canonical chaos; below ~24 the orbit collapses to fixed points.",
        &mut p.rho_base,
        20.0..=40.0,
    );
    slider(
        ui,
        "β base",
        "Baseline β. 8/3 ≈ 2.667 is canonical.",
        &mut p.beta_base,
        1.0..=5.0,
    );
    ui.add_space(6.0);
    ui.label(egui::RichText::new("Audio coupling").strong().small());
    slider(
        ui,
        "Centroid → σ",
        "How much normalised spectral centroid (0..1) shifts σ. Larger = more visible \
         response to brightness changes in the audio.",
        &mut p.centroid_drive,
        0.0..=12.0,
    );
    slider(
        ui,
        "RMS → ρ",
        "How much loudness (RMS, clamped to [0, 0.4]) shifts ρ.",
        &mut p.rms_rho_drive,
        0.0..=20.0,
    );
    slider(
        ui,
        "RMS → β",
        "How much loudness shifts β.",
        &mut p.rms_beta_drive,
        0.0..=2.0,
    );
    ui.add_space(6.0);
    ui.label(egui::RichText::new("Integration").strong().small());
    slider(
        ui,
        "dt",
        "RK4 timestep. 0.005 is stable across the parameter range we explore. \
         Larger dt = faster orbit, less stable.",
        &mut p.dt,
        0.001..=0.02,
    );
    slider(
        ui,
        "Steps / frame",
        "How many integration steps per render frame. Higher = faster orbital motion.",
        &mut p.steps_per_frame,
        1..=64,
    );
    slider(
        ui,
        "Trail length",
        "How many recent points to draw as the trail.",
        &mut p.trail_length,
        100..=8000,
    );
    slider(
        ui,
        "Stroke width",
        "Trail line thickness in pixels.",
        &mut p.stroke_width,
        0.5..=4.0,
    );
}

fn chladni_params_ui(ui: &mut egui::Ui, p: &mut ChladniParams) {
    slider(
        ui,
        "Grid size",
        "Side length of the rendering grid in cells. The field is evaluated at \
         each cell — 64 is fast and sharp; higher gets pretty but quadratic in cost.",
        &mut p.grid_size,
        16..=128,
    );
    slider(
        ui,
        "Phase speed",
        "How fast the modulation phase drifts each frame. Keeps the figure \
         animated even on steady-state input.",
        &mut p.phase_speed,
        0.0..=0.10,
    );
    slider(
        ui,
        "Intensity",
        "Overall brightness multiplier. Higher = more saturated patterns; \
         too high and the figure clips to white.",
        &mut p.intensity,
        0.1..=2.0,
    );
    slider(
        ui,
        "Hue (positive)",
        "Hue used where ψ > 0 (the bright lobes). 0.55 is teal.",
        &mut p.hue_positive,
        0.0..=1.0,
    );
    slider(
        ui,
        "Hue (negative)",
        "Hue used where ψ < 0 (the dark lobes — Chladni's sand lines).",
        &mut p.hue_negative,
        0.0..=1.0,
    );
}

fn onion_skin_params_ui(ui: &mut egui::Ui, p: &mut OnionSkinParams) {
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

// ───────────────────── 1. Lissajous ─────────────────────

fn draw_lissajous(
    painter: &egui::Painter,
    rect: egui::Rect,
    samples: &[(f32, f32)],
    p: &LissajousParams,
) {
    let centre = rect.center();
    let scale = rect.size().min_elem() * p.scale_factor;

    let stride = (samples.len() / p.subsample_target).max(1);
    let n = samples.len() / stride;
    let phosphor_base = egui::Color32::from_rgb(120, 230, 160);
    for i in 1..n {
        let (l1, r1) = samples[(i - 1) * stride];
        let (l2, r2) = samples[i * stride];
        let p1 = egui::pos2(centre.x + l1 * scale, centre.y - r1 * scale);
        let p2 = egui::pos2(centre.x + l2 * scale, centre.y - r2 * scale);
        let alpha_range = 255 - p.alpha_floor as i32;
        let alpha = p.alpha_floor as i32 + alpha_range * i as i32 / n.max(1) as i32;
        let col = egui::Color32::from_rgba_unmultiplied(
            phosphor_base.r(),
            phosphor_base.g(),
            phosphor_base.b(),
            alpha.clamp(0, 255) as u8,
        );
        painter.line_segment([p1, p2], egui::Stroke::new(p.stroke_width, col));
    }

    if p.show_guides {
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
    }
    painter.text(
        rect.left_top() + egui::vec2(12.0, 12.0),
        egui::Align2::LEFT_TOP,
        "Lissajous · L↔R phase",
        egui::FontId::monospace(11.0),
        egui::Color32::from_gray(110),
    );
}

// ───────────────────── 2. Spectral mandala ─────────────────────

fn draw_mandala(
    painter: &egui::Painter,
    rect: egui::Rect,
    samples: &[(f32, f32)],
    state: &mut VisualizerState,
) {
    let p = state.params.mandala.clone();
    let centre = rect.center();
    let max_radius = rect.size().min_elem() * 0.45;
    let inner = max_radius * p.inner_radius_frac;

    let mono: Vec<f32> = samples.iter().map(|(l, r)| 0.5 * (l + r)).collect();
    let raw_spectrum = crate::analysis::spectrum(&mono);
    if raw_spectrum.is_empty() {
        return;
    }

    // Temporal smoothing: shown[i] = (1-α)·shown[i] + α·raw[i]
    // The user-set smoothing parameter is the WEIGHT on the OLD value,
    // so 0 = full responsiveness, 1 = frozen. Convert to α = 1 - smoothing.
    if state.smoothed_spectrum.len() != raw_spectrum.len() {
        state.smoothed_spectrum = raw_spectrum.clone();
    }
    let alpha = 1.0 - p.smoothing.clamp(0.0, 0.99);
    for (s, &r) in state.smoothed_spectrum.iter_mut().zip(raw_spectrum.iter()) {
        *s = (1.0 - alpha) * *s + alpha * r;
    }

    let usable = &state.smoothed_spectrum[1..(state.smoothed_spectrum.len() - 1).max(1)];
    let n = usable.len().min(p.bin_count);
    if n == 0 {
        return;
    }

    let bin_arc = TAU / n as f32;
    for (i, &mag) in usable.iter().take(n).enumerate() {
        let angle_top = -TAU * 0.25 + bin_arc * i as f32 * 0.5;
        let angle_bot = -TAU * 0.25 - bin_arc * i as f32 * 0.5;
        let length = mag.clamp(0.0, 1.0) * (max_radius - inner);
        let t = i as f32 / n as f32;
        let hue = (p.hue_start - t * p.hue_range).rem_euclid(1.0);
        let col = hsv_to_rgb(hue, p.saturation, p.value);
        let col = egui::Color32::from_rgba_unmultiplied(col.0, col.1, col.2, 220);
        let stroke = egui::Stroke::new(p.stroke_width, col);
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

    painter.circle_stroke(
        centre,
        inner,
        egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
    );
    painter.text(
        rect.left_top() + egui::vec2(12.0, 12.0),
        egui::Align2::LEFT_TOP,
        format!(
            "Mandala · {} bins · smoothing α={:.2}",
            n,
            1.0 - p.smoothing
        ),
        egui::FontId::monospace(11.0),
        egui::Color32::from_gray(110),
    );
}

// ───────────────────── 3. Lorenz attractor ─────────────────────

fn draw_lorenz(
    painter: &egui::Painter,
    rect: egui::Rect,
    samples: &[(f32, f32)],
    state: &mut VisualizerState,
) {
    let p = state.params.lorenz.clone();
    if state.lorenz_trail.capacity() < p.trail_length {
        state
            .lorenz_trail
            .reserve(p.trail_length - state.lorenz_trail.capacity());
    }
    let rms = {
        let s: f32 = samples.iter().map(|(l, r)| 0.5 * (l * l + r * r)).sum();
        (s / samples.len().max(1) as f32).sqrt()
    };
    let centroid = spectral_centroid(samples);
    let sigma = p.sigma_base + centroid.clamp(0.0, 1.0) * p.centroid_drive;
    let rho = p.rho_base + rms.clamp(0.0, 0.4) * p.rms_rho_drive;
    let beta = p.beta_base + rms.clamp(0.0, 0.4) * p.rms_beta_drive;

    for _ in 0..p.steps_per_frame {
        state.lorenz_state = rk4_lorenz(state.lorenz_state, sigma, rho, beta, p.dt);
        if state.lorenz_trail.len() >= p.trail_length {
            state.lorenz_trail.remove(0);
        }
        state.lorenz_trail.push(state.lorenz_state);
    }

    let (mut min_x, mut max_x, mut min_z, mut max_z) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for &(x, _y, z) in &state.lorenz_trail {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_z = min_z.min(z);
        max_z = max_z.max(z);
    }
    let span_x = (max_x - min_x).max(1e-3);
    let span_z = (max_z - min_z).max(1e-3);
    let scale = (rect.width() / span_x).min(rect.height() / span_z) * 0.92;
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
        let t = i as f32 / n as f32;
        let col = hsv_to_rgb(0.7 - t * 0.7, 0.6, 0.85 + 0.15 * t);
        let alpha = 50 + (205.0 * t) as u8;
        let stroke = egui::Stroke::new(
            p.stroke_width,
            egui::Color32::from_rgba_unmultiplied(col.0, col.1, col.2, alpha),
        );
        painter.line_segment([project(x1, z1), project(x2, z2)], stroke);
    }

    painter.text(
        rect.left_top() + egui::vec2(12.0, 12.0),
        egui::Align2::LEFT_TOP,
        format!(
            "Lorenz · σ={sigma:.1}  ρ={rho:.1}  β={beta:.2}    \
             centroid={centroid:.2}  rms={rms:.3}"
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

// ───────────────────── 4. Chladni ─────────────────────

fn draw_chladni(
    painter: &egui::Painter,
    rect: egui::Rect,
    samples: &[(f32, f32)],
    state: &mut VisualizerState,
) {
    let p = state.params.chladni.clone();
    let mono: Vec<f32> = samples.iter().map(|(l, r)| 0.5 * (l + r)).collect();
    let spectrum = crate::analysis::spectrum(&mono);
    if spectrum.is_empty() {
        return;
    }

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

    state.chladni_phase += p.phase_speed;
    let phase = state.chladni_phase;

    let grid = p.grid_size;
    let cell_w = rect.width() / grid as f32;
    let cell_h = rect.height() / grid as f32;
    for gy in 0..grid {
        for gx in 0..grid {
            let x = gx as f32 / (grid - 1).max(1) as f32;
            let y = gy as f32 / (grid - 1).max(1) as f32;
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
            let mag = psi.abs().clamp(0.0, 2.0) * p.intensity;
            let hue_t = if psi >= 0.0 {
                p.hue_positive
            } else {
                p.hue_negative
            };
            let (r, g, b) = hsv_to_rgb(hue_t, 0.85, mag.clamp(0.0, 1.0));
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
        format!("Chladni · {} modes · grid {}²", MODES.len(), grid),
        egui::FontId::monospace(11.0),
        egui::Color32::from_gray(180),
    );
}

// ───────────────────── 5. Onion Skin (NEW) ─────────────────────

/// Multi-timescale trajectory through (centroid, RMS) feature space.
/// Layers note-scale recent trail, phrase-scale ghost, session-scale
/// watermark. The first mode designed against the "memoryless
/// visualisation is sterile" critique in
/// `docs/sound-vision-philosophy.md`.
fn draw_onion_skin(
    painter: &egui::Painter,
    rect: egui::Rect,
    samples: &[(f32, f32)],
    state: &mut VisualizerState,
    now: f64,
) {
    let p = state.params.onion.clone();

    // ── Compute current feature point ──────────────────────────────
    let rms = {
        let s: f32 = samples.iter().map(|(l, r)| 0.5 * (l * l + r * r)).sum();
        (s / samples.len().max(1) as f32).sqrt()
    };
    let centroid = spectral_centroid(samples).clamp(0.0, 1.0);
    // Smooth: heavy EMA reduces note-level jitter so we read TRAJECTORY,
    // not flicker. The user-facing 'smoothing' is the OLD weight, so
    // α = 1 - smoothing.
    let alpha = 1.0 - p.smoothing.clamp(0.0, 0.95);
    state.onion_smoothed.0 = (1.0 - alpha) * state.onion_smoothed.0 + alpha * centroid;
    state.onion_smoothed.1 = (1.0 - alpha) * state.onion_smoothed.1 + alpha * rms;
    let (cx, cy) = state.onion_smoothed;

    // ── Update temporal layers ─────────────────────────────────────
    state.onion_total_seconds += 0.033;

    // Recent trail: append every frame; drop old.
    state.onion_recent.push_back((cx, cy, now));
    while state.onion_recent.len() > p.recent_trail_len {
        state.onion_recent.pop_front();
    }

    // Ghost: decimated to one point per ~250 ms.
    if now - state.onion_last_ghost_time > 0.25 {
        state.onion_ghost.push_back((cx, cy));
        state.onion_last_ghost_time = now;
        // Drop ghost points older than ghost_seconds (~4 per second).
        let max_ghost = (p.ghost_seconds * 4.0) as usize;
        while state.onion_ghost.len() > max_ghost {
            state.onion_ghost.pop_front();
        }
    }

    // Watermark: bump the residency cell.
    let wg = p.watermark_grid.max(8);
    if state.onion_watermark.len() != wg * wg {
        state.onion_watermark = vec![0.0; wg * wg];
    }
    let wcx = ((cx.clamp(0.0, 1.0)) * (wg - 1) as f32) as usize;
    // RMS gets log-warped: the perceptually-relevant range is roughly
    // 0..0.4, but we want fine resolution near zero where most music
    // lives.
    let wcy_norm = (cy.clamp(0.0, 0.5) / 0.5).powf(0.5);
    let wcy = (wcy_norm * (wg - 1) as f32) as usize;
    let widx = wcy.min(wg - 1) * wg + wcx.min(wg - 1);
    if let Some(c) = state.onion_watermark.get_mut(widx) {
        *c += 0.033;
    }

    // ── Project feature space → screen ─────────────────────────────
    let pad = 36.0;
    let plot_rect = rect.shrink(pad);
    let project = |fx: f32, fy: f32| -> egui::Pos2 {
        let x = plot_rect.left() + fx.clamp(0.0, 1.0) * plot_rect.width();
        let y = plot_rect.bottom() - (fy.clamp(0.0, 0.5) / 0.5) * plot_rect.height();
        egui::pos2(x, y)
    };

    // ── Layer 1: watermark heatmap (faintest) ──────────────────────
    let max_w = state
        .onion_watermark
        .iter()
        .cloned()
        .fold(0.0_f32, f32::max);
    if max_w > 1e-6 {
        let cell_w = plot_rect.width() / wg as f32;
        let cell_h = plot_rect.height() / wg as f32;
        for gy in 0..wg {
            for gx in 0..wg {
                let v = state.onion_watermark[gy * wg + gx] / max_w;
                if v < 0.02 {
                    continue;
                }
                // Watermark uses a perceptually flat purple-to-white
                // ramp so it doesn't compete with the bright trail
                // hues on top.
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

    // ── Layer 2: ghost trail ───────────────────────────────────────
    let ghost_n = state.onion_ghost.len();
    if ghost_n > 1 {
        for i in 1..ghost_n {
            let (a_x, a_y) = state.onion_ghost[i - 1];
            let (b_x, b_y) = state.onion_ghost[i];
            let t = i as f32 / ghost_n as f32;
            let alpha = 30 + (60.0 * t) as u8;
            let col = egui::Color32::from_rgba_unmultiplied(140, 160, 200, alpha);
            painter.line_segment(
                [project(a_x, a_y), project(b_x, b_y)],
                egui::Stroke::new(1.0, col),
            );
        }
    }

    // ── Layer 3: bright recent trail ───────────────────────────────
    let recent: Vec<(f32, f32)> = state.onion_recent.iter().map(|&(x, y, _)| (x, y)).collect();
    let n = recent.len();
    if n > 1 {
        for i in 1..n {
            let (a_x, a_y) = recent[i - 1];
            let (b_x, b_y) = recent[i];
            let t = i as f32 / n as f32;
            // Hue rotates a bit along the trail for visual continuity;
            // alpha ramps from soft to bright at the head.
            let hue = (p.trail_hue + t * 0.15).rem_euclid(1.0);
            let (r, g, b) = hsv_to_rgb(hue, 0.7, 0.9);
            let alpha = 40 + (215.0 * t) as u8;
            let col = egui::Color32::from_rgba_unmultiplied(r, g, b, alpha);
            painter.line_segment(
                [project(a_x, a_y), project(b_x, b_y)],
                egui::Stroke::new(1.5 + 1.5 * t, col),
            );
        }
        // Bright head dot.
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

    // ── Layer 4 (optional): anticipated future ─────────────────────
    if p.show_future && n > 16 {
        // Linear extension via mean direction over the last 16 points.
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

    // ── Axes ───────────────────────────────────────────────────────
    if p.show_axes {
        let axis = egui::Color32::from_gray(80);
        // Axis lines around the plot.
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
        // Labels — corners.
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
            state.onion_total_seconds
        ),
        egui::FontId::monospace(11.0),
        egui::Color32::from_gray(160),
    );
}

// ───────────────────── helpers ─────────────────────

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

    #[test]
    fn default_params_round_trip_clone() {
        let p = VisualizerParams::default();
        let q = p.clone();
        // Spot-check a few fields rather than implementing PartialEq
        // across the whole tree.
        assert_eq!(p.mandala.bin_count, q.mandala.bin_count);
        assert_eq!(p.lorenz.steps_per_frame, q.lorenz.steps_per_frame);
        assert_eq!(p.onion.recent_trail_len, q.onion.recent_trail_len);
    }
}
