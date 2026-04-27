//! Pre-import conflict modal — appears when the proposed project
//! folder already contains a TinyBooth project whose Suno session
//! epoch matches the bundle the user just picked.
//!
//! Two outcomes: **Replace** wipes the existing project's tracks/
//! and manifest then re-imports fresh; **Cancel** drops the pending
//! import and leaves everything as-is.

use crate::app::TinyBoothApp;
use chrono::{DateTime, Utc};
use eframe::egui;

pub fn show(app: &mut TinyBoothApp, ctx: &egui::Context) {
    let Some(pending) = app.import_conflict.as_ref() else {
        return;
    };
    let probe = pending.probe.clone();
    let source = pending.source.display().to_string();
    let project_root = pending.project_root.display().to_string();
    let existing_name = probe
        .existing_project_name
        .clone()
        .unwrap_or_else(|| "(unknown)".into());

    let mut click_replace = false;
    let mut click_cancel = false;

    egui::Window::new("⚠  Suno session already imported")
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .collapsible(false)
        .resizable(false)
        .min_width(560.0)
        .max_width(760.0)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(
                "The bundle you picked is from the same Suno render as a project already on disk."
            ).strong());
            ui.add_space(8.0);

            egui::Grid::new("conflict_grid")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Picked source:");
                    ui.monospace(&source);
                    ui.end_row();

                    ui.label("Target project root:");
                    ui.monospace(&project_root);
                    ui.end_row();

                    ui.label("Existing project name:");
                    ui.label(&existing_name);
                    ui.end_row();

                    ui.label("Existing track count:");
                    ui.label(format!("{}", probe.existing_track_count));
                    ui.end_row();

                    if let Some(ord) = probe.existing_session_ordinal {
                        ui.label("Existing import ordinal:");
                        ui.label(format!("{ord}"));
                        ui.end_row();
                    }

                    if let Some(epoch) = probe.existing_session_epoch {
                        let iso = DateTime::<Utc>::from_timestamp(epoch, 0)
                            .map(|d| d.to_rfc3339())
                            .unwrap_or_else(|| epoch.to_string());
                        ui.label("Suno session epoch:");
                        ui.monospace(format!("{epoch}  ({iso})"));
                        ui.end_row();
                    }

                    if let Some(iso) = probe.new_session_iso.as_ref() {
                        ui.label("Picked bundle session:");
                        ui.monospace(iso);
                        ui.end_row();
                    }
                });

            ui.add_space(10.0);
            ui.colored_label(
                egui::Color32::from_rgb(230, 180, 100),
                "Replace will delete all tracks/, manifest, automation and corrections \
                 in the existing project, then re-import the bundle fresh. \
                 If you have edits worth keeping, click Cancel and rename the existing \
                 folder before re-importing.",
            );

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new("Replace existing project")
                            .min_size(egui::vec2(200.0, 30.0)),
                    )
                    .clicked()
                {
                    click_replace = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new("Cancel").min_size(egui::vec2(120.0, 30.0)))
                        .clicked()
                    {
                        click_cancel = true;
                    }
                });
            });
        });

    if click_replace {
        app.resolve_import_conflict(true);
    }
    if click_cancel {
        app.resolve_import_conflict(false);
    }
}
