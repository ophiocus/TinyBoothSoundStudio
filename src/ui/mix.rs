//! Mix tab — multitrack lanes (top) + console deck (bottom).
//!
//! The bottom half of the tab is a hardware-style console: vertical
//! fader strips per track plus a master strip on the right. Each strip
//! has its own mute / solo / arm-automation toggles, vertical fader,
//! peak meter, and dB readout.
//!
//! Automation: when a strip's `R` (arm) toggle is on and playback is
//! Playing, the UI thread samples the live `gain_db` once per frame
//! and feeds it to the project-wide [`Recorder`]. On Stop / disarm
//! the scratch lane is committed to the matching `Track.gain_automation`
//! (or `Project.master_gain_automation`) and a fresh `SplineSampler`
//! is shipped to the audio thread, which replays it via Catmull-Rom
//! interpolation on the next playback.

use crate::app::TinyBoothApp;
use crate::player::{PlayState, Player};
use eframe::egui;
use egui::{Color32, Pos2, Rect, Stroke};
use std::sync::atomic::Ordering;

const HEADER_W: f32 = 220.0;
const LANE_H: f32 = 60.0;
const ROW_GAP: f32 = 6.0;

const STRIP_W: f32 = 108.0;
const STRIP_GAP: f32 = 4.0;
const FADER_H: f32 = 130.0;
const METER_W: f32 = 6.0;
/// Cap on track-name characters before we ellipsise. Tuned so Latin-script
/// names like "Backing Vocals" / "Electric Guitar" / "Synth / Lead" fit
/// inside `STRIP_W` without truncation at 1.0× zoom.
const STRIP_NAME_CHARS: usize = 14;
/// Strip-name font size in pt at 1.0× zoom. Egui's `set_zoom_factor`
/// scales these proportionally — bump zoom from the View menu.
const FONT_STRIP_NAME: f32 = 13.0;
const FONT_STRIP_DB: f32 = 12.0;
const FONT_MASTER_NAME: f32 = 14.0;

pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    if app.project.tracks.is_empty() {
        ui.heading("Mix");
        ui.separator();
        ui.label("Record at least one track or import a Suno bundle to mix.");
        return;
    }

    // Lazy-instantiate the player.
    let need_rebuild = match app.player.as_ref() {
        None => true,
        Some(p) => p.state.tracks.len() != app.project.tracks.len(),
    };
    if need_rebuild {
        app.player = None;
        app.player_error = None;
        match Player::new(&app.project, app.audio_err_tx.clone()) {
            Ok(p) => app.player = Some(p),
            Err(e) => app.player_error = Some(format!("{e}")),
        }
    }

    transport_bar(app, ui);

    if let Some(err) = app.player_error.as_ref() {
        ui.colored_label(Color32::LIGHT_RED, err);
        return;
    }
    if app.player.is_none() {
        return;
    }

    ui.separator();

    // Capture fader values for any armed strips while playing.
    capture_automation(app);

    // Split the remaining vertical space between lanes (top) and
    // console deck (bottom). The split is user-resizable via a dragger
    // between them.
    let total = ui.available_height().max(200.0);
    let console_h = (total * app.mix_console_fraction.clamp(0.2, 0.7)).max(180.0);
    let lanes_h = (total - console_h - 8.0).max(120.0);

    // Lanes panel.
    egui::TopBottomPanel::top("mix_lanes_panel")
        .resizable(false)
        .exact_height(lanes_h)
        .show_inside(ui, |ui| {
            lanes_view(app, ui);
        });

    // Resize handle.
    ui.add_space(2.0);
    let (drag_rect, drag_resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::drag());
    let painter = ui.painter_at(drag_rect);
    painter.line_segment(
        [
            Pos2::new(drag_rect.min.x + 60.0, drag_rect.center().y),
            Pos2::new(drag_rect.max.x - 60.0, drag_rect.center().y),
        ],
        Stroke::new(
            2.0,
            if drag_resp.hovered() {
                Color32::from_gray(120)
            } else {
                Color32::from_gray(60)
            },
        ),
    );
    if drag_resp.dragged() {
        let dy = drag_resp.drag_delta().y / total;
        app.mix_console_fraction = (app.mix_console_fraction - dy).clamp(0.2, 0.7);
    }

    // Console deck.
    console_deck(app, ui);
}

