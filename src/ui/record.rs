use crate::app::TinyBoothApp;
use crate::audio;
use crate::project::{Project, TRACKS_DIR};
use crate::ui::viz;
use chrono::{DateTime, Local};
use eframe::egui;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Fixed bin count for cached thumbnail peak tables. ~200 px wide
/// thumbnails sample this at 1:1; coarser display widths down-sample.
/// Independent of WAV length, so the cache key needs only the path.
const THUMB_BINS: usize = 200;
/// Rendered thumbnail size in the recordings list, in logical px.
const THUMB_W: f32 = 140.0;
const THUMB_H: f32 = 28.0;

/// Page size for the recordings-list view. Small enough to fit on
/// reasonable screen heights without scrolling, large enough to
/// avoid constant page flipping after a few takes.
const RECORDINGS_PAGE_SIZE: usize = 10;

pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    poll_recording_preview(app, ui.ctx());
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
                match app.start_new_take() {
                    Ok(()) => app.record_last_error = None,
                    Err(e) => {
                        // Both surfaces: the global bar for consistency,
                        // and the in-tab line below because a failure
                        // reported only below the fold reads as
                        // "nothing happened".
                        app.status = Some(format!("record error: {e:#}"));
                        app.record_last_error = Some(format!("{e:#}"));
                    }
                }
            }
        } else if ui
            .add(egui::Button::new("⏹  Stop").min_size(egui::vec2(120.0, 32.0)))
            .clicked()
        {
            app.stop_take();
        }
        // Live monitor (TBSS-FR-0015): base levels through the take's
        // exact signal path, before committing a recording.
        if !recording {
            let monitoring = app.monitor.is_some();
            let label = if monitoring {
                "🎙 Monitoring…"
            } else {
                "🎙 Monitor"
            };
            let resp = ui
                .add_enabled(
                    app.selected_device.is_some(),
                    egui::Button::new(label).min_size(egui::vec2(110.0, 32.0)),
                )
                .on_hover_text(
                    "Open the input live — waveform, spectrum and meters run \
                     through the recording tone chain without writing anything. \
                     Click again to stop.",
                );
            if resp.clicked() {
                if monitoring {
                    app.monitor = None;
                } else if let Some(dev) = app.selected_device.clone() {
                    match crate::audio::start_recording(
                        &dev,
                        app.selected_mode,
                        None, // monitor: no WAV
                        app.viz.clone(),
                        app.active_profile().clone(),
                        app.audio_err_tx.clone(),
                        None,
                    ) {
                        Ok(m) => {
                            app.monitor = Some(m);
                            app.record_last_error = None;
                        }
                        Err(e) => {
                            app.record_last_error = Some(format!("{e:#}"));
                        }
                    }
                }
            }
            if monitoring {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(50));
            }
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
    if let Some(err) = app.record_last_error.clone() {
        ui.colored_label(
            egui::Color32::from_rgb(0xE0, 0x60, 0x60),
            format!("⚠ couldn't start recording: {err}"),
        );
    }

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

    let mut click_play_path: Option<PathBuf> = None;
    let mut click_stop_preview = false;
    let mut click_integrate_idx: Option<usize> = None;
    let mut click_delete_idx: Option<usize> = None;

    egui::Grid::new("recordings_list_grid")
        .num_columns(9)
        .striped(true)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.strong(""); // play
            ui.strong("Name");
            ui.strong(""); // waveform
            ui.strong(""); // export selection
            ui.strong("Duration");
            ui.strong("Mode");
            ui.strong("Profile");
            ui.strong(""); // integrate
            ui.strong(""); // delete
            ui.end_row();

            for (idx, t) in slice {
                let abs_path = rec.root.join(&t.file);
                // ▶/■ toggle: takes audition in place, right in the
                // listing — the Mix-tab detour is retired (v0.4.81).
                let playing_this = app
                    .recording_preview
                    .as_ref()
                    .is_some_and(|p| p.path == abs_path);
                let pending_this = app
                    .recording_preview_pending
                    .as_ref()
                    .is_some_and(|(p, _)| *p == abs_path);
                if pending_this {
                    ui.add_enabled(false, egui::Button::new("…"))
                        .on_disabled_hover_text("decoding…");
                } else if playing_this {
                    if ui.button("⏹").on_hover_text("Stop").clicked() {
                        click_stop_preview = true;
                    }
                } else if ui
                    .button("▶")
                    .on_hover_text("Play this take (silences everything else)")
                    .clicked()
                {
                    click_play_path = Some(abs_path.clone());
                }
                ui.label(&t.name).on_hover_text(&t.file);
                let thumb = cached_or_compute_thumb(app, &abs_path);
                let selection = app.recordings_selection.get(&abs_path).copied();
                let playhead = if playing_this {
                    app.recording_preview
                        .as_ref()
                        .map(|p| p.session.position_frac())
                } else {
                    None
                };
                let response = draw_thumbnail(ui, thumb.as_ref(), selection, playhead);
                if playing_this {
                    // While playing, the waveform is a transport: click or
                    // drag scrubs the playhead. (Region selection applies
                    // when the take is not playing.)
                    if response.clicked() || response.dragged() {
                        if let (Some(pos), Some(p)) =
                            (response.interact_pointer_pos(), app.recording_preview.as_ref())
                        {
                            let frac =
                                (pos.x - response.rect.left()) / response.rect.width().max(1.0);
                            p.session.seek_frac(frac);
                        }
                    }
                } else if let Some(t) = thumb.as_ref() {
                    update_selection_from_response(app, &abs_path, &response, t.duration_secs);
                }
                export_selection_button(app, &abs_path, ui);
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
                let can_integrate =
                    !matches!(app.project.kind, crate::project::ProjectKind::Recordings);
                if ui
                    .add_enabled(can_integrate, egui::Button::new("⇪"))
                    .on_hover_text(format!(
                        "Integrate into '{}' as a track — clean it with the full                          correction toolset there",
                        app.project.name
                    ))
                    .on_disabled_hover_text("Open or create a project first")
                    .clicked()
                {
                    click_integrate_idx = Some(*idx);
                }
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
    if click_stop_preview {
        app.recording_preview = None;
    }
    if let Some(path) = click_play_path {
        start_recording_preview(app, path);
    }
    if let Some(i) = click_integrate_idx {
        app.integrate_recording_into_project(i);
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
    show_loose_wavs(app, &rec, ui);
}

/// Render the "Loose WAVs (not in manifest)" group — every `*.wav` in
/// the recordings filespace's `tracks/` directory whose basename is
/// not referenced by a manifest track. `.swap-tmp` debris from
/// interrupted writes is filtered out.
fn show_loose_wavs(app: &mut TinyBoothApp, rec: &Project, ui: &mut egui::Ui) {
    let mut loose_play: Option<PathBuf> = None;
    let mut loose_stop = false;
    let mut loose_integrate: Option<PathBuf> = None;
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
    loose.sort_by_key(|t| std::cmp::Reverse(t.2)); // newest first

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
        .num_columns(6)
        .striped(true)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.strong("File");
            ui.strong(""); // waveform
            ui.strong(""); // export selection
            ui.strong("Size");
            ui.strong("Modified");
            ui.strong("");
            ui.end_row();

            for (path, size, mtime) in &loose {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("(unnamed)");
                // ▶/■ parity with manifest rows (FR-0008's last item):
                // the preview engine is path-based, so loose WAVs play
                // and scrub exactly like takes.
                let playing_this = app
                    .recording_preview
                    .as_ref()
                    .is_some_and(|p| p.path == **path);
                let pending_this = app
                    .recording_preview_pending
                    .as_ref()
                    .is_some_and(|(p, _)| p == path);
                if pending_this {
                    ui.add_enabled(false, egui::Button::new("…"));
                } else if playing_this {
                    if ui.small_button("⏹").clicked() {
                        loose_stop = true;
                    }
                } else if ui
                    .small_button("▶")
                    .on_hover_text("Play (silences everything else)")
                    .clicked()
                {
                    loose_play = Some((*path).clone());
                }
                ui.monospace(name);
                let thumb = cached_or_compute_thumb(app, path);
                let selection = app.recordings_selection.get(path).copied();
                let playhead = if playing_this {
                    app.recording_preview
                        .as_ref()
                        .map(|p| p.session.position_frac())
                } else {
                    None
                };
                let response = draw_thumbnail(ui, thumb.as_ref(), selection, playhead);
                if playing_this {
                    if response.clicked() || response.dragged() {
                        if let (Some(pos), Some(p)) = (
                            response.interact_pointer_pos(),
                            app.recording_preview.as_ref(),
                        ) {
                            let frac =
                                (pos.x - response.rect.left()) / response.rect.width().max(1.0);
                            p.session.seek_frac(frac);
                        }
                    }
                } else if let Some(t) = thumb.as_ref() {
                    update_selection_from_response(app, path, &response, t.duration_secs);
                }
                export_selection_button(app, path, ui);
                let can_integrate =
                    !matches!(app.project.kind, crate::project::ProjectKind::Recordings);
                if ui
                    .add_enabled(can_integrate, egui::Button::new("⇪").small())
                    .on_hover_text(format!("Integrate into '{}' as a track", app.project.name))
                    .on_disabled_hover_text("Open or create a project first")
                    .clicked()
                {
                    loose_integrate = Some((*path).clone());
                }
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

    if loose_stop {
        app.recording_preview = None;
    }
    if let Some(p) = loose_play {
        start_recording_preview(app, p);
    }
    if let Some(p) = loose_integrate {
        app.integrate_wav_into_project(&p);
    }
}

/// Cached thumbnail data per recording — peaks for rendering + the
/// WAV's total duration so click-drag pixel-x can be converted to
/// selection-seconds. Stored on `TinyBoothApp.recordings_peaks_cache`
/// behind an `Arc` for cheap per-frame cloning.
pub struct CachedThumb {
    pub peaks: Vec<f32>,
    pub duration_secs: f32,
    /// (file size, mtime) at compute time. The cache is path-keyed with no
    /// eviction, and this list renders every frame — including *during* a
    /// recording — so a thumb computed from the not-yet-finalised WAV
    /// (hound reads its data-chunk length as 0 until the writer closes)
    /// used to cache a permanently blank waveform for every fresh take.
    /// Stamp mismatch on lookup forces a recompute once the file settles.
    pub stamp: Option<(u64, std::time::SystemTime)>,
}

/// Cheap identity stamp for cache invalidation.
fn file_stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}

/// Get the cached thumbnail for `path`, decoding the WAV on the UI
/// thread on first miss. Returns `None` only if the file can't be
/// opened/parsed (corrupt header, unsupported format). Cache entries
/// are never evicted within a session — peaks + a float per take is
/// ~1 KB, negligible. **TBSS-FR-0008 item (4)** — sync UI-thread
/// decode is the MVP trade-off; an async worker would only matter
/// for very long takes.
/// Kick off an in-listing preview of one take: silence everything else,
/// then decode on a background thread (takes can be minutes long — no
/// UI-thread decode) and start the session when the poll sees the result.
fn start_recording_preview(app: &mut TinyBoothApp, path: PathBuf) {
    app.stop_all_playback();
    let (tx, rx) = std::sync::mpsc::channel();
    let job_path = path.clone();
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<(Vec<f32>, u32)> {
            let reader = hound::WavReader::open(&job_path)?;
            let (spec, samples, frames) = crate::audiodecode::decode_wav_i16(reader)?;
            let stereo = crate::audiodecode::wav_i16_to_stereo_f32(
                &samples,
                spec.channels.max(1) as usize,
                frames as usize,
            );
            Ok((stereo, spec.sample_rate))
        })()
        .map_err(|e| format!("{e:#}"));
        let _ = tx.send(result);
    });
    app.recording_preview_pending = Some((path, rx));
}

