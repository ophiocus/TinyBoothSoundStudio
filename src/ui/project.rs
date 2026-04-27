use crate::app::TinyBoothApp;
use eframe::egui;

pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    ui.heading("Project");
    ui.separator();

    ui.horizontal(|ui| {
        ui.label("Name:");
        if ui
            .add(egui::TextEdit::singleline(&mut app.project.name).desired_width(320.0))
            .changed()
        {
            app.project_dirty = true;
        }
        if ui.button("Save").clicked() {
            app.save_project();
        }
        if ui.button("Choose folder…").clicked() {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                app.project.root = dir;
                app.project_dirty = true;
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Folder:");
        ui.monospace(app.project.root.display().to_string());
    });

    ui.add_space(6.0);
    ui.label(format!("Created: {}", app.project.created.format("%Y-%m-%d %H:%M UTC")));
    ui.separator();

    if app.project.tracks.is_empty() {
        ui.label("No tracks yet. Switch to the Record tab to capture one.");
        return;
    }

    // Track table.
    egui::Grid::new("tracks_grid")
        .num_columns(7)
        .striped(true)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.strong("");
            ui.strong("Name");
            ui.strong("Source");
            ui.strong("Rate");
            ui.strong("Gain (dB)");
            ui.strong("Duration");
            ui.strong("");
            ui.end_row();

            let mut to_delete: Option<usize> = None;
            for (idx, t) in app.project.tracks.iter_mut().enumerate() {
                if ui.checkbox(&mut t.mute, "").on_hover_text("mute").changed() {
                    app.project_dirty = true;
                }
                if ui
                    .add(egui::TextEdit::singleline(&mut t.name).desired_width(160.0))
                    .changed()
                {
                    app.project_dirty = true;
                }
                let (src, hover) = match &t.source {
                    crate::project::TrackSource::SunoStem { role, original_filename, session_epoch, session_ordinal, provenance } => {
                        let mut h = format!("Suno stem — {}\nfilename: {}", role.label(), original_filename);
                        if let Some(ord) = session_ordinal {
                            h.push_str(&format!("\nsession ordinal: {ord}"));
                        }
                        if let Some(epoch) = session_epoch {
                            let iso = chrono::DateTime::<chrono::Utc>::from_timestamp(*epoch, 0)
                                .map(|d| d.to_rfc3339())
                                .unwrap_or_else(|| epoch.to_string());
                            h.push_str(&format!("\nsession epoch: {epoch}\niso: {iso}"));
                        }
                        if let Some(p) = provenance.as_ref() {
                            h.push_str(&format!("\nprovenance: {p}"));
                        }
                        let label = match session_ordinal {
                            Some(o) => format!("Suno · {} (#{o})", role.label()),
                            None => format!("Suno · {}", role.label()),
                        };
                        (label, h)
                    }
                    crate::project::TrackSource::Recorded => {
                        let label = if t.stereo {
                            "stereo".to_string()
                        } else {
                            match t.channel_source {
                                Some(c) => format!("Ch {}", c + 1),
                                None => "mix".to_string(),
                            }
                        };
                        (label, "Recorded by TinyBooth".into())
                    }
                };
                ui.label(src).on_hover_text(hover);
                ui.label(format!("{} Hz", t.sample_rate));
                if ui
                    .add(egui::Slider::new(&mut t.gain_db, -24.0..=12.0).suffix(" dB"))
                    .changed()
                {
                    app.project_dirty = true;
                }
                ui.label(format!("{:.1}s", t.duration_secs));
                if ui.button("✖").on_hover_text("remove track").clicked() {
                    to_delete = Some(idx);
                }
                ui.end_row();
            }

            if let Some(i) = to_delete {
                let t = app.project.tracks.remove(i);
                let abs = app.project.root.join(&t.file);
                let _ = std::fs::remove_file(&abs);
                app.project_dirty = true;
            }
        });
}
