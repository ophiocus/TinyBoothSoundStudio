//! Album tab — N-stem composition editor. TBSS-FR-0012.
//!
//! Edits an in-memory [`crate::album::Album`], renders it via
//! [`crate::album::render`], plays the preview through a
//! [`crate::crossfade_player::CrossfadePreviewSession`], exports via
//! the existing [`crate::export::write_crossfade`] helper, and
//! bounces into the open `.tba`'s `mix_run` row.

use crate::album::{Album, AlbumClip};
use crate::app::{AlbumUiState, TinyBoothApp};
use crate::crossfade_player::CrossfadePreviewSession;
use crate::tba::TbaDb;
use eframe::egui;
use std::path::PathBuf;

const TIMELINE_H: f32 = 80.0;

pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    ui.heading("Album");
    ui.label(
        egui::RichText::new(
            "Arrange N bounced .tib stems on a timeline with per-clip gain + \
             fade-in/fade-out. ▶ Preview, ⤓ Bounce into this .tba, Export to any \
             format. TBSS-FR-0012.",
        )
        .weak(),
    );
    ui.separator();

    // ── Header row: name + Open / Save / Save As ──────────────────
    let mut click_open = false;
    let mut click_save = false;
    let mut click_save_as = false;
    let mut click_new = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Album name:").strong());
        let resp = ui.add(
            egui::TextEdit::singleline(&mut app.album_state.album.name)
                .desired_width(220.0)
                .hint_text("Untitled album"),
        );
        if resp.changed() {
            app.album_state.dirty = true;
        }
        ui.separator();
        if ui.button("New").clicked() {
            click_new = true;
        }
        if ui.button("Open…").clicked() {
            click_open = true;
        }
        let save_label = if app.album_state.dirty {
            "Save *"
        } else {
            "Save"
        };
        ui.add_enabled_ui(app.album_state.path.is_some(), |ui| {
            if ui.button(save_label).clicked() {
                click_save = true;
            }
        });
        if ui.button("Save As…").clicked() {
            click_save_as = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(p) = app.album_state.path.as_ref() {
                ui.monospace(p.file_name().and_then(|n| n.to_str()).unwrap_or(""));
            } else {
                ui.label(egui::RichText::new("(no .tba on disk yet)").weak());
            }
        });
    });

    ui.add_space(4.0);
    draw_timeline(&app.album_state, ui);
    ui.add_space(8.0);

    // ── Clip list ──────────────────────────────────────────────────
    let mut add_clip = false;
    let mut move_up: Option<usize> = None;
    let mut move_down: Option<usize> = None;
    let mut remove: Option<usize> = None;
    egui::ScrollArea::vertical()
        .max_height(260.0)
        .show(ui, |ui| {
            egui::Grid::new("album_clip_grid")
                .num_columns(8)
                .spacing(egui::vec2(8.0, 4.0))
                .striped(true)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("#").strong());
                    ui.label(egui::RichText::new("Source").strong());
                    ui.label(egui::RichText::new("Start (s)").strong());
                    ui.label(egui::RichText::new("Fade in (s)").strong());
                    ui.label(egui::RichText::new("Fade out (s)").strong());
                    ui.label(egui::RichText::new("Gain (dB)").strong());
                    ui.label("");
                    ui.label("");
                    ui.end_row();
                    let n = app.album_state.album.clips.len();
                    for (i, clip) in app.album_state.album.clips.iter_mut().enumerate() {
                        ui.monospace(format!("{}", i + 1));
                        ui.monospace(
                            clip.source_path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("(missing)"),
                        )
                        .on_hover_text(clip.source_path.to_string_lossy().to_string());
                        let changed_start = ui
                            .add(
                                egui::DragValue::new(&mut clip.start_secs)
                                    .speed(0.05)
                                    .range(0.0..=f32::INFINITY),
                            )
                            .changed();
                        let changed_fin = ui
                            .add(
                                egui::DragValue::new(&mut clip.fade_in_secs)
                                    .speed(0.02)
                                    .range(0.0..=f32::INFINITY),
                            )
                            .changed();
                        let changed_fout = ui
                            .add(
                                egui::DragValue::new(&mut clip.fade_out_secs)
                                    .speed(0.02)
                                    .range(0.0..=f32::INFINITY),
                            )
                            .changed();
                        let changed_gain = ui
                            .add(
                                egui::DragValue::new(&mut clip.gain_db)
                                    .speed(0.1)
                                    .range(-60.0..=12.0)
                                    .suffix(" dB"),
                            )
                            .changed();
                        if changed_start || changed_fin || changed_fout || changed_gain {
                            app.album_state.dirty = true;
                        }
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(i > 0, |ui| {
                                if ui.small_button("▲").on_hover_text("Move up").clicked() {
                                    move_up = Some(i);
                                }
                            });
                            ui.add_enabled_ui(i + 1 < n, |ui| {
                                if ui.small_button("▼").on_hover_text("Move down").clicked() {
                                    move_down = Some(i);
                                }
                            });
                        });
                        if ui.small_button("✖").on_hover_text("Remove clip").clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
        });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("➕ Add Clip…").clicked() {
            add_clip = true;
        }
        if app.album_state.album.clips.is_empty() {
            ui.label(
                egui::RichText::new("Add a bounced .tib stem to begin.")
                    .weak()
                    .small(),
            );
        }
    });

    if let Some(i) = move_up {
        if i > 0 {
            app.album_state.album.clips.swap(i - 1, i);
            app.album_state.dirty = true;
        }
    }
    if let Some(i) = move_down {
        if i + 1 < app.album_state.album.clips.len() {
            app.album_state.album.clips.swap(i, i + 1);
            app.album_state.dirty = true;
        }
    }
    if let Some(i) = remove {
        app.album_state.album.clips.remove(i);
        app.album_state.dirty = true;
    }
    if add_clip {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("TinyBooth project (.tib)", &["tib"])
            .pick_file()
        {
            app.album_state.album.clips.push(AlbumClip {
                source_path: p,
                start_secs: next_default_start(&app.album_state.album),
                fade_in_secs: 2.0,
                fade_out_secs: 2.0,
                gain_db: 0.0,
            });
            app.album_state.dirty = true;
        }
    }

    ui.separator();

    // ── Transport ─────────────────────────────────────────────────
    let mut click_play = false;
    let mut click_stop = false;
    let mut click_bounce = false;
    let mut click_export = false;
    let have_clips = !app.album_state.album.clips.is_empty();
    let preview_active = app.album_state.preview.is_some();
    let (cache_present, cache_fresh) = album_run_status(&app.album_state);
    ui.horizontal(|ui| {
        ui.add_enabled_ui(have_clips, |ui| {
            if ui
                .button("▶ Preview")
                .on_hover_text("Render the album in memory and play through the default output.")
                .clicked()
            {
                click_play = true;
            }
        });
        ui.add_enabled_ui(preview_active, |ui| {
            if ui.button("■ Stop").clicked() {
                click_stop = true;
            }
        });
        ui.separator();
        ui.add_enabled_ui(have_clips && app.album_state.db.is_some(), |ui| {
            let label = if !cache_present {
                "⤓ Bounce".to_string()
            } else if cache_fresh {
                "⤓ Bounce  ✓".to_string()
            } else {
                "⤓ Bounce  ⚠ stale".to_string()
            };
            if ui
                .button(label)
                .on_hover_text(
                    "Render the album and stash it in this .tba's mix_run row so \
                     the Crossfade tab (or another Album) can load it as a single stem.",
                )
                .clicked()
            {
                click_bounce = true;
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled_ui(have_clips, |ui| {
                if ui
                    .button("Export…")
                    .on_hover_text("Render the album to a file in the chosen format.")
                    .clicked()
                {
                    click_export = true;
                }
            });
            ui.label("Format:");
            egui::ComboBox::from_id_source("album_export_format")
                .selected_text(app.album_state.export_format.label())
                .show_ui(ui, |ui| {
                    for fmt in crate::export::ExportFormat::all() {
                        ui.selectable_value(&mut app.album_state.export_format, fmt, fmt.label());
                    }
                });
        });
    });

    // Drop the preview once it's finished playing.
    if let Some(sess) = app.album_state.preview.as_ref() {
        if sess.is_finished() {
            app.album_state.preview = None;
        }
    }

    if click_new {
        do_new(&mut app.album_state);
    }
    if click_open {
        do_open(&mut app.album_state);
    }
    if click_save {
        do_save(&mut app.album_state);
    }
    if click_save_as {
        do_save_as(&mut app.album_state);
    }
    if click_play {
        do_preview(&mut app.album_state);
    }
    if click_stop {
        app.album_state.preview = None;
    }
    if click_bounce {
        do_bounce(&mut app.album_state);
    }
    if click_export {
        do_export(&mut app.album_state);
    }

    if let Some(msg) = app.album_state.status.clone() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(msg).monospace());
    }
}

