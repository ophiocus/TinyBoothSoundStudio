//! Admin → Telemetry settings… panel (TBSS-FR-0005 §"User-tweakable
//! thresholds"). Modal with a small set of sliders that control the
//! analyzer's onset / pitch / polyphony cutoffs. Edits persist to
//! `%APPDATA%\TinyBooth Sound Studio\telemetry_settings.json`.
//!
//! Editing here doesn't auto-re-analyze — the user has to invalidate
//! a track (change its profile, trim it, or delete-and-reimport) for
//! the new thresholds to take effect. Reasonable: thresholds are
//! "calibration" knobs, not per-take settings.

use crate::app::TinyBoothApp;
use eframe::egui;

pub fn show(app: &mut TinyBoothApp, ctx: &egui::Context) {
    if !app.show_telemetry_settings {
        return;
    }
    let mut open = true;
    let mut dirty = false;

    egui::Window::new("Telemetry settings")
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .min_width(520.0)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(
                    "Calibration knobs for the per-track analyzer. Defaults are \
                     conservative on Suno output. Edits persist immediately; tracks \
                     re-analyze the next time their telemetry is invalidated (profile \
                     change, Trim, or re-import).",
                )
                .small()
                .color(egui::Color32::from_gray(160)),
            );
            ui.add_space(6.0);

            let s = &mut app.telemetry_settings;

            // ── Onset detection ───────────────────────────────────
            ui.label(egui::RichText::new("Onset detection").strong());
            dirty |= ui
                .add(
                    egui::Slider::new(&mut s.drum_onset_k_mad, 1.0..=8.0)
                        .text("k · MAD threshold")
                        .step_by(0.1),
                )
                .on_hover_text(
                    "Adaptive peak-pick threshold for spectral-flux onset \
                     detection. Higher = fewer onsets detected. Default 3.0.",
                )
                .changed();
            ui.add_space(8.0);

            // ── Pick velocity ─────────────────────────────────────
            ui.label(egui::RichText::new("Pick velocity (guitar / bass)").strong());
            dirty |= ui
                .add(
                    egui::Slider::new(&mut s.guitar_pick_threshold, 0.005..=0.30)
                        .text("Guitar — min velocity")
                        .step_by(0.005),
                )
                .on_hover_text(
                    "Minimum peak amplitude for an onset to count as a guitar \
                     pick. Below this it's classified as Noise. Default 0.05.",
                )
                .changed();
            dirty |= ui
                .add(
                    egui::Slider::new(&mut s.bass_pick_threshold, 0.005..=0.30)
                        .text("Bass — min velocity")
                        .step_by(0.005),
                )
                .on_hover_text(
                    "Same shape but for bass — usually slightly lower because \
                     basses pluck quieter relative to peak. Default 0.04.",
                )
                .changed();
            ui.add_space(8.0);

            // ── Pitch tracker ─────────────────────────────────────
            ui.label(egui::RichText::new("Pitch tracker (YIN)").strong());
            dirty |= ui
                .add(
                    egui::Slider::new(&mut s.yin_threshold, 0.05..=0.40)
                        .text("YIN threshold")
                        .step_by(0.01),
                )
                .on_hover_text(
                    "Cumulative-mean-difference threshold below which YIN \
                     accepts a lag as the fundamental. 0.10–0.20 is the \
                     standard range; lower = stricter. Default 0.15.",
                )
                .changed();
            dirty |= ui
                .add(
                    egui::Slider::new(&mut s.same_pitch_cents, 10.0..=200.0)
                        .text("Same-pitch tolerance (cents)")
                        .step_by(5.0),
                )
                .on_hover_text(
                    "Two events whose pitches are within this many cents \
                     are classified as Repeat (vs. Pluck). 50 ≈ a quarter \
                     tone. Default 50.",
                )
                .changed();
            ui.add_space(8.0);

            // ── Polyphony ─────────────────────────────────────────
            ui.label(egui::RichText::new("Polyphony probe").strong());
            let mut poly = s.polyphony_peak_count as i32;
            if ui
                .add(egui::Slider::new(&mut poly, 2..=12).text("Polyphony cutoff (peaks)"))
                .on_hover_text(
                    "Number of spectral peaks above –12 dB from max in the \
                     post-onset window required to flag the event as a Strum. \
                     Lower = more events classified as polyphonic. Default 5.",
                )
                .changed()
            {
                s.polyphony_peak_count = poly.max(1) as usize;
                dirty = true;
            }

            ui.add_space(10.0);
            ui.separator();

            ui.horizontal(|ui| {
                if ui.button("Reset to defaults").clicked() {
                    *s = crate::telemetry::TelemetrySettings::default();
                    dirty = true;
                }
                ui.add_space(8.0);
                if ui.button("Close").clicked() {
                    // Closing also persists below.
                    app.show_telemetry_settings = false;
                }
            });
        });

    if dirty {
        if let Err(e) = app.telemetry_settings.save() {
            app.status = Some(format!("telemetry settings save error: {e:#}"));
        }
    }

    if !open {
        app.show_telemetry_settings = false;
    }
}
