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
                        // Reset source mode if it's no longer valid for the new device.
                        match app.selected_mode {
                            crate::audio::SourceMode::Channel(sel) if sel >= dev.channels => {
                                app.selected_mode = crate::audio::SourceMode::Mixdown;
                            }
                            crate::audio::SourceMode::Stereo if dev.channels < 2 => {
                                app.selected_mode = crate::audio::SourceMode::Mixdown;
                            }
                            _ => {}
                        }
                    }
                }
            });
        if ui.button("Refresh").clicked() {
            app.devices = audio::list_input_devices();
        }
    });

    // ── Source mode ─────────────────────────────────────────────────
    // Mixdown and Ch 1 are always offered. Ch 2+ appear for multi-ch devices.
    // Stereo is offered when the device has at least 2 input channels.
    let channel_count = app
        .selected_device
        .as_ref()
        .and_then(|n| app.devices.iter().find(|d| &d.name == n))
        .map(|d| d.channels)
        .unwrap_or(0);
    ui.horizontal_wrapped(|ui| {
        use crate::audio::SourceMode;
        ui.label("Source:");
        ui.radio_value(&mut app.selected_mode, SourceMode::Mixdown, "All (mixdown → mono)");
        for c in 0..channel_count {
            ui.radio_value(
                &mut app.selected_mode,
                SourceMode::Channel(c),
                format!("Ch {} → mono", c + 1),
            );
        }
        if channel_count >= 2 {
            ui.radio_value(
                &mut app.selected_mode,
                SourceMode::Stereo,
                "Stereo (Ch 1 + Ch 2 → L/R)",
            );
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
    let sample_rate = app.viz.sample_rate.load(std::sync::atomic::Ordering::Relaxed) as usize;
    let window = sample_rate * 2; // 2 seconds
    let left = app.viz.snapshot_left(window);
    let stereo = app.viz.is_stereo();

    if stereo {
        let right = app.viz.snapshot_right(window);
        ui.label("Waveform — L (last 2 seconds)");
        viz::draw_waveform(ui, &left, 80.0);
        ui.add_space(2.0);
        ui.label("Waveform — R");
        viz::draw_waveform(ui, &right, 80.0);
        ui.add_space(6.0);
        ui.label("Spectrum (L+R sum)");
        // Sum L+R for the spectrum — overlapping stereo spectra are visually noisy.
        let sum: Vec<f32> = left.iter().zip(right.iter()).map(|(l, r)| 0.5 * (l + r)).collect();
        viz::draw_spectrum(ui, &sum, 140.0);
        ui.add_space(6.0);
        let pl = app.viz.peak_left();
        let pr = app.viz.peak_right();
        ui.label(format!("Input level — L {:.2}   R {:.2}", pl, pr));
        viz::draw_meter(ui, pl);
        ui.add_space(2.0);
        viz::draw_meter(ui, pr);
    } else {
        ui.label("Waveform (last 2 seconds)");
        viz::draw_waveform(ui, &left, 140.0);
        ui.add_space(6.0);
        ui.label("Spectrum");
        viz::draw_spectrum(ui, &left, 140.0);
        ui.add_space(6.0);
        let p = app.viz.peak_left();
        ui.label(format!("Input level — peak {:.2}", p));
        viz::draw_meter(ui, p);
    }

    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.label("Each take is saved as a separate WAV under");
        ui.monospace(app.project.tracks_dir().display().to_string());
    });
}