// ───────────────────── transport ─────────────────────

fn transport_bar(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    // Snapshot read-only player state so we can split borrows.
    let (have_player, playing, pos_str, sample_rate) = if let Some(p) = app.player.as_ref() {
        let pos = p.state.position_secs();
        let dur = p.state.duration_secs();
        (
            true,
            p.state.play_state() == PlayState::Playing,
            format!("{}  /  {}", fmt_time(pos), fmt_time(dur)),
            p.state.sample_rate,
        )
    } else {
        (false, false, String::new(), 0)
    };

    // How many tracks already carry a correction chain — drives the
    // bulk-action buttons' enabled state and labels.
    let n_tracks = app.project.tracks.len();
    let n_with_corr = app
        .project
        .tracks
        .iter()
        .filter(|t| t.correction.is_some())
        .count();
    let n_without_corr = n_tracks.saturating_sub(n_with_corr);

    // Ephemeral global bypass — atomic on PlayerState, set by either
    // the A/B button (transient) or the persisted Disable toggle.
    let global_bypass_on = match app.player.as_ref() {
        Some(p) => p
            .state
            .global_bypass
            .load(std::sync::atomic::Ordering::Relaxed),
        None => false,
    };
    // Persisted project flag — separate from the atomic so the user can
    // toggle live A/B without dirtying the project, and toggle the
    // persisted flag without leaving the project in a "live A/B" state
    // on reload.
    let corrections_disabled = app.project.corrections_disabled;

    let mut click_play = false;
    let mut click_pause = false;
    let mut click_stop = false;
    let mut click_enable_all = false;
    let mut click_disable_persisted = false;
    let mut click_reset_all = false;
    let mut click_toggle_bypass = false;

    ui.horizontal(|ui| {
        ui.heading("Mix");
        ui.separator();
        ui.add_enabled_ui(have_player, |ui| {
            if !playing {
                if ui.add(egui::Button::new("▶  Play").min_size(egui::vec2(80.0, 30.0))).clicked() {
                    click_play = true;
                }
            } else if ui.add(egui::Button::new("⏸  Pause").min_size(egui::vec2(80.0, 30.0))).clicked() {
                click_pause = true;
            }
            if ui.add(egui::Button::new("⏹  Stop").min_size(egui::vec2(80.0, 30.0))).clicked() {
                click_stop = true;
            }
        });
        ui.separator();
        if have_player {
            ui.monospace(pos_str);
            ui.separator();
            ui.label(format!("{} Hz · stereo bus", sample_rate));
            ui.separator();
        }

        // Bulk correction toggles. "Enable all" seeds Suno-Clean on
        // every track currently at correction = None; doesn't overwrite
        // tracks the user has already tweaked.
        ui.add_enabled_ui(n_tracks > 0 && n_without_corr > 0, |ui| {
            let label = if n_without_corr == n_tracks {
                "✓ Enable all corrections".to_string()
            } else {
                format!("✓ Enable corrections on {n_without_corr}/{n_tracks}")
            };
            if ui.add(egui::Button::new(label).min_size(egui::vec2(160.0, 28.0)))
                .on_hover_text("Apply Suno-Clean to every track without an existing correction chain. Doesn't overwrite tracks you've already edited.")
                .clicked()
            {
                click_enable_all = true;
            }
        });
        // Persisted Disable — flips Project.corrections_disabled and
        // syncs the player's global_bypass. Survives reload.
        ui.add_enabled_ui(n_tracks > 0, |ui| {
            let label = if corrections_disabled {
                "⊘ Disabled (saved)"
            } else {
                "⊘ Disable (saves)"
            };
            if ui.add(egui::SelectableLabel::new(corrections_disabled, label))
                .on_hover_text("Persisted project-wide bypass. Saves to the manifest — corrections stay off across reloads. Toggle again to re-enable. Non-destructive: chain config is preserved.")
                .clicked()
            {
                click_disable_persisted = true;
            }
        });
        ui.add_enabled_ui(n_with_corr > 0, |ui| {
            let label = if n_with_corr == n_tracks {
                "⟲ Reset all".to_string()
            } else {
                format!("⟲ Reset {n_with_corr}/{n_tracks}")
            };
            if ui.add(egui::Button::new(label).min_size(egui::vec2(110.0, 28.0)))
                .on_hover_text("Destructive — strips every correction chain. Tweaks lost. Re-enable to re-seed from the cascade (project default → feature default).")
                .clicked()
            {
                click_reset_all = true;
            }
        });
        ui.separator();
        // Ephemeral A/B — flips global_bypass without touching the
        // project flag. Useful for live listening; reload restores the
        // persisted state.
        ui.add_enabled_ui(n_with_corr > 0 || corrections_disabled, |ui| {
            let label = if global_bypass_on { "A/B  ▣  bypassed" } else { "A/B  ☐  live" };
            if ui.add(egui::SelectableLabel::new(global_bypass_on, label))
                .on_hover_text("Ephemeral global A/B — flips the audio thread's bypass without touching the project flag. Doesn't dirty the project; reload restores whatever the persisted Disable was.")
                .clicked()
            {
                click_toggle_bypass = true;
            }
        });
    });

    if click_play {
        if let Some(p) = app.player.as_ref() {
            p.play();
        }
    }
    if click_pause {
        if let Some(p) = app.player.as_ref() {
            p.pause();
        }
    }
    if click_stop {
        stop_and_commit_automation(app);
    }
    if click_enable_all {
        app.enable_all_corrections();
    }
    if click_disable_persisted {
        app.toggle_corrections_disabled();
    }
    if click_reset_all {
        app.reset_all_corrections();
    }
    if click_toggle_bypass {
        app.toggle_global_bypass();
    }
}

