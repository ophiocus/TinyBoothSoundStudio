use crate::app::TinyBoothApp;
use crate::audio;
use crate::project::{Project, TRACKS_DIR};
use crate::ui::viz;
use chrono::{DateTime, Local};
use eframe::egui;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Page size for the recordings-list view. Small enough to fit on
/// reasonable screen heights without scrolling, large enough to
/// avoid constant page flipping after a few takes.
const RECORDINGS_PAGE_SIZE: usize = 10;

pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    ui.heading("Record");
    ui.separator();

    // ── Recording tone ──────────────────────────────────────────────
    let recording = app.session.is_some();
    ui.horizontal(|ui| {
        ui.label("Recording tone:");
        let current_name = app.active_profile().name.clone();
        let combo = egui::ComboBox::from_id_source("profile_combo")
            .selected_text(current_name)
            .width(240.0);
        // Locked while recording — can't swap the chain mid-take.
        ui.add_enabled_ui(!recording, |ui| {
            combo.show_ui(ui, |ui| {
                for (i, p) in app.profiles.clone().iter().enumerate() {
                    if ui
                        .selectable_label(i == app.active_profile_idx, &p.name)
                        .on_hover_text(&p.description)
                        .clicked()
                    {
                        app.set_active_profile(i);
                    }
                }
            });
        });
        if ui
            .button("Admin…")
            .on_hover_text("Edit profile parameters")
            .clicked()
        {
            app.show_admin = true;
            app.admin_edit_idx = Some(app.active_profile_idx);
        }
        if recording {
            ui.colored_label(egui::Color32::LIGHT_YELLOW, "(locked while recording)");
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.weak(app.active_profile().description.clone());
    });
    ui.separator();

    // ── Device picker ───────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label("Input device:");
        let current = app
            .selected_device
            .clone()
            .unwrap_or_else(|| "— none —".into());
        egui::ComboBox::from_id_source("device_combo")
            .selected_text(current.clone())
            .width(320.0)
            .show_ui(ui, |ui| {
                for dev in &app.devices {
                    if ui
                        .selectable_label(
                            app.selected_device.as_deref() == Some(&dev.name),
                            format!(
                                "{}  ({} ch, {} Hz)",
                                dev.name, dev.channels, dev.sample_rate
                            ),
                        )
                        .clicked()
                    {
                        app.selected_device = Some(dev.name.clone());
                        // Reset source mode if it's no longer valid for the new device.
                        match app.selected_mode {
                            crate::audio::SourceMode::Channel(sel) if sel >= dev.channels => {
                                app.selected_mode = crate::audio::SourceMode::Mixdown;
                            }
                            crate::audio::SourceMode::Stereo if dev.channels < 2 => {
                                app.selected_mode = crate::audio::SourceMode::Mixdown;
                            }
                            _ => {}
                        }
                    }
                }
            });
        if ui.button("Refresh").clicked() {
            app.devices = audio::list_input_devices();
        }
    });

    // ── Source mode ─────────────────────────────────────────────────
    // Mixdown and Ch 1 are always offered. Ch 2+ appear for multi-ch devices.
    // Stereo is offered when the device has at least 2 input channels.
    let channel_count = app
        .selected_device
        .as_ref()
        .and_then(|n| app.devices.iter().find(|d| &d.name == n))
        .map(|d| d.channels)
        .unwrap_or(0);
    ui.horizontal_wrapped(|ui| {
        use crate::audio::SourceMode;
        ui.label("Source:");
        ui.radio_value(
            &mut app.selected_mode,
            SourceMode::Mixdown,
            "All (mixdown → mono)",
        );
        for c in 0..channel_count {
            ui.radio_value(
                &mut app.selected_mode,
                SourceMode::Channel(c),
                format!("Ch {} → mono", c + 1),
            );
        }
        if channel_count >= 2 {
            ui.radio_value(
                &mut app.selected_mode,
                SourceMode::Stereo,
                "Stereo (Ch 1 + Ch 2 → L/R)",
            );
        }
    });

    // ── Track naming ────────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label("New track name:");
        ui.add(
            egui::TextEdit::singleline(&mut app.pending_track_name)
                .desired_width(260.0)
                .hint_text("(auto: track-001, track-002, …)"),
        );
    });

    // ── Transport ──────────────────────────────────────────────────
    ui.horizontal(|ui| {
        let recording = app.session.is_some();
        if !recording {
            let enabled = app.selected_device.is_some();
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::new("⏺  Record").min_size(egui::vec2(120.0, 32.0)),
                )
                .clicked()
            {
                if let Err(e) = app.start_new_take() {
                    app.status = Some(format!("record error: {e}"));
                }
            }
        } else if ui
            .add(egui::Button::new("⏹  Stop").min_size(egui::vec2(120.0, 32.0)))
            .clicked()
        {
            app.stop_take();
        }
        if let Some(sess) = app.session.as_ref() {
            ui.label(format!("REC  {:.1}s", sess.duration_secs()));
            ui.label(format!(
                "file: {}",
                sess.wav_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ));
        }
    });

    ui.add_space(8.0);

    // ── Visualisation ───────────────────────────────────────────────
    let sample_rate = app
        .viz
        .sample_rate
        .load(std::sync::atomic::Ordering::Relaxed) as usize;
    let window = sample_rate * 2; // 2 seconds
    let left = app.viz.snapshot_left(window);
    let stereo = app.viz.is_stereo();

    if stereo {
        let right = app.viz.snapshot_right(window);
        ui.label("Waveform — L (last 2 seconds)");
        viz::draw_waveform(ui, &left, 80.0);
        ui.add_space(2.0);
        ui.label("Waveform — R");
        viz::draw_waveform(ui, &right, 80.0);
        ui.add_space(6.0);
        ui.label("Spectrum (L+R sum)");
        // Sum L+R for the spectrum — overlapping stereo spectra are visually noisy.
        let sum: Vec<f32> = left
            .iter()
            .zip(right.iter())
            .map(|(l, r)| 0.5 * (l + r))
            .collect();
        viz::draw_spectrum(ui, &sum, 140.0);
        ui.add_space(6.0);
        let pl = app.viz.peak_left();
        let pr = app.viz.peak_right();
        ui.label(format!("Input level — L {:.2}   R {:.2}", pl, pr));
        viz::draw_meter(ui, pl);
        ui.add_space(2.0);
        viz::draw_meter(ui, pr);
    } else {
        ui.label("Waveform (last 2 seconds)");
        viz::draw_waveform(ui, &left, 140.0);
        ui.add_space(6.0);
        ui.label("Spectrum");
        viz::draw_spectrum(ui, &left, 140.0);
        ui.add_space(6.0);
        let p = app.viz.peak_left();
        ui.label(format!("Input level — peak {:.2}", p));
        viz::draw_meter(ui, p);
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label("Each take saves to");
        let recordings_dir = crate::config::Config::recordings_root().map(|p| p.join("tracks"));
        let path_str = recordings_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(no platform config dir)".into());
        ui.monospace(&path_str);
        // TBSS-FR-0008 item (2): path-label affordances. Both buttons
        // are no-ops without a resolvable recordings dir.
        if let Some(dir) = recordings_dir.as_ref() {
            if ui
                .small_button("📋")
                .on_hover_text("Copy path to clipboard")
                .clicked()
            {
                ui.ctx().output_mut(|o| o.copied_text = path_str.clone());
            }
            if ui
                .small_button("📂")
                .on_hover_text("Open in Explorer")
                .clicked()
            {
                // Make sure the dir exists so Explorer doesn't pop a
                // "Location not available" dialog on first run.
                let _ = std::fs::create_dir_all(dir);
                let _ = std::process::Command::new("explorer").arg(dir).spawn();
            }
        }
    });

    ui.add_space(10.0);
    ui.separator();
    show_recordings_list(app, ui);
}

