use crate::app::TinyBoothApp;
use crate::audio;
use crate::ui::viz;
use eframe::egui;

pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    ui.heading("Record");
    ui.separator();

    // ── Recording tone ──────────────────────────────────────────────
    let recording = app.session.is_some();
    ui.horizontal(|ui| {
        ui.label("Recording tone:");
        let current_name = app.active_profile().name.clone();
        let combo = egui::ComboBox::from_id_source("profile_combo")
            .selected_text(current_name)
            .width(240.0);
        // Locked while recording — can't swap the chain mid-take.
        ui.add_enabled_ui(!recording, |ui| {
            combo.show_ui(ui, |ui| {
                for (i, p) in app.profiles.clone().iter().enumerate() {
                    if ui
                        .selectable_label(i == app.active_profile_idx, &p.name)
                        .on_hover_text(&p.description)
                        .clicked()
                    {
                        app.set_active_profile(i);
                    }
                }
            });
        });
        if ui.button("Admin…").on_hover_text("Edit profile parameters").clicked() {
            app.show_admin = true;
            app.admin_edit_idx = Some(app.active_profile_idx);
        }
        if recording {
            ui.colored_label(egui::Color32::LIGHT_YELLOW, "(locked while recording)");
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.weak(app.active_profile().description.clone());
    });
    ui.separator();

    // ── Device picker ───────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label("Input device:");
        let current = app
            .selected_device
            .clone()
            .unwrap_or_else(|| "— none —".into());
        egui::ComboBox::from_id_source("device_combo")
            .selected_text(current.clone())
            .width(320.0)
            .show_ui(ui, |ui| {
                for dev in &app.devices {
                    if ui
                        .selectable_label(
                            app.selected_device.as_deref() == Some(&dev.name),
                            format!("{}  ({} ch, {} Hz)", dev.name, dev.channels, dev.sample_rate),
                        )
                        .clicked()
                    {
                        app.selected_device = Some(dev.name.clone());
                        // Reset channel selection if the new device has fewer channels.
                        if let Some(sel) = app.selected_channel {
                            if sel >= dev.channels {
                                app.selected_channel = None;
                            }
                        }
                    }
                }
            });
        if ui.button("Refresh").clicked() {
            app.devices = audio::list_input_devices();
        }
    });

    // ── Channel picker ──────────────────────────────────────────────
    let channel_count = app
        .selected_device
        .as_ref()
        .and_then(|n| app.devices.iter().find(|d| &d.name == n))
        .map(|d| d.channels)
        .unwrap_or(0);
    ui.horizontal(|ui| {
        ui.label("Source:");
        ui.radio_value(&mut app.selected_channel, None, "All (mixdown)");
        for c in 0..channel_count {
            ui.radio_value(&mut app.selected_channel, Some(c), format!("Ch {}", c + 1));
        }
    });

    // ── Track naming ────────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label("New track name:");
        ui.add(
            egui::TextEdit::singleline(&mut app.pending_track_name)
                .desired_width(260.0)
                .hint_text("(auto: track-001, track-002, …)"),
        );
    });

    // ── Transport ──────────────────────────────────────────────────
    ui.horizontal(|ui| {
        let recording = app.session.is_some();
        if !recording {
            let enabled = app.selected_device.is_some();
            if ui
                .add_enabled(enabled, egui::Button::new("⏺  Record").min_size(egui::vec2(120.0, 32.0)))
                .clicked()
            {
                if let Err(e) = app.start_new_take() {
                    app.status = Some(format!("record error: {e}"));
                }
            }
        } else {
            if ui
                .add(egui::Button::new("⏹  Stop").min_size(egui::vec2(120.0, 32.0)))
                .clicked()
            {
                app.stop_take();
            }
        }
        if let Some(sess) = app.session.as_ref() {
            ui.label(format!("REC  {:.1}s", sess.duration_secs()));
            ui.label(format!("file: {}", sess.wav_path.file_name().unwrap_or_default().to_string_lossy()));
        }
    });

    ui.add_space(8.0);

    // ── Visualisation ───────────────────────────────────────────────
    let samples = app.viz.snapshot(app.viz.sample_rate.load(std::sync::atomic::Ordering::Relaxed) as usize * 2);
    ui.label("Waveform (last 2 seconds)");
    viz::draw_waveform(ui, &samples, 140.0);
    ui.add_space(6.0);
    ui.label("Spectrum");
    viz::draw_spectrum(ui, &samples, 140.0);
    ui.add_space(6.0);
    ui.label(format!("Input level — peak {:.2}", app.viz.peak()));
    viz::draw_meter(ui, app.viz.peak());

    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.label("Each take is saved as a separate WAV under");
        ui.monospace(app.project.tracks_dir().display().to_string());
    });
}
