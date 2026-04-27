use crate::app::TinyBoothApp;
use crate::export::{self, ExportFormat, ExportOptions};
use eframe::egui;

pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    ui.heading("Export");
    ui.separator();

    if app.project.tracks.is_empty() {
        ui.label("Record at least one track before exporting.");
        return;
    }

    // ── Format picker ───────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label("Format:");
        egui::ComboBox::from_id_source("fmt_combo")
            .selected_text(app.export_format.label())
            .show_ui(ui, |ui| {
                for f in ExportFormat::all() {
                    let enabled = !f.needs_ffmpeg() || app.ffmpeg_available;
                    ui.add_enabled_ui(enabled, |ui| {
                        let txt = if enabled {
                            f.label().to_string()
                        } else {
                            format!("{} (ffmpeg missing)", f.label())
                        };
                        if ui.selectable_label(app.export_format == f, txt).clicked() {
                            app.export_format = f;
                        }
                    });
                }
            });
    });

    if !app.ffmpeg_available {
        ui.colored_label(
            egui::Color32::from_rgb(230, 200, 80),
            "ffmpeg not found — only WAV export is available. Drop ffmpeg.exe next to the app or onto PATH.",
        );
    }

    // ── Bitrate slider for lossy codecs ─────────────────────────────
    match app.export_format {
        ExportFormat::Mp3
        | ExportFormat::OggVorbis
        | ExportFormat::OggOpus
        | ExportFormat::M4aAac => {
            ui.horizontal(|ui| {
                ui.label("Bitrate:");
                ui.add(egui::Slider::new(&mut app.export_bitrate, 64..=320).suffix(" kbps"));
            });
        }
        _ => {}
    }

    ui.add_space(6.0);
    ui.label(format!(
        "Mixing {} unmuted track(s) into a single mono file.",
        app.project.tracks.iter().filter(|t| !t.mute).count()
    ));

    ui.add_space(10.0);
    if ui
        .add_enabled(
            !app.export_busy,
            egui::Button::new("Export…").min_size(egui::vec2(160.0, 32.0)),
        )
        .clicked()
    {
        let default_name = format!(
            "{}.{}",
            sanitise_filename(&app.project.name),
            app.export_format.extension(),
        );
        if let Some(out) = rfd::FileDialog::new()
            .set_file_name(&default_name)
            .add_filter(app.export_format.label(), &[app.export_format.extension()])
            .save_file()
        {
            app.export_busy = true;
            let opts = ExportOptions {
                format: app.export_format,
                bitrate_kbps: app.export_bitrate,
                out_path: out,
            };
            match export::export(&app.project, &opts) {
                Ok(()) => {
                    app.export_msg = Some(format!("Exported: {}", opts.out_path.display()));
                    app.status = app.export_msg.clone();
                }
                Err(e) => {
                    app.export_msg = Some(format!("Export failed: {e}"));
                    app.status = app.export_msg.clone();
                }
            }
            app.export_busy = false;
        }
    }

    if let Some(m) = app.export_msg.as_ref() {
        ui.add_space(6.0);
        ui.label(m);
    }
}

fn sanitise_filename(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out.push_str("export");
    }
    out
}
