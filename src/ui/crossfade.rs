//! Crossfade tab — load two WAVs, position B's start with a slider,
//! visualize the overlap, preview each track independently and the
//! crossfade mix, export to any format `export.rs` supports.
//! TBSS-FR-0010.

use crate::app::{CrossfadePreviewMode, CrossfadeUiState, LoadedCrossfadeTrack, TinyBoothApp};
use crate::crossfade::{compute_mix, CrossfadeCurve, CrossfadeSpec};
use crate::crossfade_player::CrossfadePreviewSession;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

/// Pixel width of a fade-handle hit rect — small enough to leave most
/// of the timeline draggable as track B, large enough to reliably grab.
const HANDLE_HIT_W: f32 = 10.0;

const PEAK_BINS: usize = 200;
const LANE_H: f32 = 60.0;
/// Height of the zoom minimap strip drawn above the lanes. Tall enough
/// to grab, short enough not to crowd out the waveforms.
const ZOOM_STRIP_H: f32 = 16.0;
/// Gap between the strip and lane A (matches the gap between the lanes).
const STRIP_GAP: f32 = 4.0;

/// Resolve the visible time range given the current zoom state. When
/// `zoom_pct` is at the no-zoom sentinel (100), returns the full
/// `[tl_start, tl_end]`. Otherwise clamps `zoom_start_secs` so the
/// resolved range always fits inside the global timeline.
fn view_range(st: &CrossfadeUiState, tl_start: f32, tl_end: f32) -> (f32, f32) {
    let tl_dur = (tl_end - tl_start).max(0.001);
    let pct = st.zoom_pct.clamp(0.1, 100.0);
    if pct >= 100.0 - 0.0005 {
        return (tl_start, tl_end);
    }
    let view_span = (tl_dur * pct / 100.0).max(0.001).min(tl_dur);
    let max_start = tl_end - view_span;
    let view_start = st.zoom_start_secs.clamp(tl_start, max_start);
    (view_start, view_start + view_span)
}

