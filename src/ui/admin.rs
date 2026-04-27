//! Admin window for viewing and editing recording-tone profile parameters.
//!
//! Every number on a `Profile` is exposed as a labelled drag-value (so you
//! can type a number directly or scrub with the mouse). Changes are in-memory
//! until you press Save — then they're written to `profiles.json` under
//! `%APPDATA%\TinyBooth Sound Studio\`.

use crate::app::TinyBoothApp;
use crate::dsp::Profile;
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
        // The Admin window doesn't track per-edit dirty state — Save all
        // is explicit. Discard the changed bit.
        let _ = crate::ui::profile_editor::render(p, ui, true);
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new(
                "Changes take effect for the next recording. \
                 Press Save all above to persist to disk.",
            )
            .italics()
            .weak(),
        );
    });
}