/// Top-down timeline visualisation — each clip rendered as a colored
/// band at its `[start, start+duration]` range, with shaded fade
/// edges. Read-only in v0.4.52; drag editing is deferred polish.
fn draw_timeline(st: &AlbumUiState, ui: &mut egui::Ui) {
    let avail_w = ui.available_width().max(200.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(avail_w, TIMELINE_H), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(14, 14, 18));

    if st.album.clips.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "(timeline empty)",
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(120),
        );
        return;
    }

    // We don't know clip durations at edit time (would require
    // probing each .tib's mix_run header). Render each clip as a
    // 1-second-default band starting at `start_secs` — purely for
    // arrangement awareness. End-of-clip is implied by fade-out edge.
    let max_end = st
        .album
        .clips
        .iter()
        .map(|c| c.start_secs + c.fade_in_secs.max(0.1) + c.fade_out_secs.max(0.1) + 1.0)
        .fold(1.0_f32, f32::max);
    let n = st.album.clips.len();
    let band_h = ((rect.height() - 8.0) / n.max(1) as f32).max(8.0);
    let secs_to_x = |s: f32| -> f32 { rect.left() + (s / max_end) * rect.width() };
    let palette = [
        egui::Color32::from_rgb(120, 180, 200),
        egui::Color32::from_rgb(180, 140, 200),
        egui::Color32::from_rgb(120, 200, 130),
        egui::Color32::from_rgb(200, 180, 100),
        egui::Color32::from_rgb(200, 120, 140),
    ];
    for (i, clip) in st.album.clips.iter().enumerate() {
        let color = palette[i % palette.len()];
        let y0 = rect.top() + 4.0 + i as f32 * band_h;
        let y1 = y0 + band_h - 2.0;
        // Default clip-width visualisation: 5 seconds. The bounce
        // step will replace this with the actual decoded length.
        let assumed_dur = 5.0_f32;
        let x0 = secs_to_x(clip.start_secs.max(0.0));
        let x1 = secs_to_x(clip.start_secs + assumed_dur);
        let band = egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1));
        painter.rect_filled(band, 2.0, color.linear_multiply(0.4));
        // Fade-edge shading
        let fin_x1 = secs_to_x(clip.start_secs + clip.fade_in_secs).min(x1);
        let fout_x0 = secs_to_x(clip.start_secs + assumed_dur - clip.fade_out_secs).max(x0);
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(fin_x1, y1)),
            2.0,
            color.linear_multiply(0.2),
        );
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(fout_x0, y0), egui::pos2(x1, y1)),
            2.0,
            color.linear_multiply(0.2),
        );
        painter.text(
            egui::pos2(x0 + 4.0, y0 + 1.0),
            egui::Align2::LEFT_TOP,
            clip.source_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?"),
            egui::FontId::monospace(10.0),
            egui::Color32::from_gray(230),
        );
    }
}