/// Format a duration in `mm:ss.mmm` for the per-track playhead counter.
fn fmt_ms(secs: f32) -> String {
    let total_ms = (secs.max(0.0) * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_secs = total_ms / 1000;
    let s = total_secs % 60;
    let m = total_secs / 60;
    format!("{:02}:{:02}.{:03}", m, s, ms)
}

pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    ui.heading("Crossfade");
    ui.label(
        egui::RichText::new(
            "Load two WAVs, position B's start relative to A, listen to either \
             independently or to the crossfade mix, and export the result. \
             TBSS-FR-0010.",
        )
        .weak(),
    );
    ui.separator();

    // ── Source pickers ─────────────────────────────────────────────
    let mut a_clicked = false;
    let mut b_clicked = false;
    let mut a_clear = false;
    let mut b_clear = false;

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Track A:").strong());
        match &app.crossfade_state.track_a {
            Some(t) => {
                ui.monospace(t.path.file_name().and_then(|n| n.to_str()).unwrap_or("?"));
                ui.label(format!(
                    "{:.2}s · {} Hz · {} ch",
                    t.duration_secs, t.sample_rate, t.channels
                ));
                if ui.small_button("✖").on_hover_text("Unload").clicked() {
                    a_clear = true;
                }
            }
            None => {
                ui.label(egui::RichText::new("(none)").weak());
            }
        }
        if ui.button("Load…").clicked() {
            a_clicked = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Track B:").strong());
        match &app.crossfade_state.track_b {
            Some(t) => {
                ui.monospace(t.path.file_name().and_then(|n| n.to_str()).unwrap_or("?"));
                ui.label(format!(
                    "{:.2}s · {} Hz · {} ch",
                    t.duration_secs, t.sample_rate, t.channels
                ));
                if ui.small_button("✖").on_hover_text("Unload").clicked() {
                    b_clear = true;
                }
            }
            None => {
                ui.label(egui::RichText::new("(none)").weak());
            }
        }
        if ui.button("Load…").clicked() {
            b_clicked = true;
        }
    });

    if a_clicked {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("WAV or TinyBooth stem (.tib)", &["wav", "tib"])
            .add_filter("WAV", &["wav"])
            .add_filter("TinyBooth project (.tib)", &["tib"])
            .pick_file()
        {
            handle_load(app, &p, true);
        }
    }
    if b_clicked {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("WAV or TinyBooth stem (.tib)", &["wav", "tib"])
            .add_filter("WAV", &["wav"])
            .add_filter("TinyBooth project (.tib)", &["tib"])
            .pick_file()
        {
            handle_load(app, &p, false);
        }
    }
    if a_clear {
        app.crossfade_state.track_a = None;
        stop_preview(&mut app.crossfade_state);
    }
    if b_clear {
        app.crossfade_state.track_b = None;
        stop_preview(&mut app.crossfade_state);
    }

    // Guided-bounce modal — shown when the user picked a .tib that has
    // no `mix_run` row yet. TBSS-FR-0011 §C.
    render_bounce_flow_modal(app, ui.ctx());

    ui.add_space(8.0);

    // ── Offset + curve controls ────────────────────────────────────
    let have_both = app.crossfade_state.track_a.is_some() && app.crossfade_state.track_b.is_some();
    let rate_match = match (
        app.crossfade_state.track_a.as_ref(),
        app.crossfade_state.track_b.as_ref(),
    ) {
        (Some(a), Some(b)) => a.sample_rate == b.sample_rate,
        _ => true,
    };
    if have_both && !rate_match {
        ui.colored_label(
            egui::Color32::from_rgb(230, 120, 120),
            "Sample-rate mismatch — both tracks must be at the same Hz. \
             Re-export one of them and reload.",
        );
    }

    let (a_dur, b_dur) = match (
        app.crossfade_state.track_a.as_ref(),
        app.crossfade_state.track_b.as_ref(),
    ) {
        (Some(a), Some(b)) => (a.duration_secs, b.duration_secs),
        _ => (1.0, 1.0),
    };
    let min_off = -b_dur;
    let max_off = a_dur;
    ui.add_enabled_ui(have_both && rate_match, |ui| {
        ui.horizontal(|ui| {
            ui.label("B start offset:");
            ui.add(
                egui::Slider::new(&mut app.crossfade_state.b_offset_secs, min_off..=max_off)
                    .suffix(" s")
                    .clamp_to_range(true),
            )
            .on_hover_text("Or drag track B's waveform directly on the timeline.");
            if ui.small_button("0").on_hover_text("Reset to 0 s").clicked() {
                app.crossfade_state.b_offset_secs = 0.0;
            }
            if ui
                .button("Snap fade to overlap")
                .on_hover_text("Reset the fade region to span the entire overlap between A and B.")
                .clicked()
            {
                snap_fade_to_overlap(&mut app.crossfade_state);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Curve:");
            ui.radio_value(
                &mut app.crossfade_state.curve,
                CrossfadeCurve::EqualPower,
                "Equal-power",
            )
            .on_hover_text("cos²/sin² — sums to 1 in power. Right default for unrelated material.");
            ui.radio_value(
                &mut app.crossfade_state.curve,
                CrossfadeCurve::Linear,
                "Linear",
            )
            .on_hover_text(
                "Linear ramp — sums to 1 in amplitude. Right for phase-coherent material.",
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let fs = app.crossfade_state.fade_start_secs;
                let fe = app.crossfade_state.fade_end_secs;
                ui.label(
                    egui::RichText::new(format!(
                        "Fade: {:.2}s → {:.2}s ({:.2}s)",
                        fs,
                        fe,
                        (fe - fs).max(0.0)
                    ))
                    .monospace()
                    .weak(),
                );
            });
        });
    });

    ui.add_space(8.0);

    // ── Zoom controls ──────────────────────────────────────────────
    // Form fields are always visible (per spec); they work even when
    // not loaded — they just have nothing to act on. The X button
    // returns the view to "no zoom".
    ui.horizontal(|ui| {
        ui.label("Zoom start:");
        ui.add(
            egui::DragValue::new(&mut app.crossfade_state.zoom_start_secs)
                .speed(0.01)
                .suffix(" s"),
        )
        .on_hover_text("Left edge of the zoomed view, in timeline seconds.");
        ui.add_space(8.0);
        ui.label("Zoom %:");
        ui.add(
            egui::DragValue::new(&mut app.crossfade_state.zoom_pct)
                .speed(0.25)
                .suffix(" %"),
        )
        .on_hover_text(
            "Percentage of the full timeline visible. 100 % = no zoom. \
             Smaller % = more zoomed in (sub-second precision).",
        );
        // Hard clamp so the form can't break the view math.
        app.crossfade_state.zoom_pct = app.crossfade_state.zoom_pct.clamp(0.1, 100.0);
        let zoomed = app.crossfade_state.zoom_pct < 100.0 - 0.0005;
        ui.add_enabled_ui(zoomed, |ui| {
            if ui
                .button("✖ Reset zoom")
                .on_hover_text("Return to full-timeline view.")
                .clicked()
            {
                app.crossfade_state.zoom_pct = 100.0;
                app.crossfade_state.zoom_drag_anchor_secs = None;
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(if zoomed {
                    "drag on the strip above the lanes to set a new region"
                } else {
                    "drag on the strip above the lanes to zoom"
                })
                .weak()
                .small(),
            );
        });
    });

    ui.add_space(4.0);

    // ── Waveform visualisation ─────────────────────────────────────
    draw_timeline(app, ui);

    ui.add_space(8.0);

    // ── Transport ──────────────────────────────────────────────────
    let mut play_a = false;
    let mut play_b = false;
    let mut play_mix = false;
    let mut stop = false;
    let mut export = false;

    ui.horizontal(|ui| {
        ui.add_enabled_ui(app.crossfade_state.track_a.is_some(), |ui| {
            if ui
                .button("▶ A")
                .on_hover_text("Play track A start-to-end")
                .clicked()
            {
                play_a = true;
            }
        });
        ui.add_enabled_ui(app.crossfade_state.track_b.is_some(), |ui| {
            if ui
                .button("▶ B")
                .on_hover_text("Play track B start-to-end")
                .clicked()
            {
                play_b = true;
            }
        });
        ui.add_enabled_ui(have_both && rate_match, |ui| {
            if ui
                .button("▶ Crossfade")
                .on_hover_text("Play the full mixed timeline")
                .clicked()
            {
                play_mix = true;
            }
        });
        ui.add_enabled_ui(app.crossfade_state.preview.is_some(), |ui| {
            if ui.button("■ Stop").clicked() {
                stop = true;
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled_ui(have_both && rate_match, |ui| {
                if ui
                    .button("Export…")
                    .on_hover_text("Render the crossfade mix to a file (any supported format)")
                    .clicked()
                {
                    export = true;
                }
            });
            // Format picker.
            ui.label("Format:");
            egui::ComboBox::from_id_source("xfade_export_format")
                .selected_text(app.crossfade_state.export_format.label())
                .show_ui(ui, |ui| {
                    for fmt in crate::export::ExportFormat::all() {
                        ui.selectable_value(
                            &mut app.crossfade_state.export_format,
                            fmt,
                            fmt.label(),
                        );
                    }
                });
        });
    });

    // Drop the preview as soon as it's finished playing so the UI
    // reflects "stopped" without a manual click.
    if let Some(sess) = app.crossfade_state.preview.as_ref() {
        if sess.is_finished() {
            stop_preview(&mut app.crossfade_state);
        }
    }

    if play_a {
        start_preview_track(&mut app.crossfade_state, true);
    }
    if play_b {
        start_preview_track(&mut app.crossfade_state, false);
    }
    if play_mix {
        start_preview_mix(&mut app.crossfade_state);
    }
    if stop {
        stop_preview(&mut app.crossfade_state);
    }
    if export {
        do_export(app);
    }

    if let Some(msg) = app.crossfade_state.status.clone() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(msg).monospace());
    }
}

