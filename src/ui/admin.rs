//! Admin window for viewing and editing recording-tone profile parameters.
//!
//! Every number on a `Profile` is exposed as a labelled drag-value (so you
//! can type a number directly or scrub with the mouse). Changes are in-memory
//! until you press Save — then they're written to `profiles.json` under
//! `%APPDATA%\TinyBooth Sound Studio\`.

use crate::app::TinyBoothApp;
use crate::dsp::{EqBandKind, Profile};
use eframe::egui;

pub fn show(app: &mut TinyBoothApp, ctx: &egui::Context) {
    let mut open = app.show_admin;
    egui::Window::new("Recording-tone profiles")
        .open(&mut open)
        .default_size([720.0, 520.0])
        .min_size([560.0, 420.0])
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            render_body(app, ui);
        });
    app.show_admin = open;
}

fn render_body(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        if ui
            .button("+ New")
            .on_hover_text("Duplicate the selected profile")
            .clicked()
        {
            let src = app
                .admin_edit_idx
                .and_then(|i| app.profiles.get(i).cloned())
                .unwrap_or_else(|| Profile::raw("Custom"));
            let mut np = src.clone();
            np.name = format!("{} (copy)", src.name);
            app.profiles.push(np);
            app.admin_edit_idx = Some(app.profiles.len() - 1);
        }
        if ui.button("Save all").clicked() {
            app.save_profiles();
        }
        if ui
            .button("Reset to defaults")
            .on_hover_text("Discard all edits and restore the built-in profiles")
            .clicked()
        {
            app.reset_profiles_to_defaults();
            app.admin_edit_idx = Some(0);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(s) = app.admin_status.as_ref() {
                ui.weak(s.clone());
            }
            if let Some(p) = crate::dsp::profiles_path() {
                ui.weak(format!("file: {}", p.display()));
            }
        });
    });

    ui.separator();

    egui::SidePanel::left("profile_list")
        .resizable(true)
        .default_width(180.0)
        .show_inside(ui, |ui| {
            ui.strong("Profiles");
            ui.add_space(4.0);
            let mut delete: Option<usize> = None;
            for i in 0..app.profiles.len() {
                ui.horizontal(|ui| {
                    let selected = app.admin_edit_idx == Some(i);
                    let label = if i == app.active_profile_idx {
                        format!("● {}", app.profiles[i].name)
                    } else {
                        app.profiles[i].name.clone()
                    };
                    if ui.selectable_label(selected, label).clicked() {
                        app.admin_edit_idx = Some(i);
                    }
                    if ui
                        .small_button("✖")
                        .on_hover_text("delete profile")
                        .clicked()
                    {
                        delete = Some(i);
                    }
                });
            }
            if let Some(i) = delete {
                if app.profiles.len() > 1 {
                    app.profiles.remove(i);
                    // Keep the active profile index valid.
                    if app.active_profile_idx >= app.profiles.len() {
                        app.active_profile_idx = app.profiles.len() - 1;
                    }
                    if let Some(ei) = app.admin_edit_idx {
                        if ei >= app.profiles.len() {
                            app.admin_edit_idx = Some(app.profiles.len() - 1);
                        }
                    }
                } else {
                    app.admin_status = Some("Keep at least one profile.".into());
                }
            }
        });

    egui::CentralPanel::default().show_inside(ui, |ui| {
        let Some(idx) = app.admin_edit_idx.filter(|i| *i < app.profiles.len()) else {
            ui.label("Select a profile on the left.");
            return;
        };
        render_editor(&mut app.profiles[idx], ui);
    });
}

