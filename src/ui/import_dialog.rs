//! Import-result modal — shown after every Suno import (success or
//! failure). Dismissible. Can't be missed; always announces something
//! happened, links to the per-import log file, and offers to open the
//! log folder.

use crate::app::TinyBoothApp;
use eframe::egui;

pub fn show(app: &mut TinyBoothApp, ctx: &egui::Context) {
    // Drain dialog state into a local because the closure needs &mut app
    // for actions like opening folders.
    let outcome = match app.import_dialog.as_ref() {
        Some(o) => o,
        None => return,
    };
    let success = outcome.success;
    let summary = outcome.summary.clone();
    let log_path = outcome.log_path.clone();
    let source = outcome.source.clone();

    let mut close = false;
    let mut open_log = false;
    let mut open_log_folder = false;
    let mut go_to_project_tab = false;

    let title = if success { "✅  Import complete" } else { "⚠  Import did not complete" };

    egui::Window::new(title)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .collapsible(false)
        .resizable(false)
        .min_width(520.0)
        .max_width(720.0)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(if success { "Done." } else { "Nothing was imported." })
                .color(if success {
                    egui::Color32::from_rgb(100, 220, 150)
                } else {
                    egui::Color32::from_rgb(230, 180, 100)
                })
                .strong());
            ui.add_space(6.0);
            ui.label(format!("Source: {source}"));
            ui.separator();
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                ui.label(summary);
            });
            ui.separator();
            ui.horizontal(|ui| {
                if success {
                    if ui.button("Go to Project tab").clicked() { go_to_project_tab = true; close = true; }
                }
                if ui.button("Open log").on_hover_text(log_path.display().to_string()).clicked() {
                    open_log = true;
                }
                if ui.button("Open log folder").clicked() {
                    open_log_folder = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() { close = true; }
                });
            });
        });

    if open_log {
        let _ = open::that(&log_path);
    }
    if open_log_folder {
        if let Some(dir) = log_path.parent() {
            let _ = open::that(dir);
        }
    }
    if go_to_project_tab {
        app.tab = crate::app::Tab::Project;
    }
    if close {
        app.import_dialog = None;
    }
}