/// "Recent recordings" — paged list of every take in the persistent
/// recordings filespace, newest first. Each entry has play / delete
/// actions; ▶ swaps the active project to the recordings project,
/// switches to the Mix tab, solos that take, and starts playback.
fn show_recordings_list(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    // Load fresh from disk on every Record-tab frame. The recordings
    // manifest is small (JSON only — WAV samples are not loaded by
    // Project::load), so this costs microseconds and avoids any
    // cache-staleness bugs around external edits / deletions.
    let rec = match Project::open_or_create_recordings() {
        Ok(p) => p,
        Err(e) => {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                format!("could not open recordings filespace: {e:#}"),
            );
            return;
        }
    };

    let total = rec.tracks.len();
    let total_pages = total.div_ceil(RECORDINGS_PAGE_SIZE).max(1);
    if app.recordings_page >= total_pages {
        app.recordings_page = total_pages - 1;
    }

    // Header row: title + count + page nav.
    ui.horizontal(|ui| {
        ui.heading(format!("Recordings ({total})"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if total_pages > 1 {
                ui.add_enabled_ui(app.recordings_page + 1 < total_pages, |ui| {
                    if ui.button("Next ▶").clicked() {
                        app.recordings_page += 1;
                    }
                });
                ui.label(format!(
                    "page {} / {}",
                    app.recordings_page + 1,
                    total_pages
                ));
                ui.add_enabled_ui(app.recordings_page > 0, |ui| {
                    if ui.button("◀ Prev").clicked() {
                        app.recordings_page -= 1;
                    }
                });
            }
        });
    });

    if total == 0 {
        ui.label(
            egui::RichText::new("No recordings yet — hit ⏺ above to capture one.")
                .italics()
                .weak(),
        );
        return;
    }

    // Newest first: walk the project's tracks in reverse (track-NNN
    // ids are minted ascending, so reverse iteration is newest-first).
    // Pagination: skip and take across the reversed sequence.
    let entries: Vec<(usize, &crate::project::Track)> =
        rec.tracks.iter().enumerate().rev().collect();
    let start = app.recordings_page * RECORDINGS_PAGE_SIZE;
    let end = (start + RECORDINGS_PAGE_SIZE).min(entries.len());
    let slice = &entries[start..end];

    let mut click_play_idx: Option<usize> = None;
    let mut click_delete_idx: Option<usize> = None;

    egui::Grid::new("recordings_list_grid")
        .num_columns(6)
        .striped(true)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.strong(""); // play
            ui.strong("Name");
            ui.strong("Duration");
            ui.strong("Mode");
            ui.strong("Profile");
            ui.strong(""); // delete
            ui.end_row();

            for (idx, t) in slice {
                if ui
                    .button("▶")
                    .on_hover_text("Play in mixer (switches to Mix tab)")
                    .clicked()
                {
                    click_play_idx = Some(*idx);
                }
                ui.label(&t.name).on_hover_text(&t.file);
                ui.label(format!("{:.1}s", t.duration_secs));
                let mode = if t.stereo {
                    "stereo".to_string()
                } else {
                    match t.channel_source {
                        Some(c) => format!("Ch {}", c + 1),
                        None => "mix".to_string(),
                    }
                };
                ui.label(mode);
                let prof = t.profile.as_ref().map(|p| p.name.as_str()).unwrap_or("—");
                ui.label(prof);
                if ui
                    .button("🗑")
                    .on_hover_text("Delete this take (removes the WAV)")
                    .clicked()
                {
                    click_delete_idx = Some(*idx);
                }
                ui.end_row();
            }
        });

    // Apply clicks AFTER the closure so we don't double-borrow `app`.
    if let Some(i) = click_play_idx {
        app.play_recording_in_mixer(i);
    }
    if let Some(i) = click_delete_idx {
        app.delete_recording(i);
    }

    // TBSS-FR-0008 item (1): list every loose WAV in tracks/ that's
    // not covered by the manifest. Lets the user see files dropped
    // in manually (or carried from another machine) instead of them
    // being invisible. Adoption / play / delete actions are deferred
    // to the full FR-0008 implementation; for now this is a
    // read-only view + reveal-in-Explorer per file.
    show_loose_wavs(&rec, ui);
}

