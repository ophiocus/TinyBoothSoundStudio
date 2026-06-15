//! Crossfade tab — load two WAVs, position B's start with a slider,
//! visualize the overlap, preview each track independently and the
//! crossfade mix, export to any format `export.rs` supports.
//! TBSS-FR-0010.

use crate::app::{CrossfadeUiState, LoadedCrossfadeTrack, TinyBoothApp};
use crate::crossfade::{compute_mix, CrossfadeCurve, CrossfadeSpec};
use crate::crossfade_player::CrossfadePreviewSession;
use eframe::egui;
use std::path::{Path, PathBuf};

const PEAK_BINS: usize = 200;
const LANE_H: f32 = 60.0;

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
            .add_filter("WAV", &["wav"])
            .pick_file()
        {
            handle_load(app, &p, true);
        }
    }
    if b_clicked {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("WAV", &["wav"])
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
            );
            if ui.small_button("0").on_hover_text("Reset to 0 s").clicked() {
                app.crossfade_state.b_offset_secs = 0.0;
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
        });
    });

    ui.add_space(8.0);

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

fn draw_timeline(app: &TinyBoothApp, ui: &mut egui::Ui) {
    let st = &app.crossfade_state;
    let Some(a) = st.track_a.as_ref() else {
        ui.label(egui::RichText::new("Load Track A and Track B to see the timeline.").weak());
        return;
    };
    let Some(b) = st.track_b.as_ref() else {
        ui.label(egui::RichText::new("Load Track B to see the crossfade timeline.").weak());
        return;
    };

    // Timeline span in seconds.
    let off = st.b_offset_secs;
    let tl_start = 0.0_f32.min(off);
    let tl_end = a.duration_secs.max(off + b.duration_secs);
    let tl_dur = (tl_end - tl_start).max(0.001);

    let avail_w = ui.available_width().max(200.0);
    // Two stacked lanes.
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(avail_w, LANE_H * 2.0 + 6.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(14, 14, 18));

    let lane_a = egui::Rect::from_min_max(
        rect.left_top(),
        egui::pos2(rect.right(), rect.top() + LANE_H),
    );
    let lane_b = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + LANE_H + 6.0),
        rect.right_bottom(),
    );

    // Map (seconds_relative_to_tl_start) → pixel x.
    let secs_to_x = |s: f32| -> f32 { lane_a.left() + (s - tl_start) / tl_dur * lane_a.width() };

    // Overlap region (in tl-relative seconds).
    let overlap_start_secs = (0.0_f32).max(off);
    let overlap_end_secs = a.duration_secs.min(off + b.duration_secs);
    let has_overlap = overlap_end_secs > overlap_start_secs;
    if has_overlap {
        let x0 = secs_to_x(overlap_start_secs);
        let x1 = secs_to_x(overlap_end_secs);
        let overlap_rect = egui::Rect::from_min_max(
            egui::pos2(x0, lane_a.top()),
            egui::pos2(x1, lane_b.bottom()),
        );
        painter.rect_filled(
            overlap_rect,
            2.0,
            egui::Color32::from_rgba_unmultiplied(255, 200, 80, 30),
        );
    }

    // Draw each lane's waveform + label.
    draw_lane(&painter, lane_a, a, 0.0, tl_start, tl_dur, "A");
    draw_lane(&painter, lane_b, b, off, tl_start, tl_dur, "B");

    // Draw the fade curve over the overlap, faintly.
    if has_overlap {
        let mid_a = lane_a.center().y;
        let mid_b = lane_b.center().y;
        let h = LANE_H * 0.35;
        let x0 = secs_to_x(overlap_start_secs);
        let x1 = secs_to_x(overlap_end_secs);
        let width = (x1 - x0).max(1.0);
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
            let x = x0 + width * t;
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

fn draw_lane(
    painter: &egui::Painter,
    rect: egui::Rect,
    track: &LoadedCrossfadeTrack,
    secs_offset: f32,
    tl_start: f32,
    tl_dur: f32,
    label: &str,
) {
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(0.5, egui::Color32::from_gray(60)),
    );
    if track.peaks.is_empty() {
        return;
    }
    // Where the lane's audio sits on the timeline.
    let t_left = secs_offset;
    let t_right = secs_offset + track.duration_secs;
    let x_left = rect.left() + (t_left - tl_start) / tl_dur * rect.width();
    let x_right = rect.left() + (t_right - tl_start) / tl_dur * rect.width();
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
    painter.text(
        rect.left_top() + egui::vec2(6.0, 6.0),
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::monospace(11.0),
        egui::Color32::from_gray(160),
    );
}

fn handle_load(app: &mut TinyBoothApp, path: &Path, is_a: bool) {
    match load_wav_as_stereo(path) {
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
    match CrossfadePreviewSession::play(t.samples.clone(), t.sample_rate, t.channels) {
        Ok(s) => {
            st.preview = Some(s);
            st.status = Some(format!("Playing track {}", if is_a { "A" } else { "B" }));
        }
        Err(e) => {
            st.status = Some(format!("preview failed: {e:#}"));
        }
    }
}

fn start_preview_mix(st: &mut CrossfadeUiState) {
    stop_preview(st);
    let mix = match build_mix(st) {
        Ok(m) => m,
        Err(e) => {
            st.status = Some(format!("mix failed: {e:#}"));
            return;
        }
    };
    let sr = mix.sample_rate;
    match CrossfadePreviewSession::play(mix.samples, sr, 2) {
        Ok(s) => {
            st.preview = Some(s);
            st.status = Some("Playing crossfade".into());
        }
        Err(e) => {
            st.status = Some(format!("preview failed: {e:#}"));
        }
    }
}

fn stop_preview(st: &mut CrossfadeUiState) {
    st.preview = None;
}

fn build_mix(st: &CrossfadeUiState) -> anyhow::Result<crate::crossfade::CrossfadeMix> {
    use anyhow::anyhow;
    let a = st.track_a.as_ref().ok_or_else(|| anyhow!("no track A"))?;
    let b = st.track_b.as_ref().ok_or_else(|| anyhow!("no track B"))?;
    if a.sample_rate != b.sample_rate {
        return Err(anyhow!("sample-rate mismatch"));
    }
    let b_offset_frames = (st.b_offset_secs * a.sample_rate as f32).round() as i64;
    let spec = CrossfadeSpec {
        a_samples: &a.samples,
        b_samples: &b.samples,
        sample_rate: a.sample_rate,
        b_offset_frames,
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