/// Default start time for a freshly-added clip — places it just past
/// the latest existing clip's start (with a small gap) so adding clips
/// in sequence yields a roughly-sequential timeline by default.
fn next_default_start(album: &Album) -> f32 {
    album
        .clips
        .iter()
        .map(|c| c.start_secs)
        .fold(0.0_f32, f32::max)
        + 5.0
}

/// Snapshot of the mix-run cache state for the Bounce pip.
fn album_run_status(st: &AlbumUiState) -> (bool, bool) {
    let Some(db) = st.db.as_ref() else {
        return (false, false);
    };
    let Ok(Some(header)) = db.read_mix_run_header() else {
        return (false, false);
    };
    let live = crate::album::compute_mixrun_signature(&st.album);
    (true, live == header.source_signature)
}

fn do_new(st: &mut AlbumUiState) {
    st.album = Album::default();
    st.path = None;
    st.db = None;
    st.dirty = false;
    st.preview = None;
    st.status = Some("New album — Save As… when ready.".into());
}

fn do_open(st: &mut AlbumUiState) {
    let Some(p) = rfd::FileDialog::new()
        .add_filter("TinyBooth Album (.tba)", &["tba"])
        .pick_file()
    else {
        return;
    };
    match TbaDb::open(p.clone()) {
        Ok(db) => match crate::tba_album::load_album(&db) {
            Ok(album) => {
                st.album = album;
                st.db = Some(db);
                st.path = Some(p.clone());
                st.dirty = false;
                st.preview = None;
                st.status = Some(format!(
                    "opened {} ({} clip{})",
                    p.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                    st.album.clips.len(),
                    if st.album.clips.len() == 1 { "" } else { "s" }
                ));
            }
            Err(e) => st.status = Some(format!("open failed: {e:#}")),
        },
        Err(e) => st.status = Some(format!("open failed: {e:#}")),
    }
}

fn do_save(st: &mut AlbumUiState) {
    let Some(db) = st.db.as_mut() else {
        st.status = Some("No .tba on disk — use Save As…".into());
        return;
    };
    match crate::tba_album::save_album(db, &st.album) {
        Ok(()) => {
            st.dirty = false;
            st.status = Some("Saved.".into());
        }
        Err(e) => st.status = Some(format!("save failed: {e:#}")),
    }
}