// ───────────────────── multitrack lane view ─────────────────────

fn lanes_view(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    let Some(player) = app.player.as_ref() else {
        return;
    };
    let dur = player.state.duration_secs().max(0.001);
    let pos = player.state.position_secs();

    let mut requested_correction: Option<usize> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        for (idx, track) in player.state.tracks.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(HEADER_W, LANE_H),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.add_space(2.0);
                        ui.label(egui::RichText::new(&track.name).strong());
                        ui.horizontal(|ui| {
                            let mut bypass = track.bypass_correction.load(Ordering::Relaxed);
                            let has_corr = track.correction().is_some();
                            ui.add_enabled_ui(has_corr, |ui| {
                                if ui
                                    .add(egui::SelectableLabel::new(bypass, "A/B"))
                                    .on_hover_text(if bypass {
                                        "Bypassed (original)"
                                    } else {
                                        "Correction active"
                                    })
                                    .clicked()
                                {
                                    bypass = !bypass;
                                    track.bypass_correction.store(bypass, Ordering::Relaxed);
                                }
                            });
                            let label = if has_corr {
                                "Correction"
                            } else {
                                "+ Correction"
                            };
                            if ui.button(label).clicked() {
                                requested_correction = Some(idx);
                            }
                        });
                    },
                );

                let avail = ui.available_size().x.max(200.0);
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(avail, LANE_H), egui::Sense::hover());
                draw_lane(
                    ui,
                    rect,
                    &track.peaks,
                    dur,
                    pos,
                    track.frame_count,
                    track.sample_rate,
                    track.automation().as_ref(),
                );
            });
            ui.add_space(ROW_GAP);
        }
    });

    if let Some(i) = requested_correction {
        if app.project.tracks[i].correction.is_none() {
            let seed = app
                .profiles
                .iter()
                .find(|p| p.name == "Suno-Clean")
                .or_else(|| app.profiles.first())
                .cloned();
            app.project.tracks[i].correction = seed.clone();
            app.project_dirty = true;
            if let Some(player) = app.player.as_ref() {
                if let Some(track) = player.state.tracks.get(i) {
                    track.set_correction(seed);
                }
            }
        }
        app.editing_correction_for = Some(i);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_lane(
    ui: &mut egui::Ui,
    rect: Rect,
    peaks: &[f32],
    total_secs: f32,
    pos_secs: f32,
    track_frames: u64,
    sample_rate: u32,
    automation: Option<&crate::automation::AutomationLane>,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(10, 10, 14));
    if peaks.is_empty() {
        return;
    }

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
    painter.line_segment(
        [Pos2::new(rect.min.x, mid_y), Pos2::new(rect.max.x, mid_y)],
        Stroke::new(0.5, Color32::from_gray(40)),
    );

    // Automation curve (drawn semi-transparent under the playhead).
    if let Some(lane) = automation {
        if !lane.points.is_empty() {
            let auto_color = Color32::from_rgba_unmultiplied(230, 200, 80, 180);
            let cols = rect.width() as usize;
            let sampler = crate::automation::SplineSampler::build(lane);
            // Map dB → y: 0 dB at midline, +6 at top, -60 at bottom.
            let db_to_y = |db: f32| -> f32 {
                let n = ((db + 60.0) / 66.0).clamp(0.0, 1.0); // 0..1 from -60 to +6
                rect.max.y - n * rect.height()
            };
            let mut prev: Option<Pos2> = None;
            for x_px in 0..cols {
                let t = x_px as f32 / cols.max(1) as f32 * total_secs;
                if let Some(db) = sampler.sample(t) {
                    let p = Pos2::new(rect.min.x + x_px as f32, db_to_y(db));
                    if let Some(pv) = prev {
                        painter.line_segment([pv, p], Stroke::new(1.5, auto_color));
                    }
                    prev = Some(p);
                } else {
                    prev = None;
                }
            }
        }
    }

    // Synchronized playhead.
    let head_x = rect.min.x + rect.width() * (pos_secs / total_secs).clamp(0.0, 1.0);
    painter.line_segment(
        [Pos2::new(head_x, rect.min.y), Pos2::new(head_x, rect.max.y)],
        Stroke::new(1.5, Color32::from_rgb(230, 200, 80)),
    );
}

