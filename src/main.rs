#![windows_subsystem = "windows"]

mod analysis;
mod app;
mod audio;
mod config;
mod dsp;
mod export;
mod git_update;
mod project;
mod ui;

use eframe::egui;
use std::sync::Arc;

pub const APP_NAME: &str = "TinyBooth Sound Studio";
pub const APP_WINDOW_TITLE: &str = "TinyBooth Sound Studio";
pub const APP_GH_REPO: &str = "ophiocus/TinyBoothSoundStudio";

/// PNG bitmap embedded at compile time for the window's top-left icon.
/// 256×256 is plenty — Windows' DWM scales it down for titlebar/taskbar.
const VIEWPORT_ICON_PNG: &[u8] = include_bytes!("../assets/icon_viewport.png");

/// Decode the embedded icon into raw RGBA for egui. Falls back silently
/// if decoding ever fails — we'd rather ship without a custom icon than
/// refuse to start.
fn load_icon() -> egui::IconData {
    match image::load_from_memory(VIEWPORT_ICON_PNG) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();
            egui::IconData { rgba: rgba.into_raw(), width, height }
        }
        Err(_) => egui::IconData::default(),
    }
}

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title(APP_WINDOW_TITLE)
            .with_icon(Arc::new(load_icon())),
        ..Default::default()
    };

    eframe::run_native(
        APP_NAME,
        native_options,
        Box::new(|cc| Ok(Box::new(app::TinyBoothApp::new(cc)))),
    )
}