fn draw_timeline(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    // Pull live playback position onto the playheads BEFORE we read
    // them for the lane lines/hit-rects, so they reflect the most
    // recent audio frame this UI tick.
    sync_playheads_from_preview(&mut app.crossfade_state);

    // First read everything we need (so the immutable borrow ends
    // before we start grabbing &mut for the drag handlers).
    let (a_dur, b_dur, tl_start, tl_end) = {
        let st = &app.crossfade_state;
        let Some(a) = st.track_a.as_ref() else {
            ui.label(egui::RichText::new("Load Track A and Track B to see the timeline.").weak());
            return;
        };
        let Some(b) = st.track_b.as_ref() else {
            ui.label(egui::RichText::new("Load Track B to see the crossfade timeline.").weak());
            return;
        };
        let off = st.b_offset_secs;
        let tl_start = 0.0_f32.min(off);
        let tl_end = a.duration_secs.max(off + b.duration_secs);
        (a.duration_secs, b.duration_secs, tl_start, tl_end)
    };
    let tl_dur = (tl_end - tl_start).max(0.001);
    // Keep playheads in their tracks' valid range. b_offset/duration
    // change at runtime; the playheads need to follow.
    app.crossfade_state.a_playhead_secs = app.crossfade_state.a_playhead_secs.clamp(0.0, a_dur);
    app.crossfade_state.b_playhead_secs = app.crossfade_state.b_playhead_secs.clamp(0.0, b_dur);
    // While a preview is running, ask for a continuous repaint so the
    // playhead moves smoothly instead of waiting for cursor activity.
    if app.crossfade_state.preview.is_some() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(16));
    }

    // Resolve the visible time range from the current zoom state. All
    // downstream pixel math uses this — handles, playheads, waveforms
    // all zoom for free.
    let (view_start, view_end) = view_range(&app.crossfade_state, tl_start, tl_end);
    let view_dur = (view_end - view_start).max(0.001);

    let avail_w = ui.available_width().max(200.0);
    let total_h = ZOOM_STRIP_H + STRIP_GAP + LANE_H * 2.0 + 6.0;
    let (rect, _outer) = ui.allocate_exact_size(egui::vec2(avail_w, total_h), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(14, 14, 18));

    let strip_rect = egui::Rect::from_min_max(
        rect.left_top(),
        egui::pos2(rect.right(), rect.top() + ZOOM_STRIP_H),
    );
    let lanes_top = rect.top() + ZOOM_STRIP_H + STRIP_GAP;
    let lane_a = egui::Rect::from_min_max(
        egui::pos2(rect.left(), lanes_top),
        egui::pos2(rect.right(), lanes_top + LANE_H),
    );
    let lane_b = egui::Rect::from_min_max(
        egui::pos2(rect.left(), lanes_top + LANE_H + 6.0),
        rect.right_bottom(),
    );
    let lanes_rect = egui::Rect::from_min_max(lane_a.left_top(), lane_b.right_bottom());
    let px_per_sec = lanes_rect.width() / view_dur;
    let secs_to_x = |s: f32| -> f32 { lanes_rect.left() + (s - view_start) * px_per_sec };

    // ── Zoom strip (minimap + rubber-band drag) ────────────────────
    // Drag here selects the new view range. Always drawn at the full
    // [tl_start, tl_end] scale so it doubles as an overview minimap.
    let strip_px_per_sec = strip_rect.width() / tl_dur;
    let strip_x_to_secs =
        |x: f32| -> f32 { tl_start + ((x - strip_rect.left()) / strip_px_per_sec) };
    let strip_secs_to_x = |s: f32| -> f32 { strip_rect.left() + (s - tl_start) * strip_px_per_sec };
    painter.rect_filled(strip_rect, 2.0, egui::Color32::from_rgb(22, 22, 28));
    painter.rect_stroke(
        strip_rect,
        2.0,
        egui::Stroke::new(0.5, egui::Color32::from_gray(60)),
    );
    let strip_resp = ui.interact(
        strip_rect,
        ui.id().with("xfade_zoom_strip"),
        egui::Sense::click_and_drag(),
    );
    if strip_resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if strip_resp.drag_started() {
        if let Some(p) = strip_resp.interact_pointer_pos() {
            let t = strip_x_to_secs(p.x).clamp(tl_start, tl_end);
            app.crossfade_state.zoom_drag_anchor_secs = Some(t);
        }
    }
    if strip_resp.drag_stopped() {
        if let (Some(anchor), Some(p)) = (
            app.crossfade_state.zoom_drag_anchor_secs,
            strip_resp.interact_pointer_pos(),
        ) {
            let t = strip_x_to_secs(p.x).clamp(tl_start, tl_end);
            let (a, b) = if anchor <= t {
                (anchor, t)
            } else {
                (t, anchor)
            };
            let span = (b - a).max(0.0);
            // Require ≥ 5 ms drag to count as a zoom request — a stray
            // click on the strip shouldn't collapse the view to a point.
            if span >= 0.005 {
                app.crossfade_state.zoom_start_secs = a;
                app.crossfade_state.zoom_pct = (span / tl_dur * 100.0).clamp(0.1, 100.0);
            }
        }
        app.crossfade_state.zoom_drag_anchor_secs = None;
    }

    // Strip overlay: current view box (when zoomed) or rubber band (during drag).
    if let Some(anchor) = app.crossfade_state.zoom_drag_anchor_secs {
        if let Some(p) = ui.ctx().input(|i| i.pointer.hover_pos()) {
            let cur = strip_x_to_secs(p.x).clamp(tl_start, tl_end);
            let (a, b) = if anchor <= cur {
                (anchor, cur)
            } else {
                (cur, anchor)
            };
            let r = egui::Rect::from_min_max(
                egui::pos2(strip_secs_to_x(a), strip_rect.top() + 1.0),
                egui::pos2(strip_secs_to_x(b), strip_rect.bottom() - 1.0),
            );
            painter.rect_filled(
                r,
                1.0,
                egui::Color32::from_rgba_unmultiplied(120, 200, 255, 70),
            );
            painter.rect_stroke(
                r,
                1.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 200, 255)),
            );
        }
    } else if app.crossfade_state.zoom_pct < 100.0 - 0.0005 {
        let r = egui::Rect::from_min_max(
            egui::pos2(strip_secs_to_x(view_start), strip_rect.top() + 1.0),
            egui::pos2(strip_secs_to_x(view_end), strip_rect.bottom() - 1.0),
        );
        painter.rect_filled(
            r,
            1.0,
            egui::Color32::from_rgba_unmultiplied(120, 200, 255, 45),
        );
        painter.rect_stroke(
            r,
            1.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 200, 255)),
        );
    }
    // Strip-side waveform shadow — single thin line per track's range
    // so the user can see WHERE the audio is on the global timeline.
    let strip_mid = strip_rect.center().y;
    let a_x0 = strip_secs_to_x(0.0).max(strip_rect.left());
    let a_x1 = strip_secs_to_x(a_dur).min(strip_rect.right());
    let off_secs = app.crossfade_state.b_offset_secs;
    let b_x0 = strip_secs_to_x(off_secs).max(strip_rect.left());
    let b_x1 = strip_secs_to_x(off_secs + b_dur).min(strip_rect.right());
    let shadow_a = egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 200, 130));
    let shadow_b = egui::Stroke::new(1.0, egui::Color32::from_rgb(170, 130, 200));
    if a_x1 > a_x0 {
        painter.line_segment(
            [
                egui::pos2(a_x0, strip_mid - 3.0),
                egui::pos2(a_x1, strip_mid - 3.0),
            ],
            shadow_a,
        );
    }
    if b_x1 > b_x0 {
        painter.line_segment(
            [
                egui::pos2(b_x0, strip_mid + 3.0),
                egui::pos2(b_x1, strip_mid + 3.0),
            ],
            shadow_b,
        );
    }

    // Strip-side playhead markers — visible regardless of zoom so the
    // user can always see where each head is on the global timeline.
    // A points DOWN from the top edge; B points UP from the bottom edge.
    let a_ph_strip_x = strip_secs_to_x(app.crossfade_state.a_playhead_secs);
    let b_ph_strip_x =
        strip_secs_to_x(app.crossfade_state.b_offset_secs + app.crossfade_state.b_playhead_secs);
    let mk = 4.0;
    if a_ph_strip_x >= strip_rect.left() - 1.0 && a_ph_strip_x <= strip_rect.right() + 1.0 {
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(a_ph_strip_x, strip_rect.top() + mk),
                egui::pos2(a_ph_strip_x - mk, strip_rect.top()),
                egui::pos2(a_ph_strip_x + mk, strip_rect.top()),
            ],
            egui::Color32::from_rgb(120, 200, 255),
            egui::Stroke::NONE,
        ));
    }
    if b_ph_strip_x >= strip_rect.left() - 1.0 && b_ph_strip_x <= strip_rect.right() + 1.0 {
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(b_ph_strip_x, strip_rect.bottom() - mk),
                egui::pos2(b_ph_strip_x - mk, strip_rect.bottom()),
                egui::pos2(b_ph_strip_x + mk, strip_rect.bottom()),
            ],
            egui::Color32::from_rgb(120, 200, 255),
            egui::Stroke::NONE,
        ));
    }

    // ── Click/drag anywhere on a lane = seek that lane's playhead ─
    // Allocated BEFORE the fade handles so the handles' narrow hit
    // zones still win when overlapping. A click anywhere = jump
    // playhead to pointer; a drag = continuous seek. Either way the
    // active preview is stopped (DAW seek convention).
    let a_lane_resp = ui.interact(
        lane_a,
        ui.id().with("xfade_lane_a_seek"),
        egui::Sense::click_and_drag(),
    );
    let b_lane_resp = ui.interact(
        lane_b,
        ui.id().with("xfade_lane_b_seek"),
        egui::Sense::click_and_drag(),
    );
    if a_lane_resp.hovered() || b_lane_resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    let a_active = a_lane_resp.clicked() || a_lane_resp.dragged();
    if a_active {
        if let Some(p) = a_lane_resp.interact_pointer_pos() {
            stop_preview(&mut app.crossfade_state);
            let t = view_start + (p.x - lanes_rect.left()) / lanes_rect.width() * view_dur;
            app.crossfade_state.a_playhead_secs = t.clamp(0.0, a_dur);
        }
    }
    let b_active = b_lane_resp.clicked() || b_lane_resp.dragged();
    if b_active {
        if let Some(p) = b_lane_resp.interact_pointer_pos() {
            stop_preview(&mut app.crossfade_state);
            // B's playhead is track-local (0..b_dur). Convert pointer
            // time on the global timeline back into B-local seconds.
            let t_global = view_start + (p.x - lanes_rect.left()) / lanes_rect.width() * view_dur;
            let t_local = t_global - app.crossfade_state.b_offset_secs;
            app.crossfade_state.b_playhead_secs = t_local.clamp(0.0, b_dur);
        }
    }

    // ── Fade-region shading + draggable handles ────────────────────
    let fs = app.crossfade_state.fade_start_secs;
    let fe = app.crossfade_state.fade_end_secs;
    let fade_present = fe > fs;
    if fade_present {
        let x0 = secs_to_x(fs);
        let x1 = secs_to_x(fe);
        let fade_rect = egui::Rect::from_min_max(
            egui::pos2(x0.max(lanes_rect.left()), lane_a.top()),
            egui::pos2(x1.min(lanes_rect.right()), lane_b.bottom()),
        );
        painter.rect_filled(
            fade_rect,
            2.0,
            egui::Color32::from_rgba_unmultiplied(255, 200, 80, 40),
        );
    }

    let fs_x = secs_to_x(fs);
    let fe_x = secs_to_x(fe);
    // Hit zones confined to the lanes area so they don't bleed up into
    // the zoom strip (whose drag handler owns its own rect).
    let fs_rect = egui::Rect::from_min_max(
        egui::pos2(fs_x - HANDLE_HIT_W * 0.5, lanes_rect.top()),
        egui::pos2(fs_x + HANDLE_HIT_W * 0.5, lanes_rect.bottom()),
    );
    let fe_rect = egui::Rect::from_min_max(
        egui::pos2(fe_x - HANDLE_HIT_W * 0.5, lanes_rect.top()),
        egui::pos2(fe_x + HANDLE_HIT_W * 0.5, lanes_rect.bottom()),
    );
    let fs_resp = ui.interact(
        fs_rect,
        ui.id().with("xfade_handle_start"),
        egui::Sense::click_and_drag(),
    );
    let fe_resp = ui.interact(
        fe_rect,
        ui.id().with("xfade_handle_end"),
        egui::Sense::click_and_drag(),
    );
    if fs_resp.hovered() || fe_resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if fs_resp.dragged() && px_per_sec > 0.0 {
        let dx = fs_resp.drag_delta().x;
        if dx != 0.0 {
            let new = app.crossfade_state.fade_start_secs + dx / px_per_sec;
            app.crossfade_state.fade_start_secs =
                new.clamp(tl_start, app.crossfade_state.fade_end_secs);
        }
    }
    if fe_resp.dragged() && px_per_sec > 0.0 {
        let dx = fe_resp.drag_delta().x;
        if dx != 0.0 {
            let new = app.crossfade_state.fade_end_secs + dx / px_per_sec;
            app.crossfade_state.fade_end_secs =
                new.clamp(app.crossfade_state.fade_start_secs, tl_end);
        }
    }

    // ── Render lanes + waveforms + handle visuals ──────────────────
    let st = &app.crossfade_state;
    let a_ref = st.track_a.as_ref().unwrap();
    let b_ref = st.track_b.as_ref().unwrap();
    let off = st.b_offset_secs;
    draw_lane(
        &painter,
        lane_a,
        a_ref,
        0.0,
        view_start,
        view_dur,
        "A",
        st.a_playhead_secs,
    );
    draw_lane(
        &painter,
        lane_b,
        b_ref,
        off,
        view_start,
        view_dur,
        "B",
        st.b_playhead_secs,
    );

    // Vertical handle lines + tiny grip caps centred on the lanes.
    let edge_stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 200, 80));
    painter.line_segment(
        [
            egui::pos2(fs_x, lanes_rect.top()),
            egui::pos2(fs_x, lanes_rect.bottom()),
        ],
        edge_stroke,
    );
    painter.line_segment(
        [
            egui::pos2(fe_x, lanes_rect.top()),
            egui::pos2(fe_x, lanes_rect.bottom()),
        ],
        edge_stroke,
    );
    let cap = egui::vec2(8.0, 8.0);
    painter.rect_filled(
        egui::Rect::from_center_size(egui::pos2(fs_x, lanes_rect.center().y), cap),
        2.0,
        egui::Color32::from_rgb(255, 200, 80),
    );
    painter.rect_filled(
        egui::Rect::from_center_size(egui::pos2(fe_x, lanes_rect.center().y), cap),
        2.0,
        egui::Color32::from_rgb(255, 200, 80),
    );

    // Curve preview drawn over the fade range.
    if fade_present {
        let mid_a = lane_a.center().y;
        let mid_b = lane_b.center().y;
        let h = LANE_H * 0.35;
        let width = (fe_x - fs_x).max(1.0);
        let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 200, 80));
        let steps = (width as usize).max(8);
        let mut prev_a: Option<egui::Pos2> = None;
        let mut prev_b: Option<egui::Pos2> = None;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let (wa, wb) = match st.curve {
                CrossfadeCurve::EqualPower => {
                    let arg = std::f32::consts::PI * t * 0.5;
                    let ca = arg.cos();
                    let sa = arg.sin();
                    (ca * ca, sa * sa)
                }
                CrossfadeCurve::Linear => (1.0 - t, t),
            };
            let x = fs_x + width * t;
            let pa = egui::pos2(x, mid_a + h - wa * h * 2.0);
            let pb = egui::pos2(x, mid_b + h - wb * h * 2.0);
            if let Some(p) = prev_a {
                painter.line_segment([p, pa], stroke);
            }
            if let Some(p) = prev_b {
                painter.line_segment([p, pb], stroke);
            }
            prev_a = Some(pa);
            prev_b = Some(pb);
        }
    }
}