fn render_editor(p: &mut Profile, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.heading(&p.name);

        egui::Grid::new("profile_meta")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(260.0));
                ui.end_row();
                ui.label("Description");
                ui.add(
                    egui::TextEdit::multiline(&mut p.description)
                        .desired_width(460.0)
                        .desired_rows(2),
                );
                ui.end_row();
            });

        ui.add_space(10.0);
        ui.strong("Input");
        row(ui, "Input gain (dB)", |ui| {
            ui.add(
                egui::DragValue::new(&mut p.input_gain_db)
                    .speed(0.1)
                    .suffix(" dB")
                    .range(-24.0..=24.0),
            );
        });

        ui.add_space(10.0);
        ui.strong("High-pass filter");
        row(ui, "Enabled", |ui| {
            ui.checkbox(&mut p.hpf_enabled, "");
        });
        row(ui, "Cutoff (Hz)", |ui| {
            ui.add(
                egui::DragValue::new(&mut p.hpf_hz)
                    .speed(1.0)
                    .suffix(" Hz")
                    .range(20.0..=1000.0),
            );
        });

        ui.add_space(10.0);
        ui.strong("Noise gate");
        row(ui, "Enabled", |ui| {
            ui.checkbox(&mut p.gate_enabled, "");
        });
        row(ui, "Threshold (dB)", |ui| {
            ui.add(
                egui::DragValue::new(&mut p.gate_threshold_db)
                    .speed(0.5)
                    .suffix(" dB")
                    .range(-80.0..=0.0),
            );
        });
        row(ui, "Attack (ms)", |ui| {
            ui.add(
                egui::DragValue::new(&mut p.gate_attack_ms)
                    .speed(0.5)
                    .suffix(" ms")
                    .range(0.1..=200.0),
            );
        });
        row(ui, "Release (ms)", |ui| {
            ui.add(
                egui::DragValue::new(&mut p.gate_release_ms)
                    .speed(1.0)
                    .suffix(" ms")
                    .range(1.0..=2000.0),
            );
        });

        ui.add_space(10.0);
        ui.strong("Parametric EQ (4 bands)");
        ui.label(
            egui::RichText::new("Bands with kind = Bypass are skipped.")
                .italics()
                .weak(),
        );
        for (i, band) in p.eq_bands.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add_sized([60.0, 20.0], egui::Label::new(format!("Band {}", i + 1)));
                egui::ComboBox::from_id_source(format!("eq_kind_{i}"))
                    .selected_text(band.kind.label())
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for k in [
                            EqBandKind::Bypass,
                            EqBandKind::Peak,
                            EqBandKind::LowShelf,
                            EqBandKind::HighShelf,
                        ] {
                            ui.selectable_value(&mut band.kind, k, k.label());
                        }
                    });
                let active = band.kind != EqBandKind::Bypass;
                ui.add_enabled_ui(active, |ui| {
                    ui.label("Hz");
                    ui.add(
                        egui::DragValue::new(&mut band.hz)
                            .speed(1.0)
                            .range(20.0..=20_000.0),
                    );
                    ui.label("Gain");
                    ui.add(
                        egui::DragValue::new(&mut band.gain_db)
                            .speed(0.1)
                            .suffix(" dB")
                            .range(-24.0..=24.0),
                    );
                    ui.label("Q");
                    ui.add(
                        egui::DragValue::new(&mut band.q)
                            .speed(0.05)
                            .range(0.1..=10.0),
                    );
                });
            });
        }

        ui.add_space(10.0);
        ui.strong("De-esser");
        row(ui, "Enabled", |ui| {
            ui.checkbox(&mut p.deess_enabled, "");
        });
        row(ui, "Frequency (Hz)", |ui| {
            ui.add(
                egui::DragValue::new(&mut p.deess_hz)
                    .speed(50.0)
                    .suffix(" Hz")
                    .range(2_000.0..=14_000.0),
            );
        });
        row(ui, "Threshold (dB)", |ui| {
            ui.add(
                egui::DragValue::new(&mut p.deess_threshold_db)
                    .speed(0.5)
                    .suffix(" dB")
                    .range(-60.0..=0.0),
            );
        });
        row(ui, "Ratio (x:1)", |ui| {
            ui.add(
                egui::DragValue::new(&mut p.deess_ratio)
                    .speed(0.1)
                    .range(1.0..=12.0),
            );
        });

        ui.add_space(10.0);
        ui.strong("Compressor");
        row(ui, "Enabled", |ui| {
            ui.checkbox(&mut p.compressor_enabled, "");
        });
        row(ui, "Threshold (dB)", |ui| {
            ui.add(
                egui::DragValue::new(&mut p.compressor_threshold_db)
                    .speed(0.5)
                    .suffix(" dB")
                    .range(-60.0..=0.0),
            );
        });
        row(ui, "Ratio (x:1)", |ui| {
            ui.add(
                egui::DragValue::new(&mut p.compressor_ratio)
                    .speed(0.1)
                    .range(1.0..=20.0),
            );
        });
        row(ui, "Attack (ms)", |ui| {
            ui.add(
                egui::DragValue::new(&mut p.compressor_attack_ms)
                    .speed(0.5)
                    .suffix(" ms")
                    .range(0.1..=200.0),
            );
        });
        row(ui, "Release (ms)", |ui| {
            ui.add(
                egui::DragValue::new(&mut p.compressor_release_ms)
                    .speed(1.0)
                    .suffix(" ms")
                    .range(1.0..=2000.0),
            );
        });
        row(ui, "Makeup gain (dB)", |ui| {
            ui.add(
                egui::DragValue::new(&mut p.compressor_makeup_db)
                    .speed(0.1)
                    .suffix(" dB")
                    .range(-12.0..=24.0),
            );
        });

        ui.add_space(14.0);
        ui.label(egui::RichText::new(
            "Changes take effect for the next recording. Press Save all above to persist to disk."
        ).italics().weak());
    });
}

fn row(ui: &mut egui::Ui, label: &str, contents: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized([160.0, 20.0], egui::Label::new(label));
        contents(ui);
    });
}
