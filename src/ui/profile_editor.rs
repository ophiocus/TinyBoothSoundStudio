//! Shared `Profile` editor body — used by both the **Admin** window
//! (which edits the recording-tone profiles list) and the per-track
//! **Correction** window (which edits one track's `correction` chain).
//!
//! Returns `true` if any field changed during the call. The caller owns
//! the framing (heading, save/disable buttons, surrounding ScrollArea
//! if it wants one); this module only renders the parameter rows.

use crate::dsp::{EqBandKind, Profile};
use eframe::egui;

/// Render every editable field on a `Profile`. Sections, in order:
///
/// 1. Identity (name + description) — shown by the caller via `meta_grid`
///    when it wants the meta row; pure-DSP callers can pass `false` to
///    skip the identity rows. (Both current callers want them, but
///    keeping it parameterised costs nothing.)
/// 2. Input gain (dB)
/// 3. High-pass filter
/// 4. Parametric EQ (4 bands)
/// 5. De-esser
/// 6. Noise gate
/// 7. Compressor
///
/// Returns `true` if any drag-value, checkbox, or combo changed in this
/// frame — caller uses the bit to mark project / profiles dirty.
pub fn render(p: &mut Profile, ui: &mut egui::Ui, show_identity: bool) -> bool {
    let mut changed = false;

    if show_identity {
        egui::Grid::new("profile_meta")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Name");
                if ui
                    .add(egui::TextEdit::singleline(&mut p.name).desired_width(260.0))
                    .changed()
                {
                    changed = true;
                }
                ui.end_row();
                ui.label("Description");
                if ui
                    .add(
                        egui::TextEdit::multiline(&mut p.description)
                            .desired_width(460.0)
                            .desired_rows(2),
                    )
                    .changed()
                {
                    changed = true;
                }
                ui.end_row();
            });
    }

    ui.add_space(10.0);
    ui.strong("Input");
    changed |= row(ui, "Input gain (dB)", |ui| {
        ui.add(
            egui::DragValue::new(&mut p.input_gain_db)
                .speed(0.1)
                .suffix(" dB")
                .range(-24.0..=24.0),
        )
        .changed()
    });

    ui.add_space(10.0);
    ui.strong("High-pass filter");
    changed |= row(ui, "Enabled", |ui| {
        ui.checkbox(&mut p.hpf_enabled, "").changed()
    });
    changed |= row(ui, "Cutoff (Hz)", |ui| {
        ui.add(
            egui::DragValue::new(&mut p.hpf_hz)
                .speed(1.0)
                .suffix(" Hz")
                .range(20.0..=1000.0),
        )
        .changed()
    });

    ui.add_space(10.0);
    ui.strong("Suno cleanup");
    ui.label(
        egui::RichText::new(
            "DC-offset trim (sub-audible 5 Hz HPF) and a top-octave low-pass \
             that suppresses the AI-shimmer artefacts common in Suno output.",
        )
        .italics()
        .weak(),
    );
    changed |= row(ui, "Remove DC offset", |ui| {
        ui.checkbox(&mut p.dc_remove_enabled, "").changed()
    });
    changed |= row(ui, "Nyquist clean", |ui| {
        ui.checkbox(&mut p.nyquist_clean_enabled, "").changed()
    });
    changed |= row(ui, "Nyquist cutoff (Hz)", |ui| {
        ui.add_enabled(
            p.nyquist_clean_enabled,
            egui::DragValue::new(&mut p.nyquist_clean_hz)
                .speed(50.0)
                .suffix(" Hz")
                .range(8_000.0..=20_000.0),
        )
        .changed()
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
            let mut kind_changed = false;
            egui::ComboBox::from_id_source(format!("profile_eq_kind_{i}"))
                .selected_text(band.kind.label())
                .width(110.0)
                .show_ui(ui, |ui| {
                    for k in [
                        EqBandKind::Bypass,
                        EqBandKind::Peak,
                        EqBandKind::LowShelf,
                        EqBandKind::HighShelf,
                    ] {
                        if ui.selectable_value(&mut band.kind, k, k.label()).changed() {
                            kind_changed = true;
                        }
                    }
                });
            if kind_changed {
                changed = true;
            }

            let active = band.kind != EqBandKind::Bypass;
            ui.add_enabled_ui(active, |ui| {
                ui.label("Hz");
                if ui
                    .add(
                        egui::DragValue::new(&mut band.hz)
                            .speed(1.0)
                            .range(20.0..=20_000.0),
                    )
                    .changed()
                {
                    changed = true;
                }
                ui.label("Gain");
                if ui
                    .add(
                        egui::DragValue::new(&mut band.gain_db)
                            .speed(0.1)
                            .suffix(" dB")
                            .range(-24.0..=24.0),
                    )
                    .changed()
                {
                    changed = true;
                }
                ui.label("Q");
                if ui
                    .add(
                        egui::DragValue::new(&mut band.q)
                            .speed(0.05)
                            .range(0.1..=10.0),
                    )
                    .changed()
                {
                    changed = true;
                }
            });
        });
    }

    ui.add_space(10.0);
    ui.strong("De-esser");
    changed |= row(ui, "Enabled", |ui| {
        ui.checkbox(&mut p.deess_enabled, "").changed()
    });
    changed |= row(ui, "Frequency (Hz)", |ui| {
        ui.add(
            egui::DragValue::new(&mut p.deess_hz)
                .speed(50.0)
                .suffix(" Hz")
                .range(2_000.0..=14_000.0),
        )
        .changed()
    });
    changed |= row(ui, "Threshold (dB)", |ui| {
        ui.add(
            egui::DragValue::new(&mut p.deess_threshold_db)
                .speed(0.5)
                .suffix(" dB")
                .range(-60.0..=0.0),
        )
        .changed()
    });
    changed |= row(ui, "Ratio (x:1)", |ui| {
        ui.add(
            egui::DragValue::new(&mut p.deess_ratio)
                .speed(0.1)
                .range(1.0..=12.0),
        )
        .changed()
    });

    ui.add_space(10.0);
    ui.strong("Noise gate");
    changed |= row(ui, "Enabled", |ui| {
        ui.checkbox(&mut p.gate_enabled, "").changed()
    });
    changed |= row(ui, "Threshold (dB)", |ui| {
        ui.add(
            egui::DragValue::new(&mut p.gate_threshold_db)
                .speed(0.5)
                .suffix(" dB")
                .range(-80.0..=0.0),
        )
        .changed()
    });
    changed |= row(ui, "Attack (ms)", |ui| {
        ui.add(
            egui::DragValue::new(&mut p.gate_attack_ms)
                .speed(0.5)
                .suffix(" ms")
                .range(0.1..=200.0),
        )
        .changed()
    });
    changed |= row(ui, "Release (ms)", |ui| {
        ui.add(
            egui::DragValue::new(&mut p.gate_release_ms)
                .speed(1.0)
                .suffix(" ms")
                .range(1.0..=2000.0),
        )
        .changed()
    });

    ui.add_space(10.0);
    ui.strong("Compressor");
    changed |= row(ui, "Enabled", |ui| {
        ui.checkbox(&mut p.compressor_enabled, "").changed()
    });
    changed |= row(ui, "Threshold (dB)", |ui| {
        ui.add(
            egui::DragValue::new(&mut p.compressor_threshold_db)
                .speed(0.5)
                .suffix(" dB")
                .range(-60.0..=0.0),
        )
        .changed()
    });
    changed |= row(ui, "Ratio (x:1)", |ui| {
        ui.add(
            egui::DragValue::new(&mut p.compressor_ratio)
                .speed(0.1)
                .range(1.0..=20.0),
        )
        .changed()
    });
    changed |= row(ui, "Attack (ms)", |ui| {
        ui.add(
            egui::DragValue::new(&mut p.compressor_attack_ms)
                .speed(0.5)
                .suffix(" ms")
                .range(0.1..=200.0),
        )
        .changed()
    });
    changed |= row(ui, "Release (ms)", |ui| {
        ui.add(
            egui::DragValue::new(&mut p.compressor_release_ms)
                .speed(1.0)
                .suffix(" ms")
                .range(1.0..=2000.0),
        )
        .changed()
    });
    changed |= row(ui, "Makeup gain (dB)", |ui| {
        ui.add(
            egui::DragValue::new(&mut p.compressor_makeup_db)
                .speed(0.1)
                .suffix(" dB")
                .range(-12.0..=24.0),
        )
        .changed()
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