/// Reset the fade range to span the current A/B overlap. Called when
/// tracks are first loaded and via the "Snap fade to overlap" button.
fn snap_fade_to_overlap(st: &mut CrossfadeUiState) {
    let (Some(a), Some(b)) = (st.track_a.as_ref(), st.track_b.as_ref()) else {
        return;
    };
    let off = st.b_offset_secs;
    let overlap_start = 0.0_f32.max(off);
    let overlap_end = a.duration_secs.min(off + b.duration_secs);
    if overlap_end > overlap_start {
        st.fade_start_secs = overlap_start;
        st.fade_end_secs = overlap_end;
    } else {
        // No overlap — collapse to a point at A's end (instant cut).
        st.fade_start_secs = a.duration_secs;
        st.fade_end_secs = a.duration_secs;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_lane(
    painter: &egui::Painter,
    rect: egui::Rect,
    track: &LoadedCrossfadeTrack,
    secs_offset: f32,
    view_start: f32,
    view_dur: f32,
    label: &str,
    playhead_secs: f32,
) {
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(0.5, egui::Color32::from_gray(60)),
    );
    if !track.peaks.is_empty() {
        // Where the lane's audio sits on the timeline.
        let t_left = secs_offset;
        let t_right = secs_offset + track.duration_secs;
        let x_left = rect.left() + (t_left - view_start) / view_dur * rect.width();
        let x_right = rect.left() + (t_right - view_start) / view_dur * rect.width();
        let w = (x_right - x_left).max(1.0);
        let mid = rect.center().y;
        let half_h = (rect.height() * 0.4).max(2.0);
        let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 200, 130));
        let cols = w as usize;
        for x in 0..cols {
            let idx = ((x as f32 / cols.max(1) as f32) * track.peaks.len() as f32) as usize;
            let idx = idx.min(track.peaks.len() - 1);
            let p = track.peaks[idx].min(1.0);
            let xp = x_left + x as f32;
            painter.line_segment(
                [
                    egui::pos2(xp, mid - p * half_h),
                    egui::pos2(xp, mid + p * half_h),
                ],
                stroke,
            );
        }
    }

    // Playhead — a thin vertical line at the playhead's position on
    // the timeline. Drawn even when peaks is empty so the user still
    // has a grabbable target on degenerate (silent / very short) tracks.
    // When zoomed in such that the playhead sits off-screen, draw a
    // small triangle at the matching lane edge pointing OUTWARD so the
    // user can see which direction the head is in.
    let ph_x = rect.left() + (secs_offset + playhead_secs - view_start) / view_dur * rect.width();
    let ph_color = egui::Color32::from_rgb(120, 200, 255);
    if ph_x >= rect.left() - 1.0 && ph_x <= rect.right() + 1.0 {
        let ph_stroke = egui::Stroke::new(1.5, ph_color);
        painter.line_segment(
            [
                egui::pos2(ph_x, rect.top()),
                egui::pos2(ph_x, rect.bottom()),
            ],
            ph_stroke,
        );
        let cap = 5.0;
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(ph_x, rect.top() + cap),
                egui::pos2(ph_x - cap, rect.top()),
                egui::pos2(ph_x + cap, rect.top()),
            ],
            ph_color,
            egui::Stroke::NONE,
        ));
    } else {
        // Off-screen: edge marker. Left-pointing triangle on the right
        // edge means the head is past the right of view; right-pointing
        // on the left edge means it's behind the left of view.
        let mid_y = rect.center().y;
        let s = 6.0;
        if ph_x < rect.left() {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(rect.left() + 2.0, mid_y),
                    egui::pos2(rect.left() + 2.0 + s, mid_y - s),
                    egui::pos2(rect.left() + 2.0 + s, mid_y + s),
                ],
                ph_color,
                egui::Stroke::NONE,
            ));
        } else {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(rect.right() - 2.0, mid_y),
                    egui::pos2(rect.right() - 2.0 - s, mid_y - s),
                    egui::pos2(rect.right() - 2.0 - s, mid_y + s),
                ],
                ph_color,
                egui::Stroke::NONE,
            ));
        }
    }

    painter.text(
        rect.left_top() + egui::vec2(6.0, 6.0),
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::monospace(11.0),
        egui::Color32::from_gray(160),
    );
    // mm:ss.mmm counter — top-right of the lane, monospace so digits
    // don't jitter as they tick over.
    painter.text(
        rect.right_top() + egui::vec2(-6.0, 6.0),
        egui::Align2::RIGHT_TOP,
        fmt_ms(playhead_secs),
        egui::FontId::monospace(11.0),
        egui::Color32::from_rgb(180, 210, 235),
    );
}

