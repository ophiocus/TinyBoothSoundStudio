//! Project-trim panel (v0.4.0). Opened from the Project tab via the
//! "Trim project…" button. **Isolated** by design — does not weave
//! into the Mix tab. Single batch operation: pick a `[start, end]`
//! range in seconds and crop every WAV in the project (stems +
//! bundled Suno mixdown) to that range, atomically.
//!
//! Why a panel and not a tab: trim is rare-but-occasional ("the song
//! has 3 s of silence at the start"), not part of the moment-to-
//! moment mix workflow. Keeping it modal keeps the Mix tab
//! uncluttered and the action explicit.
//!
//! Why destructive (rewrites WAVs in place rather than recording
//! offsets): keeps the player engine, coherence analysis, and export
//! pipeline free of trim-aware special cases. Every WAV in the
//! project shares the same new frame-0, so coherence still works,
//! the player keeps reading from frame 0, and export sums what's on
//! disk. Tradeoff: not undoable from the app — re-import the bundle
//! to recover the originals.

use crate::app::TinyBoothApp;
use crate::trim;
use eframe::egui;

/// Panel state. Lives on `TinyBoothApp` so the panel survives Mix-tab
/// switches without losing the user's in-progress entries.
pub struct TrimState {
    /// Edit buffers backing the `mm:ss.mmm` text fields. Parsed to
    /// `f32` seconds via [`trim::parse_time_secs`] on Apply.
    pub start_text: String,
    pub end_text: String,
    /// Last result message shown in the status row.
    pub status: Option<String>,
    /// Cached reference-track peaks `(min, max)` pairs, computed once
    /// per project change and reused while the panel is open.
    cached_peaks: Vec<(f32, f32)>,
    cached_total_secs: f32,
    /// Project root the cache was built for; lets us invalidate when
    /// the user opens a different project without closing the panel.
    cached_for_root: Option<std::path::PathBuf>,
}

impl Default for TrimState {
    fn default() -> Self {
        Self {
            start_text: "00:00.000".into(),
            end_text: String::new(),
            status: None,
            cached_peaks: Vec::new(),
            cached_total_secs: 0.0,
            cached_for_root: None,
        }
    }
}

const PEAK_BIN_COUNT: usize = 600;
const WAVE_HEIGHT: f32 = 80.0;

pub fn show(app: &mut TinyBoothApp, ctx: &egui::Context) {
    if !app.show_trim {
        return;
    }
    let mut open = true;
    egui::Window::new("✂  Trim project")
        .open(&mut open)
        .default_size([720.0, 320.0])
        .min_size([520.0, 260.0])
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| body(app, ui));
    if !open {
        app.show_trim = false;
    }
}

fn body(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    if app.project.tracks.is_empty() && app.project.suno_mixdown_path.is_none() {
        ui.label("Open a project with at least one track or a bundled mixdown to use the trimmer.");
        return;
    }

    // Refresh the cached waveform when the active project changes.
    let cur_root = app.project.root.clone();
    if app.trim_state.cached_for_root.as_ref() != Some(&cur_root) {
        match trim::reference_waveform(&app.project, PEAK_BIN_COUNT) {
            Ok((peaks, total)) => {
                app.trim_state.cached_peaks = peaks;
                app.trim_state.cached_total_secs = total;
                app.trim_state.cached_for_root = Some(cur_root);
                if app.trim_state.end_text.is_empty() {
                    app.trim_state.end_text = trim::format_time_secs(total);
                }
            }
            Err(e) => {
                app.trim_state.status = Some(format!("could not load reference waveform: {e:#}"));
            }
        }
    }

    // ── Header info ──────────────────────────────────────────────
    let total_secs = app.trim_state.cached_total_secs;
    let total_str = trim::format_time_secs(total_secs);
    ui.label(
        egui::RichText::new(format!(
            "Reference: {} ({} tracks{}) — total length {}",
            reference_label(app),
            app.project.tracks.len(),
            if app.project.suno_mixdown_path.is_some() {
                " + mixdown"
            } else {
                ""
            },
            total_str
        ))
        .weak(),
    );
    ui.separator();

    // ── Waveform thumbnail ───────────────────────────────────────
    draw_waveform(app, ui);
    ui.add_space(6.0);

    // ── Start / End time entries ─────────────────────────────────
    let start_secs = trim::parse_time_secs(&app.trim_state.start_text);
    let end_secs = trim::parse_time_secs(&app.trim_state.end_text);
    let range_valid = match (start_secs, end_secs) {
        (Some(s), Some(e)) => s.is_finite() && e.is_finite() && e > s && s >= 0.0,
        _ => false,
    };

    egui::Grid::new("trim_grid")
        .num_columns(3)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Start");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut app.trim_state.start_text)
                    .desired_width(120.0)
                    .hint_text("00:00.000"),
            );
            if start_secs.is_none() && !app.trim_state.start_text.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(220, 90, 90), "unparseable");
            } else {
                ui.label(""); // spacer
            }
            let _ = resp;
            ui.end_row();

            ui.label("End");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut app.trim_state.end_text)
                    .desired_width(120.0)
                    .hint_text("00:00.000"),
            );
            if end_secs.is_none() && !app.trim_state.end_text.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(220, 90, 90), "unparseable");
            } else if let (Some(s), Some(e)) = (start_secs, end_secs) {
                if e <= s {
                    ui.colored_label(egui::Color32::from_rgb(220, 90, 90), "must be > start");
                } else if e > total_secs + 0.001 {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 200, 80),
                        format!("clamps to {}", trim::format_time_secs(total_secs)),
                    );
                } else {
                    ui.label("");
                }
            } else {
                ui.label("");
            }
            let _ = resp;
            ui.end_row();
        });

    if let (Some(s), Some(e)) = (start_secs, end_secs) {
        if range_valid {
            ui.label(
                egui::RichText::new(format!(
                    "Resulting length: {} (cuts {} from start, {} from end)",
                    trim::format_time_secs(e - s),
                    trim::format_time_secs(s),
                    trim::format_time_secs((total_secs - e).max(0.0)),
                ))
                .weak(),
            );
        }
    }

    ui.add_space(8.0);
    ui.separator();

    // ── Apply / cancel ───────────────────────────────────────────
    let mut click_apply = false;
    ui.horizontal(|ui| {
        let apply_btn = ui.add_enabled(
            range_valid,
            egui::Button::new("✂  Apply trim").min_size(egui::vec2(140.0, 28.0)),
        );
        if apply_btn.clicked() {
            click_apply = true;
        }
        if !range_valid {
            ui.weak("set a valid start < end");
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.weak(
                "Destructive — overwrites the WAVs in this project. \
                 Re-import the bundle if you need to undo.",
            );
        });
    });

    if click_apply {
        match (start_secs, end_secs) {
            (Some(s), Some(e)) => apply_trim(app, s, e),
            _ => {
                app.trim_state.status =
                    Some("Internal error: range vanished between validation and apply.".into());
            }
        }
    }

    // ── Status ───────────────────────────────────────────────────
    if let Some(msg) = app.trim_state.status.clone() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(msg).monospace());
    }
}