// ───────────────────── console deck ─────────────────────

fn console_deck(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    let n_tracks = match app.player.as_ref() {
        Some(p) => p.state.tracks.len(),
        None => return,
    };
    let mut commit_track: Option<usize> = None;
    let mut commit_master = false;

    egui::ScrollArea::horizontal().show(ui, |ui| {
        ui.horizontal(|ui| {
            for idx in 0..n_tracks {
                if strip(app, ui, idx) {
                    commit_track = Some(idx);
                }
                ui.add_space(STRIP_GAP);
            }
            ui.add_space(STRIP_GAP * 2.0);
            if master_strip(app, ui) {
                commit_master = true;
            }
        });
    });

    if let Some(i) = commit_track {
        commit_track_automation(app, i);
    }
    if commit_master {
        commit_master_automation(app);
    }
}

/// Returns true if the strip's R toggle was just turned OFF (caller
/// should commit the recorder's scratch lane for this track).
fn strip(app: &mut TinyBoothApp, ui: &mut egui::Ui, idx: usize) -> bool {
    // Clone the Arc so we can drop the immutable borrow on app before
    // any mutation. Cheap — Arc clone is two atomic ops.
    let track = match app.player.as_ref() {
        Some(p) => match p.state.tracks.get(idx) {
            Some(t) => t.clone(),
            None => return false,
        },
        None => return false,
    };

    let mut frame_color = Color32::from_rgb(22, 22, 26);
    if track.recording_armed.load(Ordering::Relaxed) {
        frame_color = Color32::from_rgb(70, 30, 30);
    } else if track.solo.load(Ordering::Relaxed) {
        frame_color = Color32::from_rgb(60, 50, 20);
    }

    let mut just_disarmed = false;
    egui::Frame::group(ui.style())
        .fill(frame_color)
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            ui.set_width(STRIP_W);
            // For a *vertical* slider, `spacing.slider_width` is the
            // main-axis (rail) length, not the thickness. Pin it to
            // FADER_H so the rail fills the bounding box `add_sized`
            // allocates below, instead of egui's default 100 px stub
            // floating in a 130 px box. Rail thickness comes from the
            // cross-axis allocation (rect.width() / 4 in egui), which
            // is already substantial at the current STRIP_W.
            // Scoped to this strip — sliders elsewhere keep defaults.
            ui.style_mut().spacing.slider_width = FADER_H;
            ui.vertical_centered(|ui| {
                let name = ellipsize(&track.name, STRIP_NAME_CHARS);
                ui.label(egui::RichText::new(name).size(FONT_STRIP_NAME).strong());
            });
            ui.add_space(3.0);
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    let mute = track.mute.load(Ordering::Relaxed);
                    if ui
                        .add_sized([26.0, 22.0], egui::SelectableLabel::new(mute, "M"))
                        .on_hover_text("Mute")
                        .clicked()
                    {
                        track.mute.store(!mute, Ordering::Relaxed);
                    }
                    let solo = track.solo.load(Ordering::Relaxed);
                    if ui
                        .add_sized([26.0, 22.0], egui::SelectableLabel::new(solo, "S"))
                        .on_hover_text("Solo")
                        .clicked()
                    {
                        track.solo.store(!solo, Ordering::Relaxed);
                    }
                    let armed = track.recording_armed.load(Ordering::Relaxed);
                    if ui
                        .add_sized([26.0, 22.0], egui::SelectableLabel::new(armed, "R"))
                        .on_hover_text("Arm — record fader gestures during playback")
                        .clicked()
                    {
                        let new_armed = !armed;
                        track.recording_armed.store(new_armed, Ordering::Relaxed);
                        if !new_armed {
                            just_disarmed = true;
                        }
                    }
                });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let mut gain = track.gain_db();
                let resp = ui.add_sized(
                    [STRIP_W - 30.0, FADER_H],
                    egui::Slider::new(&mut gain, -60.0..=6.0)
                        .vertical()
                        .show_value(false),
                );
                if resp.changed() {
                    track.set_gain_db(gain);
                }
                draw_meter(ui, track.peak(), FADER_H);
            });
            ui.add_space(3.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(format!("{:+.1} dB", track.gain_db()))
                        .size(FONT_STRIP_DB)
                        .monospace(),
                );
            });
        });
    let _ = app; // keep argument used for future expansion
    just_disarmed
}