/// Render the guided-bounce modal when [`crate::app::CrossfadeBounceFlow`]
/// is active. Three actions: Bounce as-is, Auto-apply Suno-Clean + Bounce,
/// Open project to tune (active project switch + Mix tab). TBSS-FR-0011 §C.
fn render_bounce_flow_modal(app: &mut TinyBoothApp, ctx: &egui::Context) {
    if app.crossfade_bounce_flow.is_none() {
        return;
    }
    // Snapshot what the buttons need before opening the Window, so the
    // closure borrows nothing mutable from `app`.
    let (project_name, track_count, n_without_corr, prev_error) = {
        let f = app.crossfade_bounce_flow.as_ref().unwrap();
        (
            f.project_name.clone(),
            f.track_count,
            f.n_without_corr,
            f.error.clone(),
        )
    };
    let mut click_as_is = false;
    let mut click_seed = false;
    let mut click_tune = false;
    let mut click_cancel = false;
    egui::Window::new("Bounce this project first")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(if project_name.is_empty() {
                    "(unnamed .tib)".to_string()
                } else {
                    project_name.clone()
                })
                .strong(),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} track{} · {} without a correction chain",
                    track_count,
                    if track_count == 1 { "" } else { "s" },
                    n_without_corr,
                ))
                .weak()
                .small(),
            );
            ui.separator();
            ui.label(
                "This .tib has no bounced mix yet. The Crossfade tab needs a single \
                 stem to load. Pick how to render it:",
            );
            ui.add_space(6.0);

            // 1) Bounce as-is — default focus.
            let as_is_resp = ui
                .add(
                    egui::Button::new(egui::RichText::new("⤓  Bounce as-is").strong())
                        .min_size(egui::vec2(280.0, 32.0)),
                )
                .on_hover_text(
                    "Render the master mix using each track's current correction chain. \
                     Transparent: tracks at correction = None stay uncorrected. \
                     Best when you've already tuned the project.",
                );
            if as_is_resp.clicked() {
                click_as_is = true;
            }
            ui.add_space(4.0);

            // 2) Auto-apply Suno-Clean — only enabled when there are
            //    uncorrected tracks to seed.
            ui.add_enabled_ui(n_without_corr > 0, |ui| {
                let label = if n_without_corr > 0 {
                    format!("✓  Apply Suno-Clean to {n_without_corr}, then Bounce")
                } else {
                    "✓  Apply Suno-Clean (nothing to seed)".to_string()
                };
                if ui
                    .add(egui::Button::new(label).min_size(egui::vec2(280.0, 32.0)))
                    .on_hover_text(
                        "Seed Suno-Clean (or the project's default correction) on every \
                         track currently without a chain, save the project, then bounce. \
                         Persistent — the corrections stay applied.",
                    )
                    .clicked()
                {
                    click_seed = true;
                }
            });
            ui.add_space(4.0);

            // 3) Open project to tune.
            if ui
                .add(
                    egui::Button::new("⋯  Open project to tune…").min_size(egui::vec2(280.0, 32.0)),
                )
                .on_hover_text(
                    "Switch to the Mix tab with this .tib as the active project. \
                     Tune corrections per-track, click Bounce there, then come back \
                     to Crossfade and reload.",
                )
                .clicked()
            {
                click_tune = true;
            }
            ui.add_space(8.0);

            if let Some(err) = prev_error.as_ref() {
                ui.colored_label(
                    egui::Color32::from_rgb(230, 120, 120),
                    format!("Last attempt failed: {err}"),
                );
            }
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        click_cancel = true;
                    }
                });
            });
        });

    // Apply the click (outside the window closure so `app` is borrowable).
    if click_cancel {
        app.crossfade_bounce_flow = None;
        return;
    }
    if click_tune {
        if let Some(flow) = app.crossfade_bounce_flow.take() {
            app.open_project_path(&flow.tib_path);
            app.tab = crate::app::Tab::Mix;
            app.status = Some(format!(
                "Opened {} — tune corrections, click Bounce, then reload from the Crossfade tab.",
                flow.tib_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("(unnamed)")
            ));
        }
        return;
    }
    if click_as_is || click_seed {
        let (path, is_a) = {
            let f = app.crossfade_bounce_flow.as_ref().unwrap();
            (f.tib_path.clone(), f.is_a)
        };
        match app.bounce_tib_for_crossfade(&path, click_seed) {
            Ok(()) => {
                app.crossfade_bounce_flow = None;
                // Re-run the normal load — `mix_run` is now populated,
                // so handle_load takes the existing-cache path.
                handle_load(app, &path, is_a);
            }
            Err(e) => {
                if let Some(f) = app.crossfade_bounce_flow.as_mut() {
                    f.error = Some(format!("{e:#}"));
                }
            }
        }
    }
}