fn apply_trim(app: &mut TinyBoothApp, start_secs: f32, end_secs: f32) {
    // Trims happen on disk. Drop the player so its in-memory WAV
    // copies don't continue to play stale data; it'll re-load on the
    // next Mix-tab visit via the existing rebuild path.
    app.player = None;
    app.player_error = None;

    match trim::trim_project(&mut app.project, start_secs, end_secs) {
        Ok(report) => {
            // Reset markers to the new range and re-cache the waveform.
            app.trim_state.start_text = trim::format_time_secs(0.0);
            app.trim_state.end_text =
                trim::format_time_secs((report.end_secs - report.start_secs).max(0.0));
            app.trim_state.cached_for_root = None; // forces re-load on next frame
            app.project_dirty = true;
            // Save immediately so a crash before the next manual save
            // doesn't lose the manifest update; the WAVs on disk are
            // already mutated, so the manifest and disk drift if we
            // postpone.
            app.save_project();
            app.trim_state.status = Some(report.summary_line());
        }
        Err(e) => {
            app.trim_state.status = Some(format!("Trim failed: {e:#}"));
        }
    }
}

fn reference_label(app: &TinyBoothApp) -> String {
    if let Some(p) = app.project.suno_mixdown_path.as_ref() {
        format!("mixdown ({})", short_path(p))
    } else if let Some(t) = app.project.tracks.first() {
        format!("first track ({})", t.name)
    } else {
        "—".into()
    }
}

fn short_path(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(_, n)| n).unwrap_or(rel)
}

/// Render the cached peak table as a centred-baseline waveform with
/// translucent vertical markers at the parsed start / end times. The
/// markers update live as the user edits the time-entry boxes.
fn draw_waveform(app: &TinyBoothApp, ui: &mut egui::Ui) {
    let avail_w = ui.available_width().max(200.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(avail_w, WAVE_HEIGHT), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(18, 18, 22));

    if app.trim_state.cached_peaks.is_empty() || app.trim_state.cached_total_secs <= 0.0 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "no waveform",
            egui::FontId::proportional(12.0),
            egui::Color32::from_gray(120),
        );
        return;
    }

    let n = app.trim_state.cached_peaks.len();
    let mid_y = rect.center().y;
    let half_h = (rect.height() * 0.5) - 2.0;
    let bin_w = rect.width() / n as f32;
    let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 220, 150));
    for (i, (mn, mx)) in app.trim_state.cached_peaks.iter().enumerate() {
        let x = rect.min.x + (i as f32 + 0.5) * bin_w;
        let y_top = mid_y - mx.clamp(-1.0, 1.0) * half_h;
        let y_bot = mid_y - mn.clamp(-1.0, 1.0) * half_h;
        painter.line_segment(
            [egui::Pos2::new(x, y_top), egui::Pos2::new(x, y_bot)],
            stroke,
        );
    }
    // Centre line.
    painter.line_segment(
        [
            egui::Pos2::new(rect.min.x, mid_y),
            egui::Pos2::new(rect.max.x, mid_y),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
    );

    // Start / end markers.
    let total = app.trim_state.cached_total_secs;
    if let Some(s) = trim::parse_time_secs(&app.trim_state.start_text) {
        draw_marker(
            &painter,
            rect,
            s,
            total,
            egui::Color32::from_rgb(80, 160, 230),
        );
    }
    if let Some(e) = trim::parse_time_secs(&app.trim_state.end_text) {
        draw_marker(
            &painter,
            rect,
            e,
            total,
            egui::Color32::from_rgb(230, 160, 80),
        );
    }
}

fn draw_marker(
    painter: &egui::Painter,
    rect: egui::Rect,
    secs: f32,
    total: f32,
    color: egui::Color32,
) {
    if total <= 0.0 {
        return;
    }
    let frac = (secs / total).clamp(0.0, 1.0);
    let x = rect.min.x + frac * rect.width();
    painter.line_segment(
        [
            egui::Pos2::new(x, rect.min.y),
            egui::Pos2::new(x, rect.max.y),
        ],
        egui::Stroke::new(2.0, color),
    );
}