/// Poll the pending decode; when ready, start playback. Called once per
/// Record-tab frame. Also clears a finished preview so the row's button
/// flips back to ▶ on its own.
fn poll_recording_preview(app: &mut TinyBoothApp, ctx: &egui::Context) {
    if let Some((path, rx)) = app.recording_preview_pending.as_ref() {
        match rx.try_recv() {
            Ok(Ok((stereo, rate))) => {
                let path = path.clone();
                app.recording_preview_pending = None;
                match crate::crossfade_player::CrossfadePreviewSession::play(stereo, rate, 2, 0) {
                    Ok(session) => {
                        app.recording_preview =
                            Some(crate::app::RecordingPreview { path, session });
                    }
                    Err(e) => app.status = Some(format!("preview failed: {e:#}")),
                }
            }
            Ok(Err(e)) => {
                app.recording_preview_pending = None;
                app.status = Some(format!("preview decode failed: {e}"));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(80));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                app.recording_preview_pending = None;
                app.status = Some("preview decode thread died".into());
            }
        }
    }
    if let Some(p) = app.recording_preview.as_ref() {
        if p.session.is_finished() {
            app.recording_preview = None;
        } else {
            // Keep the playhead moving without mouse motion.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}

fn cached_or_compute_thumb(app: &mut TinyBoothApp, path: &Path) -> Option<Arc<CachedThumb>> {
    // Never thumb the file that's being recorded right now: its header is
    // not finalised, so it decodes as empty — and it changes every buffer
    // anyway. The row renders as a blank placeholder until Stop.
    if let Some(sess) = app.session.as_ref() {
        if sess.wav_path == path {
            return None;
        }
    }
    let stamp = file_stamp(path);
    if let Some(cached) = app.recordings_peaks_cache.get(path) {
        // A stamp match means the file hasn't changed since we thumbed it.
        // Mismatch (the take finished being written, an external tool
        // replaced it) or a stat failure falls through to recompute.
        if cached.stamp.is_some() && cached.stamp == stamp {
            return Some(cached.clone());
        }
    }
    let mut thumb = compute_wav_thumb(path)?;
    thumb.stamp = stamp;
    let arc = Arc::new(thumb);
    app.recordings_peaks_cache
        .insert(path.to_path_buf(), arc.clone());
    Some(arc)
}

/// Decode `path` and produce its peak vector (fixed `THUMB_BINS` bins)
/// plus duration in seconds. Tolerates 16/24/32-bit int and float;
/// unsupported / corrupt files return `None` so the row falls back to
/// the placeholder.
fn compute_wav_thumb(path: &Path) -> Option<CachedThumb> {
    let reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let total_frames = reader.duration() as usize;
    let duration_secs = total_frames as f32 / spec.sample_rate.max(1) as f32;
    if total_frames == 0 {
        return Some(CachedThumb {
            peaks: vec![0.0; THUMB_BINS],
            duration_secs,
            stamp: None,
        });
    }

    let (_, samples, _) = crate::audiodecode::decode_wav_i16(reader).ok()?;

    let frames = samples.len() / channels;
    let frames_per_bin = frames.div_ceil(THUMB_BINS).max(1);
    let denom = i16::MAX as f32;
    let mut peaks = Vec::with_capacity(THUMB_BINS);
    for b in 0..THUMB_BINS {
        let f0 = b * frames_per_bin;
        let f1 = ((b + 1) * frames_per_bin).min(frames);
        let mut peak = 0.0f32;
        for f in f0..f1 {
            for c in 0..channels {
                let s = samples[f * channels + c] as f32 / denom;
                let a = s.abs();
                if a > peak {
                    peak = a;
                }
            }
        }
        peaks.push(peak);
    }
    Some(CachedThumb {
        peaks,
        duration_secs,
        stamp: None,
    })
}

/// Render the thumbnail (`THUMB_W × THUMB_H`) plus, if present, a
/// translucent overlay for the click-drag selection. Returns the
/// `Response` so the caller can drive selection state from drag events.
/// Selection is `(start_secs, end_secs)` — order not normalised here
/// (the caller stores it in drag order; render normalises on the fly).
fn draw_thumbnail(
    ui: &mut egui::Ui,
    thumb: Option<&Arc<CachedThumb>>,
    selection: Option<(f32, f32)>,
    playhead_frac: Option<f32>,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(THUMB_W, THUMB_H), egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(20, 20, 24));
    let Some(thumb) = thumb else {
        // Placeholder slash so corrupt / unreadable WAVs are visually
        // distinct from "loading" (which never appears in Phase A's
        // sync decode — the first frame either has peaks or doesn't).
        painter.line_segment(
            [rect.left_top(), rect.right_bottom()],
            egui::Stroke::new(0.5, egui::Color32::from_gray(80)),
        );
        return response;
    };
    if thumb.peaks.is_empty() {
        return response;
    }
    let mid = rect.center().y;
    let half_h = THUMB_H * 0.42;
    let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 200, 130));
    let cols = THUMB_W as usize;
    for x in 0..cols {
        let idx = (x as f32 / cols.max(1) as f32 * thumb.peaks.len() as f32) as usize;
        let idx = idx.min(thumb.peaks.len() - 1);
        let p = thumb.peaks[idx].min(1.0);
        let xp = rect.left() + x as f32;
        painter.line_segment(
            [
                egui::pos2(xp, mid - p * half_h),
                egui::pos2(xp, mid + p * half_h),
            ],
            stroke,
        );
    }

    // Selection overlay — a translucent fill across the selected range
    // plus thin vertical edges. Normalise the drag-order pair so the
    // overlay renders consistently whether the user dragged L→R or R→L.
    if let Some((a, b)) = selection {
        if thumb.duration_secs > 0.0 {
            let (s, e) = (a.min(b), a.max(b));
            let dur = thumb.duration_secs;
            let x0 = rect.left() + (s / dur).clamp(0.0, 1.0) * rect.width();
            let x1 = rect.left() + (e / dur).clamp(0.0, 1.0) * rect.width();
            let sel_rect =
                egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom()));
            painter.rect_filled(
                sel_rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(255, 200, 80, 50),
            );
            let edge = egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 200, 80));
            painter.line_segment(
                [egui::pos2(x0, rect.top()), egui::pos2(x0, rect.bottom())],
                edge,
            );
            painter.line_segment(
                [egui::pos2(x1, rect.top()), egui::pos2(x1, rect.bottom())],
                edge,
            );
        }
    }

    if let Some(frac) = playhead_frac {
        // Scrub playhead — drawn last so it rides above peaks/selection.
        let x = rect.left() + frac.clamp(0.0, 1.0) * rect.width();
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 200, 80)),
        );
    }
    response
}

