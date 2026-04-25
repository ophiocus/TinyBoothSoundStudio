//! Mix tab — multitrack playback + remastering UI.
//!
//! Top transport bar (▶/⏸/⏹ + time), then one row per track: header
//! controls on the left (mute / A-B bypass / gain / Correction…) and a
//! waveform lane on the right with the synchronized playhead drawn over
//! every lane at the same X position.

use crate::app::TinyBoothApp;
use crate::player::{PlayState, Player};
use eframe::egui;
use egui::{Color32, Pos2, Rect, Stroke};

const HEADER_W: f32 = 280.0;
const LANE_H: f32 = 70.0;
const ROW_GAP: f32 = 8.0;

pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    if app.project.tracks.is_empty() {
        ui.heading("Mix");
        ui.separator();
        ui.label("Record at least one track or import a Suno bundle to mix.");
        return;
    }

    // Lazy-instantiate the player on first entry into the tab. We rebuild
    // when the project's track count changed since last build (cheap
    // rebuild every project switch — Phase-3 polish could detect more
    // granular changes).
    let need_rebuild = match app.player.as_ref() {
        None => true,
        Some(p) => p.state.tracks.len() != app.project.tracks.len(),
    };
    if need_rebuild {
        app.player = None;
        app.player_error = None;
        match Player::new(&app.project) {
            Ok(p) => app.player = Some(p),
            Err(e) => app.player_error = Some(format!("{e}")),
        }
    }

    // ── Transport bar ────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.heading("Mix");
        ui.separator();
        let player = app.player.as_ref();
        let playing = player.map(|p| p.state.play_state() == PlayState::Playing).unwrap_or(false);
        let enabled = player.is_some();

        ui.add_enabled_ui(enabled, |ui| {
            if !playing {
                if ui.add(egui::Button::new("▶  Play").min_size(egui::vec2(80.0, 30.0))).clicked() {
                    if let Some(p) = app.player.as_ref() { p.play(); }
                }
            } else {
                if ui.add(egui::Button::new("⏸  Pause").min_size(egui::vec2(80.0, 30.0))).clicked() {
                    if let Some(p) = app.player.as_ref() { p.pause(); }
                }
            }
            if ui.add(egui::Button::new("⏹  Stop").min_size(egui::vec2(80.0, 30.0))).clicked() {
                if let Some(p) = app.player.as_ref() { p.stop(); }
            }
        });

        ui.separator();
        if let Some(p) = player {
            let pos = p.state.position_secs();
            let dur = p.state.duration_secs();
            ui.monospace(format!("{}  /  {}", fmt_time(pos), fmt_time(dur)));
            ui.separator();
            ui.label(format!("{} Hz · stereo bus", p.state.sample_rate));
        }
    });

    if let Some(err) = app.player_error.as_ref() {
        ui.colored_label(Color32::LIGHT_RED, err);
        return;
    }

    let Some(player) = app.player.as_ref() else { return };

    ui.separator();

    // ── Track lanes ─────────────────────────────────────────────
    let dur = player.state.duration_secs().max(0.001);
    let pos = player.state.position_secs();

    // Index of the track whose Correction editor was just requested
    // (handled outside the borrow on app.player).
    let mut requested_correction: Option<usize> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        for (idx, track) in player.state.tracks.iter().enumerate() {
            ui.horizontal(|ui| {
                // Header column.
                ui.allocate_ui_with_layout(
                    egui::vec2(HEADER_W, LANE_H),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.add_space(2.0);
                        ui.label(egui::RichText::new(&track.name).strong());
                        let mut mute = track.mute.load(std::sync::atomic::Ordering::Relaxed);
                        let mut bypass = track.bypass_correction.load(std::sync::atomic::Ordering::Relaxed);
                        let has_corr = track.correction().is_some();

                        ui.horizontal(|ui| {
                            if ui.add(egui::SelectableLabel::new(mute, "🔇")).on_hover_text("Mute").clicked() {
                                mute = !mute;
                                track.mute.store(mute, std::sync::atomic::Ordering::Relaxed);
                            }
                            ui.add_enabled_ui(has_corr, |ui| {
                                if ui.add(egui::SelectableLabel::new(bypass, "A/B"))
                                    .on_hover_text(if bypass { "Bypassed (original)" } else { "Correction active" })
                                    .clicked()
                                {
                                    bypass = !bypass;
                                    track.bypass_correction.store(bypass, std::sync::atomic::Ordering::Relaxed);
                                }
                            });
                            let mut gain = track.gain_db();
                            if ui.add(egui::DragValue::new(&mut gain).speed(0.1).suffix(" dB").range(-24.0..=12.0)).changed() {
                                track.set_gain_db(gain);
                            }
                            let label = if has_corr { "Correction" } else { "+ Correction" };
                            if ui.button(label).clicked() {
                                requested_correction = Some(idx);
                            }
                        });
                    },
                );

                // Waveform lane fills the rest of the row.
                let avail = ui.available_size().x.max(200.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(avail, LANE_H), egui::Sense::hover());
                draw_lane(ui, rect, &track.peaks, dur, pos, track.frame_count, track.sample_rate);
            });
            ui.add_space(ROW_GAP);
        }
    });

    if let Some(i) = requested_correction {
        // If the track has no correction yet, seed it from the active
        // recording-tone profile so the user has something to tweak.
        if app.project.tracks[i].correction.is_none() {
            let seed = app.profiles
                .iter()
                .find(|p| p.name == "Suno-Clean")
                .or_else(|| app.profiles.first())
                .cloned();
            app.project.tracks[i].correction = seed.clone();
            app.project_dirty = true;
            // Push to the player so playback updates immediately.
            if let Some(player) = app.player.as_ref() {
                if let Some(track) = player.state.tracks.get(i) {
                    track.set_correction(seed);
                }
            }
        }
        app.editing_correction_for = Some(i);
    }
}