fn handle_load(app: &mut TinyBoothApp, path: &Path, is_a: bool) {
    let is_tib = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("tib"))
        .unwrap_or(false);
    // For .tib: peek for a bounced mix_run row. Missing → open the
    // guided-bounce modal instead of erroring out. TBSS-FR-0011 §C.
    if is_tib {
        match probe_tib_for_bounce_flow(path) {
            Ok(TibProbe::HasMixRun) => { /* fall through to normal load */ }
            Ok(TibProbe::Empty {
                project_name,
                track_count,
                n_without_corr,
            }) => {
                app.crossfade_bounce_flow = Some(crate::app::CrossfadeBounceFlow {
                    tib_path: path.to_path_buf(),
                    is_a,
                    project_name,
                    track_count,
                    n_without_corr,
                    error: None,
                });
                return;
            }
            Err(e) => {
                app.crossfade_state.status = Some(format!("load failed: {e:#}"));
                return;
            }
        }
    }
    let loaded = if is_tib {
        load_tib_mix_run_as_stereo(path)
    } else {
        load_wav_as_stereo(path)
    };
    match loaded {
        Ok(loaded) => {
            // If the OTHER track is loaded at a different rate, reject.
            let other_rate = if is_a {
                app.crossfade_state.track_b.as_ref().map(|t| t.sample_rate)
            } else {
                app.crossfade_state.track_a.as_ref().map(|t| t.sample_rate)
            };
            if let Some(or) = other_rate {
                if or != loaded.sample_rate {
                    app.crossfade_state.status = Some(format!(
                        "sample-rate mismatch: this file is {} Hz, other track is {} Hz",
                        loaded.sample_rate, or
                    ));
                    return;
                }
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("(unnamed)")
                .to_string();
            app.crossfade_state.status =
                Some(format!("loaded {name} ({:.2} s)", loaded.duration_secs));
            if is_a {
                app.crossfade_state.track_a = Some(loaded);
            } else {
                app.crossfade_state.track_b = Some(loaded);
            }
            // Drop any preview — it's pointed at stale samples.
            stop_preview(&mut app.crossfade_state);
            // Snap the fade region to whatever overlap now exists.
            snap_fade_to_overlap(&mut app.crossfade_state);
        }
        Err(e) => {
            app.crossfade_state.status = Some(format!("load failed: {e:#}"));
        }
    }
}

