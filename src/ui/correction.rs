//! Per-track Correction editor — floating window opened from the Mix
//! tab. Operates directly on `Project.tracks[i].correction`. Mutations
//! also flow into the player's matching `TrackPlay.correction_profile`
//! so the change is audible the next playback cycle (the audio thread
//! polls a generation counter to rebuild its local FilterChainStereo).

use crate::app::TinyBoothApp;
use crate::dsp::Profile;
use eframe::egui;

pub fn show(app: &mut TinyBoothApp, ctx: &egui::Context) {
    let Some(idx) = app.editing_correction_for else {
        return;
    };
    if idx >= app.project.tracks.len() {
        app.editing_correction_for = None;
        return;
    }

    let mut open = true;
    let title = format!("🪞  Correction · {}", app.project.tracks[idx].name);
    egui::Window::new(title)
        .open(&mut open)
        .default_size([520.0, 600.0])
        .min_size([460.0, 460.0])
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| body(app, idx, ui));
    if !open {
        app.editing_correction_for = None;
    }
}

fn body(app: &mut TinyBoothApp, idx: usize, ui: &mut egui::Ui) {
    // Disable / enable header.
    ui.horizontal(|ui| {
        let has_corr = app.project.tracks[idx].correction.is_some();
        ui.label(
            egui::RichText::new(if has_corr { "Active" } else { "Disabled" }).color(if has_corr {
                egui::Color32::from_rgb(100, 220, 150)
            } else {
                egui::Color32::DARK_GRAY
            }),
        );
        if has_corr {
            if ui.button("Disable correction").clicked() {
                app.project.tracks[idx].correction = None;
                app.project_dirty = true;
                push_to_player(app, idx, None);
                return;
            }
        } else if ui.button("Enable with Suno-Clean preset").clicked() {
            let seed = app
                .profiles
                .iter()
                .find(|p| p.name == "Suno-Clean")
                .cloned();
            app.project.tracks[idx].correction = seed.clone();
            app.project_dirty = true;
            push_to_player(app, idx, seed);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.weak("Edits apply to the next playback cycle.");
        });
    });
    ui.separator();

    let Some(p) = app.project.tracks[idx].correction.as_mut() else {
        ui.label("Correction is currently disabled — enable it above to edit a chain.");
        return;
    };

    // Save dirty if anything changed inside the editor.
    let mut any_changed = false;
    egui::ScrollArea::vertical().show(ui, |ui| {
        any_changed |= editor_body(p, ui);
    });

    if any_changed {
        app.project_dirty = true;
        let snapshot = app.project.tracks[idx].correction.clone();
        push_to_player(app, idx, snapshot);
    }
}

/// Bumps the matching player track's correction generation so the audio
/// thread rebuilds its FilterChainStereo on the next callback.
fn push_to_player(app: &mut TinyBoothApp, idx: usize, profile: Option<Profile>) {
    if let Some(player) = app.player.as_ref() {
        if let Some(track) = player.state.tracks.get(idx) {
            track.set_correction(profile);
        }
    }
}

fn editor_body(p: &mut Profile, ui: &mut egui::Ui) -> bool {
    // Shared body — same layout as the Admin window. Returns `true` when
    // any field changed in this frame; the caller bumps the player's
    // generation counter so playback picks up the edit.
    crate::ui::profile_editor::render(p, ui, true)
}