/// Returns true if the master strip's R toggle was just turned OFF.
fn master_strip(app: &mut TinyBoothApp, ui: &mut egui::Ui) -> bool {
    // Clone the Arc<PlayerState> so we drop the immutable borrow on app
    // before any project-level mutation.
    let state = match app.player.as_ref() {
        Some(p) => p.state.clone(),
        None => return false,
    };

    let mut frame_color = Color32::from_rgb(28, 28, 36);
    if state.master_recording_armed.load(Ordering::Relaxed) {
        frame_color = Color32::from_rgb(80, 30, 30);
    }

    let mut just_disarmed = false;
    let mut new_master_db: Option<f32> = None;
    egui::Frame::group(ui.style())
        .fill(frame_color)
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            ui.set_width(STRIP_W + 12.0);
            // See the matching comment in `strip()`: this is rail length
            // for vertical sliders, not thickness.
            ui.style_mut().spacing.slider_width = FADER_H;
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("MASTER")
                        .size(FONT_MASTER_NAME)
                        .strong()
                        .color(Color32::from_rgb(230, 200, 80)),
                );
            });
            ui.add_space(3.0);
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    ui.add_sized([26.0, 22.0], egui::SelectableLabel::new(false, "M"))
                        .on_hover_text("Mute (no-op on bus)");
                    ui.add_sized([26.0, 22.0], egui::SelectableLabel::new(false, "S"))
                        .on_hover_text("Solo (no-op on bus)");
                    let armed = state.master_recording_armed.load(Ordering::Relaxed);
                    if ui
                        .add_sized([26.0, 22.0], egui::SelectableLabel::new(armed, "R"))
                        .on_hover_text("Arm — record master fader gestures")
                        .clicked()
                    {
                        let new_armed = !armed;
                        state
                            .master_recording_armed
                            .store(new_armed, Ordering::Relaxed);
                        if !new_armed {
                            just_disarmed = true;
                        }
                    }
                });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let mut gain = state.master_gain_db();
                let resp = ui.add_sized(
                    [STRIP_W - 30.0, FADER_H],
                    egui::Slider::new(&mut gain, -60.0..=6.0)
                        .vertical()
                        .show_value(false),
                );
                if resp.changed() {
                    state.set_master_gain_db(gain);
                    new_master_db = Some(gain);
                }
                ui.vertical(|ui| {
                    draw_meter(ui, state.master_peak_left(), FADER_H);
                });
                ui.vertical(|ui| {
                    draw_meter(ui, state.master_peak_right(), FADER_H);
                });
            });
            ui.add_space(3.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(format!("{:+.1} dB", state.master_gain_db()))
                        .size(FONT_STRIP_DB)
                        .monospace(),
                );
            });
        });
    if let Some(db) = new_master_db {
        app.project.master_gain_db = db;
        app.project_dirty = true;
    }
    just_disarmed
}

