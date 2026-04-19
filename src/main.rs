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

pub const APP_NAME: &str = "TinyBooth Sound Studio";
pub const APP_WINDOW_TITLE: &str = "TinyBooth Sound Studio";
pub const APP_GH_REPO: &str = "ophiocus/TinyBoothSoundStudio";

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title(APP_WINDOW_TITLE),
        ..Default::default()
    };

    eframe::run_native(
        APP_NAME,
        native_options,
        Box::new(|cc| Ok(Box::new(app::TinyBoothApp::new(cc)))),
    )
}
