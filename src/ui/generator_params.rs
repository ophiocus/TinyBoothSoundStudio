//! Generator-track params modal (TBSS-FR-0009 step 5).
//!
//! Shown when the user clicks `File → Add Generator Track…`. Lets the
//! user pick the generator mode (Binaural / Isochronic / Layered) and
//! per-mode parameters, then commits via `Add & Bake`. The new track
//! is appended to `project.tracks` and immediately baked (if the
//! project has another stem to anchor the duration); otherwise it's
//! added in a not-yet-baked state with a clear status message.
//!
//! Layered is the "scope all three" architectural slot from the RFC —
//! the radio is offered for completeness but disabled with a tooltip;
//! its DSP is deferred (step 6 amounts to this disabled affordance).

use crate::app::TinyBoothApp;
use crate::project::GeneratorMode;
use eframe::egui;

pub fn show(app: &mut TinyBoothApp, ctx: &egui::Context) {
    if app.pending_generator_modal.is_none() {
        return;
    }

    let mut click_commit = false;
    let mut click_cancel = false;

    egui::Window::new("Add Generator Track")
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .collapsible(false)
        .resizable(false)
        .min_width(440.0)
        .show(ctx, |ui| {
            let Some(pending) = app.pending_generator_modal.as_mut() else {
                return;
            };

            ui.label(egui::RichText::new(
                "Synthesised focus-music stem (binaural beats / isochronic tones). \
                 Baked from the parameters below and lays into the mix at the \
                 longest other track's duration."
            ).weak());
            ui.add_space(6.0);

            // ── Mode picker ──────────────────────────────────────────
            ui.heading("Mode");
            let current_kind = mode_kind(&pending.mode);

            // Binaural radio.
            if ui
                .radio(current_kind == ModeKind::Binaural, "Binaural beats — stereo, needs headphones")
                .on_hover_text("Independent sine carriers on L/R at `carrier ± beat/2`. The brain perceives the L–R difference as a beat at `beat_hz`.")
                .clicked()
            {
                pending.mode = GeneratorMode::Binaural {
                    carrier_hz: 200.0,
                    beat_hz: 10.0,
                    amplitude: 0.3,
                };
            }
            // Isochronic radio.
            if ui
                .radio(current_kind == ModeKind::Isochronic, "Isochronic tones — works over speakers")
                .on_hover_text("Single sine carrier modulated by a smoothed pulse envelope. No headphones required.")
                .clicked()
            {
                pending.mode = GeneratorMode::Isochronic {
                    tone_hz: 200.0,
                    pulse_hz: 10.0,
                    duty_cycle: 0.5,
                    amplitude: 0.3,
                };
            }
            // Layered — disabled per TBSS-FR-0009 step 6 (architectural
            // slot reserved; DSP deferred).
            ui.add_enabled_ui(false, |ui| {
                ui.radio(false, "Layered focus music — coming in a follow-up")
                    .on_hover_text(
                        "Background drone / ambient pad layered with an entrainment \
                         carrier. The architectural slot is reserved; the DSP design \
                         lands in a later release.",
                    );
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);

            // ── Per-mode parameter fields ────────────────────────────
            ui.heading("Parameters");
            match &mut pending.mode {
                GeneratorMode::Binaural {
                    carrier_hz,
                    beat_hz,
                    amplitude,
                } => {
                    egui::Grid::new("bin_params")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Carrier (Hz)")
                                .on_hover_text("Centre frequency of both sines. 100–400 Hz is comfortable for sustained listening.");
                            ui.add(egui::Slider::new(carrier_hz, 40.0..=800.0).suffix(" Hz"));
                            ui.end_row();

                            ui.label("Beat (Hz)")
                                .on_hover_text("L–R difference. Brain-state bands: delta 0.5–4, theta 4–8, alpha 8–12, beta 12–30, gamma 30+.");
                            ui.add(egui::Slider::new(beat_hz, 0.5..=40.0).suffix(" Hz"));
                            ui.end_row();

                            ui.label("Amplitude")
                                .on_hover_text("Peak per channel, 0..1. Bake then trim via the lane fader for finer mix-level control.");
                            ui.add(egui::Slider::new(amplitude, 0.0..=1.0));
                            ui.end_row();
                        });
                }
                GeneratorMode::Isochronic {
                    tone_hz,
                    pulse_hz,
                    duty_cycle,
                    amplitude,
                } => {
                    egui::Grid::new("iso_params")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Tone (Hz)")
                                .on_hover_text("Sine carrier frequency. 100–400 Hz is comfortable for sustained listening.");
                            ui.add(egui::Slider::new(tone_hz, 40.0..=800.0).suffix(" Hz"));
                            ui.end_row();

                            ui.label("Pulse (Hz)")
                                .on_hover_text("Envelope pulse rate. Brain-state bands: delta 0.5–4, theta 4–8, alpha 8–12, beta 12–30, gamma 30+.");
                            ui.add(egui::Slider::new(pulse_hz, 0.5..=40.0).suffix(" Hz"));
                            ui.end_row();

                            ui.label("Duty cycle")
                                .on_hover_text("Fraction of one pulse period that is `on`, 0..1. 0.5 = symmetric on/off.");
                            ui.add(egui::Slider::new(duty_cycle, 0.05..=0.95));
                            ui.end_row();

                            ui.label("Amplitude")
                                .on_hover_text("Peak per channel, 0..1.");
                            ui.add(egui::Slider::new(amplitude, 0.0..=1.0));
                            ui.end_row();
                        });
                }
                GeneratorMode::Layered => {
                    // Unreachable in normal use — the radio is disabled
                    // — but handle gracefully if state ever shows it.
                    ui.label("Layered mode has no editable parameters yet.");
                }
            }

            ui.add_space(8.0);
            ui.colored_label(
                egui::Color32::from_rgb(150, 200, 150),
                "On Add: the track is created and immediately baked. \
                 If the project has no other stems, the track lands un-baked — \
                 import a stem then re-create.",
            );

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new("Add & Bake").min_size(egui::vec2(140.0, 30.0)))
                    .clicked()
                {
                    click_commit = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new("Cancel").min_size(egui::vec2(100.0, 30.0)))
                        .clicked()
                    {
                        click_cancel = true;
                    }
                });
            });
        });

    if click_commit {
        app.resolve_generator_modal(true);
    } else if click_cancel {
        app.resolve_generator_modal(false);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModeKind {
    Binaural,
    Isochronic,
    Layered,
}

fn mode_kind(mode: &GeneratorMode) -> ModeKind {
    match mode {
        GeneratorMode::Binaural { .. } => ModeKind::Binaural,
        GeneratorMode::Isochronic { .. } => ModeKind::Isochronic,
        GeneratorMode::Layered => ModeKind::Layered,
    }
}
