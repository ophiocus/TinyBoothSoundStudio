//! Per-track Correction editor — floating window opened from the Mix
//! tab. Operates directly on `Project.tracks[i].correction`. Mutations
//! also flow into the player's matching `TrackPlay.correction_profile`
//! so the change is audible the next playback cycle (the audio thread
//! polls a generation counter to rebuild its local FilterChainStereo).

use crate::app::TinyBoothApp;
use crate::dsp::{EqBandKind, Profile};
use eframe::egui;

pub fn show(app: &mut TinyBoothApp, ctx: &egui::Context) {
    let Some(idx) = app.editing_correction_for else { return };
    if idx >= app.project.tracks.len() {
        app.editing_correction_for = None;
        return;
    }

    let mut open = true;
    let title = format!("🪞  Correction · {}", app.project.tracks[idx].name);
    egui::Window::new(title)
        .open(&mut open)
        .default_size([520.0, 600.0])
        .min_size([460.0, 460.0])
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| body(app, idx, ui));
    if !open {
        app.editing_correction_for = None;
    }
}

fn body(app: &mut TinyBoothApp, idx: usize, ui: &mut egui::Ui) {
    // Disable / enable header.
    ui.horizontal(|ui| {
        let has_corr = app.project.tracks[idx].correction.is_some();
        ui.label(egui::RichText::new(if has_corr { "Active" } else { "Disabled" })
            .color(if has_corr { egui::Color32::from_rgb(100, 220, 150) } else { egui::Color32::DARK_GRAY }));
        if has_corr {
            if ui.button("Disable correction").clicked() {
                app.project.tracks[idx].correction = None;
                app.project_dirty = true;
                push_to_player(app, idx, None);
                return;
            }
        } else {
            if ui.button("Enable with Suno-Clean preset").clicked() {
                let seed = app.profiles.iter().find(|p| p.name == "Suno-Clean").cloned();
                app.project.tracks[idx].correction = seed.clone();
                app.project_dirty = true;
                push_to_player(app, idx, seed);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.weak("Edits apply to the next playback cycle.");
        });
    });
    ui.separator();

    let Some(p) = app.project.tracks[idx].correction.as_mut() else {
        ui.label("Correction is currently disabled — enable it above to edit a chain.");
        return;
    };

    // Save dirty if anything changed inside the editor.
    let mut any_changed = false;
    egui::ScrollArea::vertical().show(ui, |ui| {
        any_changed |= editor_body(p, ui);
    });

    if any_changed {
        app.project_dirty = true;
        let snapshot = app.project.tracks[idx].correction.clone();
        push_to_player(app, idx, snapshot);
    }
}

/// Bumps the matching player track's correction generation so the audio
/// thread rebuilds its FilterChainStereo on the next callback.
fn push_to_player(app: &mut TinyBoothApp, idx: usize, profile: Option<Profile>) {
    if let Some(player) = app.player.as_ref() {
        if let Some(track) = player.state.tracks.get(idx) {
            track.set_correction(profile);
        }
    }
}

