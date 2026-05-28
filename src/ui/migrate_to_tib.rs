//! Migrate-to-`.tib` modal (TBSS-FR-0007 phase 2c). Shown when the user
//! opens a legacy folder project (`*.tinybooth`). Offers to convert it to
//! the single-file `.tib` SQLite format and work in that, or to keep
//! working in the folder format.
//!
//! Migration is **additive** — the folder project is left on disk
//! untouched as a backup; the `.tib` is written as a sibling and becomes
//! the live project. Three outcomes: **Migrate** (convert + open the
//! `.tib`), **Open as folder** (open the folder project as-is), **Cancel**
//! (do nothing).

use crate::app::TinyBoothApp;
use eframe::egui;

pub fn show(app: &mut TinyBoothApp, ctx: &egui::Context) {
    let Some(pending) = app.pending_migration.as_ref() else {
        return;
    };
    let folder = pending.folder_manifest.display().to_string();
    let tib = pending.suggested_tib.display().to_string();

    let mut click_migrate = false;
    let mut click_folder = false;
    let mut click_cancel = false;

    egui::Window::new("Convert to single-file .tib?")
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .collapsible(false)
        .resizable(false)
        .min_width(560.0)
        .max_width(760.0)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(
                    "You opened a folder project. TinyBooth now uses a single-file \
                     .tib format — one SQLite file holding every stem, its revision \
                     history, and all console state.",
                )
                .strong(),
            );
            ui.add_space(8.0);

            egui::Grid::new("migrate_grid")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Folder project:");
                    ui.monospace(&folder);
                    ui.end_row();

                    ui.label("Will create:");
                    ui.monospace(&tib);
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.colored_label(
                egui::Color32::from_rgb(150, 200, 150),
                "Migration is additive — your folder project is left untouched as a \
                 backup. The .tib becomes the live project (atomic saves, reversible \
                 Trim, per-stem revision history).",
            );

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new("Migrate to .tib").min_size(egui::vec2(160.0, 30.0)))
                    .clicked()
                {
                    click_migrate = true;
                }
                if ui
                    .add(egui::Button::new("Open as folder").min_size(egui::vec2(160.0, 30.0)))
                    .clicked()
                {
                    click_folder = true;
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

    if click_migrate {
        app.resolve_migration(true);
    } else if click_folder {
        app.resolve_migration(false);
    } else if click_cancel {
        app.pending_migration = None;
    }
}