fn draw_lane(
    ui: &mut egui::Ui,
    rect: Rect,
    peaks: &[f32],
    total_secs: f32,
    pos_secs: f32,
    track_frames: u64,
    sample_rate: u32,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(10, 10, 14));

    if peaks.is_empty() {
        return;
    }

    // The lane represents the full project duration, so a shorter track
    // only fills the proportion of the lane corresponding to its own
    // length.
    let track_secs = track_frames as f32 / sample_rate.max(1) as f32;
    let track_w = rect.width() * (track_secs / total_secs).min(1.0);
    let track_rect = Rect::from_min_size(rect.min, egui::vec2(track_w, rect.height()));

    let mid_y = rect.center().y;
    let gain = rect.height() * 0.45;
    let stroke = Stroke::new(1.0, Color32::from_rgb(100, 220, 150));
    let cols = track_rect.width() as usize;
    if cols > 0 {
        for x_px in 0..cols {
            let bin_idx = (x_px as f32 / cols.max(1) as f32 * peaks.len() as f32) as usize;
            let bin_idx = bin_idx.min(peaks.len() - 1);
            let p = peaks[bin_idx];
            let h = p * gain;
            let x = track_rect.min.x + x_px as f32;
            painter.line_segment([Pos2::new(x, mid_y - h), Pos2::new(x, mid_y + h)], stroke);
        }
    }
    // Centre baseline.
    painter.line_segment(
        [Pos2::new(rect.min.x, mid_y), Pos2::new(rect.max.x, mid_y)],
        Stroke::new(0.5, Color32::from_gray(40)),
    );

    // Synchronized playhead — draw on every lane at the same X.
    let head_x = rect.min.x + rect.width() * (pos_secs / total_secs).clamp(0.0, 1.0);
    painter.line_segment(
        [Pos2::new(head_x, rect.min.y), Pos2::new(head_x, rect.max.y)],
        Stroke::new(1.5, Color32::from_rgb(230, 200, 80)),
    );
}

fn fmt_time(secs: f32) -> String {
    let total = secs.max(0.0) as u32;
    format!("{:02}:{:02}", total / 60, total % 60)
}