fn editor_body(p: &mut Profile, ui: &mut egui::Ui) -> bool {
    let mut changed = false;
    egui::Grid::new("correction_meta").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Preset name");
        if ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(260.0)).changed() { changed = true; }
        ui.end_row();
        ui.label("Description");
        if ui.add(egui::TextEdit::multiline(&mut p.description).desired_width(420.0).desired_rows(2)).changed() { changed = true; }
        ui.end_row();
    });

    ui.add_space(8.0);
    ui.strong("Input");
    changed |= row(ui, "Input gain (dB)", |ui| {
        ui.add(egui::DragValue::new(&mut p.input_gain_db).speed(0.1).suffix(" dB").range(-24.0..=24.0)).changed()
    });

    ui.add_space(8.0);
    ui.strong("High-pass filter");
    changed |= row(ui, "Enabled", |ui| ui.checkbox(&mut p.hpf_enabled, "").changed());
    changed |= row(ui, "Cutoff (Hz)", |ui| {
        ui.add(egui::DragValue::new(&mut p.hpf_hz).speed(1.0).suffix(" Hz").range(20.0..=1000.0)).changed()
    });

    ui.add_space(8.0);
    ui.strong("Parametric EQ (4 bands)");
    for (i, band) in p.eq_bands.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.add_sized([60.0, 20.0], egui::Label::new(format!("Band {}", i + 1)));
            let resp = egui::ComboBox::from_id_source(format!("corr_eq_{i}"))
                .selected_text(band.kind.label())
                .width(110.0)
                .show_ui(ui, |ui| {
                    let mut local = false;
                    for k in [EqBandKind::Bypass, EqBandKind::Peak, EqBandKind::LowShelf, EqBandKind::HighShelf] {
                        if ui.selectable_value(&mut band.kind, k, k.label()).changed() { local = true; }
                    }
                    local
                });
            if resp.inner.unwrap_or(false) { changed = true; }
            let active = band.kind != EqBandKind::Bypass;
            ui.add_enabled_ui(active, |ui| {
                ui.label("Hz");
                if ui.add(egui::DragValue::new(&mut band.hz).speed(1.0).range(20.0..=20_000.0)).changed() { changed = true; }
                ui.label("Gain");
                if ui.add(egui::DragValue::new(&mut band.gain_db).speed(0.1).suffix(" dB").range(-24.0..=24.0)).changed() { changed = true; }
                ui.label("Q");
                if ui.add(egui::DragValue::new(&mut band.q).speed(0.05).range(0.1..=10.0)).changed() { changed = true; }
            });
        });
    }

    ui.add_space(8.0);
    ui.strong("De-esser");
    changed |= row(ui, "Enabled", |ui| ui.checkbox(&mut p.deess_enabled, "").changed());
    changed |= row(ui, "Frequency (Hz)", |ui| {
        ui.add(egui::DragValue::new(&mut p.deess_hz).speed(50.0).suffix(" Hz").range(2_000.0..=14_000.0)).changed()
    });
    changed |= row(ui, "Threshold (dB)", |ui| {
        ui.add(egui::DragValue::new(&mut p.deess_threshold_db).speed(0.5).suffix(" dB").range(-60.0..=0.0)).changed()
    });
    changed |= row(ui, "Ratio (x:1)", |ui| {
        ui.add(egui::DragValue::new(&mut p.deess_ratio).speed(0.1).range(1.0..=12.0)).changed()
    });

    ui.add_space(8.0);
    ui.strong("Noise gate");
    changed |= row(ui, "Enabled", |ui| ui.checkbox(&mut p.gate_enabled, "").changed());
    changed |= row(ui, "Threshold (dB)", |ui| {
        ui.add(egui::DragValue::new(&mut p.gate_threshold_db).speed(0.5).suffix(" dB").range(-80.0..=0.0)).changed()
    });
    changed |= row(ui, "Attack (ms)", |ui| {
        ui.add(egui::DragValue::new(&mut p.gate_attack_ms).speed(0.5).suffix(" ms").range(0.1..=200.0)).changed()
    });
    changed |= row(ui, "Release (ms)", |ui| {
        ui.add(egui::DragValue::new(&mut p.gate_release_ms).speed(1.0).suffix(" ms").range(1.0..=2000.0)).changed()
    });

    ui.add_space(8.0);
    ui.strong("Compressor");
    changed |= row(ui, "Enabled", |ui| ui.checkbox(&mut p.compressor_enabled, "").changed());
    changed |= row(ui, "Threshold (dB)", |ui| {
        ui.add(egui::DragValue::new(&mut p.compressor_threshold_db).speed(0.5).suffix(" dB").range(-60.0..=0.0)).changed()
    });
    changed |= row(ui, "Ratio (x:1)", |ui| {
        ui.add(egui::DragValue::new(&mut p.compressor_ratio).speed(0.1).range(1.0..=20.0)).changed()
    });
    changed |= row(ui, "Attack (ms)", |ui| {
        ui.add(egui::DragValue::new(&mut p.compressor_attack_ms).speed(0.5).suffix(" ms").range(0.1..=200.0)).changed()
    });
    changed |= row(ui, "Release (ms)", |ui| {
        ui.add(egui::DragValue::new(&mut p.compressor_release_ms).speed(1.0).suffix(" ms").range(1.0..=2000.0)).changed()
    });
    changed |= row(ui, "Makeup gain (dB)", |ui| {
        ui.add(egui::DragValue::new(&mut p.compressor_makeup_db).speed(0.1).suffix(" dB").range(-12.0..=24.0)).changed()
    });

    changed
}

fn row(ui: &mut egui::Ui, label: &str, contents: impl FnOnce(&mut egui::Ui) -> bool) -> bool {
    let mut c = false;
    ui.horizontal(|ui| {
        ui.add_sized([160.0, 20.0], egui::Label::new(label));
        c = contents(ui);
    });
    c
}
