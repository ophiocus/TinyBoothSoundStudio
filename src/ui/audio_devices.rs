//! Admin → Audio devices… panel.
//!
//! Lets the user pick the master input device (recording) and output
//! device (Mix-tab playback + export-preview). Both pick persist via
//! `Config.input_device` / `Config.output_device`; the empty pick
//! means "follow the platform default".
//!
//! Why a single Admin panel rather than scattering selectors across
//! Record + Mix tabs? Because the *master* nature of these settings
//! — they outlive any one tab, drive both recording and playback —
//! belongs in Admin alongside other app-wide configuration like the
//! recording-tone profiles and the telemetry thresholds. The Mix
//! tab gets an inline shortcut (current output + "Audio devices…"
//! link) so the user doesn't have to dig two levels deep mid-mix.
//!
//! Added v0.4.27.

use crate::app::TinyBoothApp;
use eframe::egui;

pub fn show(app: &mut TinyBoothApp, ctx: &egui::Context) {
    if !app.show_audio_devices {
        return;
    }
    let mut open = true;
    let mut dirty = false;
    let mut rescan = false;
    let mut rebuild_player = false;

    egui::Window::new("Audio devices")
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .min_width(560.0)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(
                    "Master input / output for the whole app. The input is used by \
                     the Record tab; the output is used by Mix-tab playback. \
                     Empty pick = follow the platform default (whatever Windows \
                     reports as your default recording / playback device).",
                )
                .small()
                .color(egui::Color32::from_gray(160)),
            );
            ui.add_space(8.0);

            // ── Input ───────────────────────────────────────────
            ui.label(egui::RichText::new("Master input (recording)").strong());
            let inputs = crate::audio::list_input_devices();
            let current = app.config.input_device.clone();
            let current_label = match &current {
                Some(name) => name.clone(),
                None => "(system default)".into(),
            };
            egui::ComboBox::from_id_source("audio_input_device")
                .selected_text(current_label)
                .width(440.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(current.is_none(), "(system default)")
                        .clicked()
                        && current.is_some()
                    {
                        app.config.input_device = None;
                        // Sync selected_device to current effective default.
                        app.selected_device = inputs.first().map(|d| d.name.clone());
                        dirty = true;
                    }
                    for dev in &inputs {
                        let active = current.as_deref() == Some(dev.name.as_str());
                        let label = format!(
                            "{}   ·   {} ch, {} Hz",
                            dev.name, dev.channels, dev.sample_rate
                        );
                        if ui.selectable_label(active, label).clicked() && !active {
                            app.config.input_device = Some(dev.name.clone());
                            app.selected_device = Some(dev.name.clone());
                            dirty = true;
                        }
                    }
                });
            if inputs.is_empty() {
                ui.label(
                    egui::RichText::new("No input devices detected.")
                        .small()
                        .color(egui::Color32::from_rgb(220, 120, 120)),
                );
            }

            ui.add_space(10.0);

            // ── Output ──────────────────────────────────────────
            ui.label(egui::RichText::new("Master output (Mix-tab playback)").strong());
            let outputs = crate::audio::list_output_devices();
            let current_out = app.config.output_device.clone();
            let current_out_label = match &current_out {
                Some(name) => name.clone(),
                None => "(system default)".into(),
            };
            egui::ComboBox::from_id_source("audio_output_device")
                .selected_text(current_out_label)
                .width(440.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(current_out.is_none(), "(system default)")
                        .clicked()
                        && current_out.is_some()
                    {
                        app.config.output_device = None;
                        dirty = true;
                        rebuild_player = true;
                    }
                    for dev in &outputs {
                        let active = current_out.as_deref() == Some(dev.name.as_str());
                        let label = format!(
                            "{}   ·   {} ch, {} Hz",
                            dev.name, dev.channels, dev.sample_rate
                        );
                        if ui.selectable_label(active, label).clicked() && !active {
                            app.config.output_device = Some(dev.name.clone());
                            dirty = true;
                            rebuild_player = true;
                        }
                    }
                });
            if outputs.is_empty() {
                ui.label(
                    egui::RichText::new("No output devices detected.")
                        .small()
                        .color(egui::Color32::from_rgb(220, 120, 120)),
                );
            }

            ui.add_space(12.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button("↻ Rescan devices")
                    .on_hover_text(
                        "Re-enumerate cpal's device list. Useful after \
                         plugging / unplugging audio hardware while the \
                         app is open.",
                    )
                    .clicked()
                {
                    rescan = true;
                }
                ui.add_space(8.0);
                if ui.button("Close").clicked() {
                    app.show_audio_devices = false;
                }
            });
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Changing the output device rebuilds the Mix-tab player. \
                     If playback was in progress, it'll stop and you'll need \
                     to hit Play again.",
                )
                .small()
                .color(egui::Color32::from_gray(150)),
            );
        });

    if dirty {
        app.config.save_or_log();
    }
    if rescan {
        // Cheap — the dropdown will re-call list_input_devices /
        // list_output_devices on the next frame. Nothing to do here
        // except trigger a repaint by setting status.
        app.status = Some("Re-enumerated audio devices.".into());
    }
    if rebuild_player {
        // Force the Mix tab to rebuild the player with the new output
        // device on its next render. v0.4.27.
        app.player = None;
        app.player_attempt_failed_for = None;
        app.player_error = None;
    }
    if !open {
        app.show_audio_devices = false;
    }
}