fn do_save_as(st: &mut AlbumUiState) {
    let default_name = if st.album.name.is_empty() {
        "album.tba".to_string()
    } else {
        format!("{}.tba", st.album.name.replace(['/', '\\', ':'], "-"))
    };
    let Some(p) = rfd::FileDialog::new()
        .add_filter("TinyBooth Album (.tba)", &["tba"])
        .set_file_name(&default_name)
        .save_file()
    else {
        return;
    };
    let path: PathBuf = if p.extension().is_none() {
        p.with_extension("tba")
    } else {
        p
    };
    match TbaDb::create(path.clone(), &st.album.name) {
        Ok(mut db) => match crate::tba_album::save_album(&mut db, &st.album) {
            Ok(()) => {
                st.db = Some(db);
                st.path = Some(path.clone());
                st.dirty = false;
                st.status = Some(format!(
                    "Saved → {}",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                ));
            }
            Err(e) => st.status = Some(format!("save failed: {e:#}")),
        },
        Err(e) => st.status = Some(format!("create failed: {e:#}")),
    }
}

fn do_preview(st: &mut AlbumUiState) {
    st.preview = None;
    let clips = match crate::album::load_clips(&st.album.clips) {
        Ok(c) => c,
        Err(e) => {
            st.status = Some(format!("preview load failed: {e:#}"));
            return;
        }
    };
    let mix = match crate::album::render(&clips) {
        Ok(m) => m,
        Err(e) => {
            st.status = Some(format!("preview render failed: {e:#}"));
            return;
        }
    };
    match CrossfadePreviewSession::play(mix.samples, mix.sample_rate, mix.channels, 0) {
        Ok(s) => {
            st.preview = Some(s);
            st.status = Some(format!(
                "Playing album ({} Hz · {} clip{})",
                mix.sample_rate,
                st.album.clips.len(),
                if st.album.clips.len() == 1 { "" } else { "s" }
            ));
        }
        Err(e) => st.status = Some(format!("preview failed: {e:#}")),
    }
}

fn do_bounce(st: &mut AlbumUiState) {
    let clips = match crate::album::load_clips(&st.album.clips) {
        Ok(c) => c,
        Err(e) => {
            st.status = Some(format!("bounce load failed: {e:#}"));
            return;
        }
    };
    let mix = match crate::album::render(&clips) {
        Ok(m) => m,
        Err(e) => {
            st.status = Some(format!("bounce render failed: {e:#}"));
            return;
        }
    };
    let wav_bytes = match crate::album::encode_mix_to_wav_bytes(&mix) {
        Ok(b) => b,
        Err(e) => {
            st.status = Some(format!("bounce encode failed: {e:#}"));
            return;
        }
    };
    let frames = (mix.samples.len() as u64) / (mix.channels as u64).max(1);
    let signature = crate::album::compute_mixrun_signature(&st.album);
    let Some(db) = st.db.as_mut() else {
        st.status = Some("Bounce needs an open .tba — Save As… first.".into());
        return;
    };
    match db.write_mix_run(
        mix.sample_rate,
        mix.channels,
        frames,
        &signature,
        &wav_bytes,
    ) {
        Ok(()) => {
            let secs = frames as f32 / mix.sample_rate.max(1) as f32;
            st.status = Some(format!(
                "Bounced album ({} Hz · {:.2} s) → .tba mix_run",
                mix.sample_rate, secs
            ));
        }
        Err(e) => st.status = Some(format!("bounce write failed: {e:#}")),
    }
}

fn do_export(st: &mut AlbumUiState) {
    let clips = match crate::album::load_clips(&st.album.clips) {
        Ok(c) => c,
        Err(e) => {
            st.status = Some(format!("export load failed: {e:#}"));
            return;
        }
    };
    let mix = match crate::album::render(&clips) {
        Ok(m) => m,
        Err(e) => {
            st.status = Some(format!("export render failed: {e:#}"));
            return;
        }
    };
    let stem = if st.album.name.is_empty() {
        "album".to_string()
    } else {
        st.album.name.replace(['/', '\\', ':'], "-")
    };
    let ext = st.export_format.extension();
    let default_name = format!("{stem}.{ext}");
    let Some(out) = rfd::FileDialog::new()
        .add_filter(st.export_format.label(), &[ext])
        .set_file_name(&default_name)
        .save_file()
    else {
        return;
    };
    let opts = crate::export::ExportOptions {
        format: st.export_format,
        bitrate_kbps: 192,
        out_path: out.clone(),
    };
    match crate::export::write_crossfade(&mix.samples, mix.sample_rate, mix.channels, &opts) {
        Ok(()) => {
            st.status = Some(format!("Exported → {}", out.display()));
        }
        Err(e) => st.status = Some(format!("export failed: {e:#}")),
    }
}
