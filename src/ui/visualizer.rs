//! Visualizer host shim — the engine itself now lives in the shared
//! `tbviz` crate (extracted so TinyBooth and TinyAmp share one
//! `VizModule` set). This file is the TinyBooth-specific wiring: it
//! pulls the master-bus sample tap off the live player and hands it to
//! `tbviz::show`, and bridges the engine's `close_requested` flag back
//! to `app.show_visualizer`.

use crate::app::TinyBoothApp;
use eframe::egui;

/// Re-export so existing `crate::ui::visualizer::VisualizerState`
/// references (and the `TinyBoothApp::visualizer` field) keep resolving.
pub use tbviz::VisualizerState;

/// Render the visualizer. Called from `app::update` when
/// `show_visualizer` is true.
pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    // Pull the master-bus tap + negotiated rate from the live player.
    let (samples, sample_rate): (Vec<(f32, f32)>, u32) = match app.player.as_ref() {
        Some(p) => (
            p.state.output_viz.lock().iter().copied().collect(),
            p.state.sample_rate,
        ),
        None => (Vec::new(), 0),
    };

    tbviz::show(&mut app.visualizer, ui, &samples, sample_rate);

    // The engine's Close button sets a flag; bridge it to the app.
    if app.visualizer.close_requested {
        app.visualizer.close_requested = false;
        app.show_visualizer = false;
    }
}