/// Read the source WAV, crop to `[start, end]` losslessly via the
/// trim module's `crop_wav_bytes`, prompt the user for a save path
/// (default name `<stem>-<start>s-<end>s.wav`), and write the cropped
/// bytes. Returns the path written. TBSS-FR-0008 item (4) Phase C.
fn export_selection_to_file(src: &Path, start: f32, end: f32) -> anyhow::Result<PathBuf> {
    use anyhow::Context as _;
    let bytes =
        std::fs::read(src).with_context(|| format!("reading source WAV {}", src.display()))?;
    let cropped = crate::trim::crop_wav_bytes(&bytes, start, end)?;
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("take");
    let default_name = format!("{stem}-{:.2}s-{:.2}s.wav", start, end);
    let Some(out) = rfd::FileDialog::new()
        .add_filter("WAV", &["wav"])
        .set_file_name(&default_name)
        .save_file()
    else {
        anyhow::bail!("export cancelled");
    };
    std::fs::write(&out, &cropped.bytes).with_context(|| format!("writing {}", out.display()))?;
    Ok(out)
}

/// Drive selection state from a thumbnail's drag/click response. Click-
/// without-drag picks a single point (start == end). Click-drag picks a
/// range. Right-click clears the selection. All writes are best-effort
/// and never panic on a missing pointer pos.
fn update_selection_from_response(
    app: &mut TinyBoothApp,
    path: &Path,
    response: &egui::Response,
    duration_secs: f32,
) {
    if duration_secs <= 0.0 {
        return;
    }
    if response.secondary_clicked() {
        app.recordings_selection.remove(path);
        return;
    }
    let rect = response.rect;
    if rect.width() <= 0.0 {
        return;
    }
    let pos_to_secs = |pos: egui::Pos2| -> f32 {
        let local_x = (pos.x - rect.left()).clamp(0.0, rect.width());
        (local_x / rect.width()) * duration_secs
    };
    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            let t = pos_to_secs(pos);
            app.recordings_selection.insert(path.to_path_buf(), (t, t));
        }
    } else if response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            let t = pos_to_secs(pos);
            let start = app
                .recordings_selection
                .get(path)
                .map(|(s, _)| *s)
                .unwrap_or(t);
            app.recordings_selection
                .insert(path.to_path_buf(), (start, t));
        }
    }
}

/// Render the per-row "Export selection" button. Disabled when there's
/// no selection on this take. On click, runs `export_selection_to_file`
/// and routes the result through the app status bar.
fn export_selection_button(app: &mut TinyBoothApp, path: &Path, ui: &mut egui::Ui) {
    let sel = app.recordings_selection.get(path).copied();
    let has_sel = sel.is_some();
    ui.add_enabled_ui(has_sel, |ui| {
        if ui
            .small_button("💾")
            .on_hover_text(
                "Export the selected region to a new WAV (click-drag on the \
                 thumbnail to pick a range; right-click clears)",
            )
            .clicked()
        {
            if let Some((a, b)) = sel {
                let (start, end) = (a.min(b), a.max(b));
                match export_selection_to_file(path, start, end) {
                    Ok(out) => {
                        app.status = Some(format!("Exported selection → {}", out.display()));
                    }
                    Err(e) => {
                        app.status = Some(format!("Export failed: {e:#}"));
                    }
                }
            }
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