fn load_wav_as_stereo(path: &Path) -> anyhow::Result<LoadedCrossfadeTrack> {
    use anyhow::Context as _;
    let reader =
        hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    decode_wav_reader_as_stereo(reader, path)
}

/// Cheap pre-flight on a `.tib` file: does it already have a bounced
/// `mix_run` row? If not, return the bits the bounce-flow modal needs
/// to render — project name, track count, count of tracks currently
/// at `correction = None` (used to disable the Auto-Suno-Clean button
/// when there's nothing to seed). TBSS-FR-0011 §C.
enum TibProbe {
    HasMixRun,
    Empty {
        project_name: String,
        track_count: usize,
        n_without_corr: usize,
    },
}

fn probe_tib_for_bounce_flow(path: &Path) -> anyhow::Result<TibProbe> {
    use anyhow::Context as _;
    let db = crate::tib::TibDb::open(path.to_path_buf())
        .with_context(|| format!("opening .tib {}", path.display()))?;
    if db.read_mix_run_header()?.is_some() {
        return Ok(TibProbe::HasMixRun);
    }
    let conn = db.conn();
    let project_name: String = conn
        .query_row("SELECT COALESCE(name, '') FROM meta", [], |r| r.get(0))
        .context("reading meta.name")?;
    let track_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
        .context("counting tracks")?;
    let n_without_corr: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tracks WHERE correction IS NULL",
            [],
            |r| r.get(0),
        )
        .context("counting tracks without correction")?;
    Ok(TibProbe::Empty {
        project_name,
        track_count: track_count.max(0) as usize,
        n_without_corr: n_without_corr.max(0) as usize,
    })
}

/// Open a `.tib` and decode its embedded `mix_run` WAV blob into the
/// same `LoadedCrossfadeTrack` shape the .wav path produces. Surfaces
/// a clear error when the project hasn't been bounced yet (Crossfade
/// can't stitch silence). TBSS-FR-0011 §B.
fn load_tib_mix_run_as_stereo(path: &Path) -> anyhow::Result<LoadedCrossfadeTrack> {
    use anyhow::{anyhow, Context as _};
    let db = crate::tib::TibDb::open(path.to_path_buf())
        .with_context(|| format!("opening .tib {}", path.display()))?;
    let bytes = db
        .read_mix_run_audio()
        .context("reading mix_run audio")?
        .ok_or_else(|| {
            anyhow!(
                "{} has no bounced mix yet — open the project in TinyBooth and click Bounce first",
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("this .tib")
            )
        })?;
    let reader =
        hound::WavReader::new(std::io::Cursor::new(bytes)).context("decoding mix_run WAV bytes")?;
    decode_wav_reader_as_stereo(reader, path)
}

/// Common WAV → interleaved-stereo-f32 path shared by the .wav and
/// .tib (mix_run blob) loaders. Forces stereo by duplicating mono and
/// keeps the source path on the returned struct for UI labels.
fn decode_wav_reader_as_stereo<R: std::io::Read>(
    reader: hound::WavReader<R>,
    path: &Path,
) -> anyhow::Result<LoadedCrossfadeTrack> {
    let spec = reader.spec();
    let channels = spec.channels.max(1);
    let sample_rate = spec.sample_rate;
    let frames = reader.duration() as usize;
    // Decode to i16 then scale, mirroring the player's tolerance for
    // 16/24-bit int and float.
    let samples_i16: Vec<i16> = match spec.sample_format {
        hound::SampleFormat::Int => {
            if spec.bits_per_sample == 16 {
                reader
                    .into_samples::<i16>()
                    .filter_map(|r| r.ok())
                    .collect()
            } else {
                reader
                    .into_samples::<i32>()
                    .filter_map(|r| r.ok())
                    .map(|s| s.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
                    .collect()
            }
        }
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .filter_map(|r| r.ok())
            .map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect(),
    };
    let denom = i16::MAX as f32;
    let mut stereo = Vec::with_capacity(frames * 2);
    for f in 0..frames {
        let base = f * channels as usize;
        if base + (channels as usize) > samples_i16.len() {
            break;
        }
        let l = samples_i16[base] as f32 / denom;
        let r = if channels >= 2 {
            samples_i16[base + 1] as f32 / denom
        } else {
            l
        };
        stereo.push(l);
        stereo.push(r);
    }
    let duration_secs = frames as f32 / sample_rate.max(1) as f32;
    let peaks = compute_peaks(&stereo, 2);
    Ok(LoadedCrossfadeTrack {
        path: path.to_path_buf(),
        samples: stereo,
        sample_rate,
        channels: 2, // we always store as stereo
        duration_secs,
        peaks,
    })
}

fn compute_peaks(samples: &[f32], channels: usize) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    let frames = samples.len() / channels.max(1);
    if frames == 0 {
        return Vec::new();
    }
    let frames_per_bin = frames.div_ceil(PEAK_BINS).max(1);
    let mut out = Vec::with_capacity(PEAK_BINS);
    for b in 0..PEAK_BINS {
        let f0 = b * frames_per_bin;
        let f1 = ((b + 1) * frames_per_bin).min(frames);
        let mut peak = 0.0_f32;
        for f in f0..f1 {
            for c in 0..channels {
                let s = samples[f * channels + c].abs();
                if s > peak {
                    peak = s;
                }
            }
        }
        out.push(peak.min(1.0));
    }
    out
}