/// Render the "Loose WAVs (not in manifest)" group — every `*.wav` in
/// the recordings filespace's `tracks/` directory whose basename is
/// not referenced by a manifest track. `.swap-tmp` debris from
/// interrupted writes is filtered out.
fn show_loose_wavs(rec: &Project, ui: &mut egui::Ui) {
    let manifested: HashSet<String> = rec
        .tracks
        .iter()
        .filter_map(|t| {
            Path::new(&t.file)
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_ascii_lowercase())
        })
        .collect();

    let tracks_dir = rec.root.join(TRACKS_DIR);
    let mut loose: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let entries = match std::fs::read_dir(&tracks_dir) {
        Ok(it) => it,
        Err(_) => return, // dir absent on first run — nothing to list
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".wav") {
            continue;
        }
        // In-flight crop/swap debris — never list these.
        if lower.ends_with(".swap-tmp") || lower.contains(".tmp") {
            continue;
        }
        if manifested.contains(&lower) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        loose.push((path, meta.len(), mtime));
    }
    if loose.is_empty() {
        return;
    }
    loose.sort_by(|a, b| b.2.cmp(&a.2)); // newest first

    ui.add_space(10.0);
    ui.separator();
    ui.heading(format!("Loose WAVs (not in manifest) ({})", loose.len()));
    ui.label(
        egui::RichText::new(
            "Files in the recordings folder that aren't tracked in the manifest \
             — drops, carry-overs, leftovers. Reveal in Explorer to act on them.",
        )
        .italics()
        .weak(),
    );

    egui::Grid::new("loose_wavs_grid")
        .num_columns(4)
        .striped(true)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.strong("File");
            ui.strong("Size");
            ui.strong("Modified");
            ui.strong("");
            ui.end_row();

            for (path, size, mtime) in &loose {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("(unnamed)");
                ui.monospace(name);
                ui.label(human_bytes(*size));
                ui.label(human_mtime(*mtime));
                if ui
                    .small_button("📂")
                    .on_hover_text("Reveal in Explorer")
                    .clicked()
                {
                    // /select, asks Explorer to open the parent and
                    // highlight the file. Best-effort; ignore failures.
                    let _ = std::process::Command::new("explorer")
                        .arg(format!("/select,{}", path.display()))
                        .spawn();
                }
                ui.end_row();
            }
        });
}

/// Compact byte-count for the Loose WAVs size column.
fn human_bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    let n = n as f64;
    if n >= MIB {
        format!("{:.1} MiB", n / MIB)
    } else if n >= KIB {
        format!("{:.0} KiB", n / KIB)
    } else {
        format!("{n:.0} B")
    }
}

/// Local-timezone timestamp for the Loose WAVs modified column.
fn human_mtime(t: std::time::SystemTime) -> String {
    DateTime::<Local>::from(t)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}
