#![windows_subsystem = "windows"]

mod analysis;
mod app;
mod audio;
mod automation;
mod cleanup;
mod coherence;
mod config;
mod dsp;
mod export;
mod git_update;
mod lufs;
mod manual;
mod player;
mod project;
mod suno_import;
mod suno_meta;
mod telemetry;
mod tib;
mod trim;
mod ui;
mod wav_meta;

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
            egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            }
        }
        Err(_) => egui::IconData::default(),
    }
}

/// Install a panic hook that appends the panic message + backtrace to
/// `%APPDATA%\TinyBooth Sound Studio\logs\panic.log`. Without this, a
/// panic in a GUI-subsystem build has nowhere to go — there's no
/// console, so the window just vanishes with no trace (see the v0.4.39
/// "session vanished with no log" investigation). Keeps the default
/// hook chained so behaviour under a debugger/console is unchanged.
fn install_panic_logger() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(dir) = dirs::data_dir() {
            let logs = dir.join(APP_NAME).join("logs");
            let _ = std::fs::create_dir_all(&logs);
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(logs.join("panic.log"))
            {
                use std::io::Write;
                let bt = std::backtrace::Backtrace::force_capture();
                let _ = writeln!(
                    f,
                    "=== panic @ {} (v{}) ===\n{info}\n--- backtrace ---\n{bt}\n",
                    chrono::Local::now().to_rfc3339(),
                    env!("APP_VERSION"),
                );
            }
        }
        default_hook(info);
    }));
}

fn main() -> eframe::Result<()> {
    install_panic_logger();

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