fn draw_meter(ui: &mut egui::Ui, peak: f32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(METER_W, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 1.0, Color32::from_rgb(15, 15, 18));
    let h = peak.clamp(0.0, 1.0) * rect.height();
    let filled = Rect::from_min_size(
        Pos2::new(rect.min.x, rect.max.y - h),
        egui::vec2(rect.width(), h),
    );
    let color = if peak > 0.9 {
        Color32::from_rgb(230, 80, 80)
    } else if peak > 0.7 {
        Color32::from_rgb(230, 200, 80)
    } else {
        Color32::from_rgb(100, 220, 150)
    };
    painter.rect_filled(filled, 1.0, color);
}

/// Truncate `name` to at most `cap` chars, appending `…` if any chars
/// were dropped. Operates on `chars()` so multi-byte UTF-8 names (accents,
/// emoji) won't panic the way `&name[..n]` byte-slicing would.
fn ellipsize(name: &str, cap: usize) -> String {
    let count = name.chars().count();
    if count <= cap {
        name.to_owned()
    } else {
        // cap counts the visible glyphs including the ellipsis itself.
        let keep = cap.saturating_sub(1);
        let head: String = name.chars().take(keep).collect();
        format!("{head}…")
    }
}

// ───────────────────── automation recorder hooks ─────────────────────

fn capture_automation(app: &mut TinyBoothApp) {
    let Some(player) = app.player.as_ref() else {
        return;
    };
    if player.state.play_state() != PlayState::Playing {
        return;
    }
    let t = player.state.position_secs();
    for (i, track) in player.state.tracks.iter().enumerate() {
        if track.recording_armed.load(Ordering::Relaxed) {
            app.recorder.record_track(i, t, track.gain_db());
        }
    }
    if player.state.master_recording_armed.load(Ordering::Relaxed) {
        app.recorder.record_master(t, player.state.master_gain_db());
    }
}

fn stop_and_commit_automation(app: &mut TinyBoothApp) {
    if let Some(p) = app.player.as_ref() {
        p.stop();
    }
    // Commit any in-flight scratch lanes from armed strips.
    let arm_idxs: Vec<usize> = if let Some(p) = app.player.as_ref() {
        p.state
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.recording_armed.load(Ordering::Relaxed))
            .map(|(i, _)| i)
            .collect()
    } else {
        Vec::new()
    };
    for i in arm_idxs {
        commit_track_automation(app, i);
    }
    let master_armed = app
        .player
        .as_ref()
        .map(|p| p.state.master_recording_armed.load(Ordering::Relaxed))
        .unwrap_or(false);
    if master_armed {
        commit_master_automation(app);
    }
}

fn commit_track_automation(app: &mut TinyBoothApp, idx: usize) {
    let lane = app.recorder.track_scratch.remove(&idx);
    if let Some(lane) = lane {
        if !lane.is_empty() {
            app.project.tracks[idx].gain_automation = Some(lane.clone());
            if let Some(p) = app.player.as_ref() {
                if let Some(t) = p.state.tracks.get(idx) {
                    t.set_automation(Some(lane));
                }
            }
            app.project_dirty = true;
        }
    }
}

fn commit_master_automation(app: &mut TinyBoothApp) {
    let lane = std::mem::take(&mut app.recorder.master_scratch);
    if !lane.is_empty() {
        app.project.master_gain_automation = Some(lane.clone());
        if let Some(p) = app.player.as_ref() {
            p.state.set_master_automation(Some(lane));
        }
        app.project_dirty = true;
    }
}

fn fmt_time(secs: f32) -> String {
    let total = secs.max(0.0) as u32;
    format!("{:02}:{:02}", total / 60, total % 60)
}
