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
        // Project-level batch trim. Disabled when there's nothing to
        // trim (no tracks AND no bundled mixdown).
        let has_audio = !app.project.tracks.is_empty() || app.project.suno_mixdown_path.is_some();
        let resp = ui.add_enabled(has_audio, egui::Button::new("✂  Trim project…"));
        if resp
            .on_hover_text(
                "Crop every WAV in this project (stems + bundled Suno mixdown) to a \
                 shared time range. Destructive — overwrites the WAVs in place. \
                 Re-import the bundle to undo.",
            )
            .clicked()
        {
            app.show_trim = true;
        }
        if ui
            .button("📊  Project Health…")
            .on_hover_text(
                "Per-track telemetry summary, analyzer status, and metadata weight. \
                 Telemetry is the brightness / sustain / density / drum-event data \
                 baked at first save.",
            )
            .clicked()
        {
            app.show_health = true;
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Folder:");
        ui.monospace(app.project.root.display().to_string());
    });

    ui.add_space(6.0);
    ui.label(format!(
        "Created: {}",
        app.project.created.format("%Y-%m-%d %H:%M UTC")
    ));
    // Song-level key estimate (v0.4.14). Sums every guitar/bass
    // track's pitch-class histogram and runs Krumhansl-Schmuckler
    // over the union. Updates incrementally as telemetry results
    // land. Hidden when no melodic track has analyzed yet.
    if let Some(k) = app.project.song_key_estimate.as_ref() {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Estimated key:").strong());
            ui.label(egui::RichText::new(k.label()).color(egui::Color32::from_rgb(180, 220, 240)))
                .on_hover_text(format!(
                    "Krumhansl-Schmuckler over the summed pitch-class histograms \
                 of every guitar/bass track in this project.\n\
                 Confidence {:.2}. Runner-up: {} {} ({:.2}).",
                    k.confidence,
                    {
                        const NOTES: [&str; 12] = [
                            "C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B",
                        ];
                        NOTES[(k.second_choice_root as usize) % 12]
                    },
                    k.second_choice_mode.label(),
                    k.second_choice_confidence,
                ));
        });
    }
    ui.separator();

    if app.project.tracks.is_empty() {
        ui.label("No tracks yet. Switch to the Record tab to capture one.");
        return;
    }

    // Track table.
    egui::Grid::new("tracks_grid")
        .num_columns(8)
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
            ui.strong("");
            ui.end_row();

            let mut to_delete: Option<usize> = None;
            let mut to_swap: Option<usize> = None;
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
                    crate::project::TrackSource::SunoStem {
                        role,
                        original_filename,
                        session_epoch,
                        session_ordinal,
                        provenance,
                    } => {
                        let mut h = format!(
                            "Suno stem — {}\nfilename: {}",
                            role.label(),
                            original_filename
                        );
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
                if ui
                    .button("↔ Swap…")
                    .on_hover_text(
                        "Hot-load a different WAV into this track. The new \
                         audio replaces the file in-place; the track keeps \
                         its name, role, correction chain, automation, \
                         polarity, and telemetry profile. The project saves \
                         automatically; telemetry re-runs in the background. \
                         New WAV must match the project's sample rate.",
                    )
                    .clicked()
                {
                    to_swap = Some(idx);
                }
                if ui.button("✖").on_hover_text("remove track").clicked() {
                    to_delete = Some(idx);
                }
                ui.end_row();
            }

            if let Some(i) = to_delete {
                let t = app.project.tracks.remove(i);
                // Folder projects: unlink the sibling WAV. .tib projects:
                // the audio rows cascade away on the next save (the prune
                // step in tib_project::save_metadata). Critically, do NOT
                // fs::remove_file on a .tib project — `t.file` is empty
                // for .tib tracks, so root.join("") would point at the
                // .tib file itself and we'd delete the whole project.
                if !app.is_tib() {
                    let abs = app.project.root.join(&t.file);
                    let _ = std::fs::remove_file(&abs);
                }
                app.project_dirty = true;
            }
            if let Some(i) = to_swap {
                if let Some(src) = rfd::FileDialog::new()
                    .set_title("Pick a WAV to hot-load into this track")
                    .add_filter("WAV", &["wav"])
                    .pick_file()
                {
                    match app.hot_load_swap(i, &src) {
                        Ok(()) => {
                            app.status = Some(format!(
                                "Swapped: {} → track #{}",
                                src.file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| "(unnamed)".into()),
                                i + 1
                            ));
                        }
                        Err(e) => {
                            app.status = Some(format!("Swap failed: {e:#}"));
                        }
                    }
                }
            }
        });
}