fn start_preview_track(st: &mut CrossfadeUiState, is_a: bool) {
    stop_preview(st);
    let track = if is_a {
        st.track_a.as_ref()
    } else {
        st.track_b.as_ref()
    };
    let Some(t) = track else { return };
    // Start from the lane's own playhead (DAW seek-to-cursor). The
    // user can drag the playhead back to 0 if they want to replay
    // from the head.
    let playhead_secs = if is_a {
        st.a_playhead_secs
    } else {
        st.b_playhead_secs
    };
    let start_frame = (playhead_secs.max(0.0) * t.sample_rate as f32).round() as u64;
    match CrossfadePreviewSession::play(t.samples.clone(), t.sample_rate, t.channels, start_frame) {
        Ok(s) => {
            st.preview = Some(s);
            st.preview_mode = Some(if is_a {
                CrossfadePreviewMode::PlayA
            } else {
                CrossfadePreviewMode::PlayB
            });
            st.status = Some(format!("Playing track {}", if is_a { "A" } else { "B" }));
        }
        Err(e) => {
            st.status = Some(format!("preview failed: {e:#}"));
        }
    }
}

fn start_preview_mix(st: &mut CrossfadeUiState) {
    stop_preview(st);
    // Topmost (A) playhead drives the crossfade transport. Convert
    // its track-local time (A starts at global 0) to the mix output
    // origin, which is `min(0, b_offset)` — so when B starts before A,
    // the mix's frame 0 sits to the left of A's frame 0.
    let off = st.b_offset_secs;
    let tl_start = 0.0_f32.min(off);
    let global_secs = st.a_playhead_secs;
    let mix_secs = (global_secs - tl_start).max(0.0);
    let mix = match build_mix(st) {
        Ok(m) => m,
        Err(e) => {
            st.status = Some(format!("mix failed: {e:#}"));
            return;
        }
    };
    let sr = mix.sample_rate;
    let start_frame = (mix_secs * sr as f32).round() as u64;
    match CrossfadePreviewSession::play(mix.samples, sr, 2, start_frame) {
        Ok(s) => {
            st.preview = Some(s);
            st.preview_mode = Some(CrossfadePreviewMode::Mix);
            st.status = Some("Playing crossfade".into());
        }
        Err(e) => {
            st.status = Some(format!("preview failed: {e:#}"));
        }
    }
}

fn stop_preview(st: &mut CrossfadeUiState) {
    st.preview = None;
    st.preview_mode = None;
}

/// Pull the live cpal playback position out of the preview session and
/// onto the per-track playheads. No-op when nothing is playing.
///
/// - `PlayA` / `PlayB`: position is in the played track's frames, maps
///   directly to that track's playhead (other lane is left alone).
/// - `Mix`: position is in mix-output frames whose origin is
///   `min(0, b_offset)`. Each track's playhead is the global time minus
///   that track's start, clamped to its own [0, duration].
fn sync_playheads_from_preview(st: &mut CrossfadeUiState) {
    let Some(sess) = st.preview.as_ref() else {
        return;
    };
    let Some(mode) = st.preview_mode else {
        return;
    };
    let sr = sess.sample_rate.max(1) as f32;
    let pos_secs = sess.position.load(Ordering::Relaxed) as f32 / sr;
    match mode {
        CrossfadePreviewMode::PlayA => {
            if let Some(a) = st.track_a.as_ref() {
                st.a_playhead_secs = pos_secs.clamp(0.0, a.duration_secs);
            }
        }
        CrossfadePreviewMode::PlayB => {
            if let Some(b) = st.track_b.as_ref() {
                st.b_playhead_secs = pos_secs.clamp(0.0, b.duration_secs);
            }
        }
        CrossfadePreviewMode::Mix => {
            let off = st.b_offset_secs;
            let tl_start = 0.0_f32.min(off);
            let global = tl_start + pos_secs;
            if let Some(a) = st.track_a.as_ref() {
                st.a_playhead_secs = (global).clamp(0.0, a.duration_secs);
            }
            if let Some(b) = st.track_b.as_ref() {
                st.b_playhead_secs = (global - off).clamp(0.0, b.duration_secs);
            }
        }
    }
}

fn build_mix(st: &CrossfadeUiState) -> anyhow::Result<crate::crossfade::CrossfadeMix> {
    use anyhow::anyhow;
    let a = st.track_a.as_ref().ok_or_else(|| anyhow!("no track A"))?;
    let b = st.track_b.as_ref().ok_or_else(|| anyhow!("no track B"))?;
    if a.sample_rate != b.sample_rate {
        return Err(anyhow!("sample-rate mismatch"));
    }
    let sr = a.sample_rate;
    let b_offset_frames = (st.b_offset_secs * sr as f32).round() as i64;
    let fade_start_frame_abs = (st.fade_start_secs * sr as f32).round() as i64;
    let fade_end_frame_abs = (st.fade_end_secs * sr as f32).round() as i64;
    let spec = CrossfadeSpec {
        a_samples: &a.samples,
        b_samples: &b.samples,
        sample_rate: sr,
        b_offset_frames,
        fade_start_frame_abs,
        fade_end_frame_abs,
        curve: st.curve,
    };
    Ok(compute_mix(&spec))
}

fn do_export(app: &mut TinyBoothApp) {
    let mix = match build_mix(&app.crossfade_state) {
        Ok(m) => m,
        Err(e) => {
            app.crossfade_state.status = Some(format!("export failed: {e:#}"));
            return;
        }
    };
    // Default filename: <A>_x_<B>.<ext>
    let a_stem = app
        .crossfade_state
        .track_a
        .as_ref()
        .and_then(|t| t.path.file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("a")
        .to_string();
    let b_stem = app
        .crossfade_state
        .track_b
        .as_ref()
        .and_then(|t| t.path.file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("b")
        .to_string();
    let ext = app.crossfade_state.export_format.extension();
    let default_name = format!("{a_stem}_x_{b_stem}.{ext}");
    let Some(out) = rfd::FileDialog::new()
        .add_filter(app.crossfade_state.export_format.label(), &[ext])
        .set_file_name(&default_name)
        .save_file()
    else {
        return;
    };

    let opts = crate::export::ExportOptions {
        format: app.crossfade_state.export_format,
        bitrate_kbps: 192,
        out_path: out.clone(),
    };
    let sr = mix.sample_rate;
    match crate::export::write_crossfade(&mix.samples, sr, 2, &opts) {
        Ok(()) => {
            app.crossfade_state.status = Some(format!("Exported → {}", out.display()));
        }
        Err(e) => {
            app.crossfade_state.status = Some(format!("export failed: {e:#}"));
        }
    }
}

#[allow(dead_code)] // shape held for future tests
fn _hold_path_import(_: PathBuf) {}
