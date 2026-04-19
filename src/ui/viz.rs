//! Shared drawing primitives for the live visualizer (waveform + spectrum).

use eframe::egui;
use egui::{Color32, Pos2, Rect, Stroke};

use crate::analysis;

pub fn draw_waveform(ui: &mut egui::Ui, samples: &[f32], height: f32) {
    let desired = egui::vec2(ui.available_width(), height);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(10, 10, 14));

    if samples.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "no signal — press Record to start",
            egui::FontId::proportional(12.0),
            Color32::DARK_GRAY,
        );
        return;
    }

    let bins = rect.width() as usize;
    let peaks = analysis::peak_bins(samples, bins);
    let mid_y = rect.center().y;
    let gain = rect.height() * 0.45;
    let stroke = Stroke::new(1.0, Color32::from_rgb(100, 220, 150));
    for (i, p) in peaks.iter().enumerate() {
        let x = rect.min.x + i as f32;
        let h = p * gain;
        painter.line_segment([Pos2::new(x, mid_y - h), Pos2::new(x, mid_y + h)], stroke);
    }

    // Centre axis.
    painter.line_segment(
        [Pos2::new(rect.min.x, mid_y), Pos2::new(rect.max.x, mid_y)],
        Stroke::new(0.5, Color32::from_gray(40)),
    );
}

pub fn draw_spectrum(ui: &mut egui::Ui, samples: &[f32], height: f32) {
    let desired = egui::vec2(ui.available_width(), height);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(10, 10, 14));

    let bins = analysis::spectrum(samples);
    if bins.is_empty() {
        return;
    }
    // Collapse into the visible number of columns (one per 2 px).
    let cols = (rect.width() / 2.0).max(16.0) as usize;
    let cols = cols.min(bins.len());
    let per = bins.len() as f32 / cols as f32;
    for c in 0..cols {
        let start = (c as f32 * per) as usize;
        let end = ((c as f32 + 1.0) * per) as usize;
        let end = end.min(bins.len()).max(start + 1);
        let avg = bins[start..end].iter().copied().sum::<f32>() / (end - start) as f32;
        let h = avg * rect.height();
        let x = rect.min.x + (c as f32) * (rect.width() / cols as f32);
        let col_w = rect.width() / cols as f32 - 1.0;
        let r = Rect::from_min_size(
            Pos2::new(x, rect.max.y - h),
            egui::vec2(col_w.max(1.0), h),
        );
        let hue = (c as f32 / cols as f32 * 0.6 + 0.4).fract();
        let color = egui::ecolor::Hsva::new(hue, 0.7, 0.9, 1.0);
        painter.rect_filled(r, 1.0, color);
    }
}

pub fn draw_meter(ui: &mut egui::Ui, peak: f32) {
    let desired = egui::vec2(ui.available_width(), 8.0);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, Color32::from_rgb(30, 30, 36));
    let w = peak.clamp(0.0, 1.0) * rect.width();
    let filled = Rect::from_min_size(rect.min, egui::vec2(w, rect.height()));
    let color = if peak > 0.9 {
        Color32::from_rgb(230, 80, 80)
    } else if peak > 0.7 {
        Color32::from_rgb(230, 200, 80)
    } else {
        Color32::from_rgb(100, 220, 150)
    };
    painter.rect_filled(filled, 2.0, color);
}
