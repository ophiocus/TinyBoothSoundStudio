//! Self-update via GitHub releases.
//!
//! Checks the latest release of `APP_GH_REPO`, compares 4-part semver against
//! `APP_VERSION` (set by build.rs from git tag), and, if newer, downloads the
//! first `.msi` asset and launches it elevated through PowerShell.

use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc;

#[derive(Debug, Clone)]
pub struct UpdateAvailable {
    pub version: String,
    pub url: String,
}

pub enum UpdateState {
    Idle,
    Checking,
    Available(UpdateAvailable),
    Downloading(mpsc::Receiver<Result<PathBuf, String>>),
}

fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> (u32, u32, u32, u32) {
        let mut p = s.split('.');
        let a = p.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let b = p.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let c = p.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let d = p.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        (a, b, c, d)
    };
    parse(latest) > parse(current)
}

pub fn check_latest_release() -> Option<UpdateAvailable> {
    let ua = format!("{}/{}", crate::APP_NAME, env!("APP_VERSION"));
    let client = reqwest::blocking::Client::builder()
        .user_agent(ua)
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;
    let url = format!("https://api.github.com/repos/{}/releases/latest", crate::APP_GH_REPO);
    let resp: serde_json::Value = client.get(url).send().ok()?.json().ok()?;
    let tag = resp["tag_name"].as_str()?.trim_start_matches('v').to_string();
    if !is_newer(&tag, env!("APP_VERSION")) {
        return None;
    }
    let dl = resp["assets"]
        .as_array()?
        .iter()
        .find(|a| a["name"].as_str().unwrap_or("").ends_with(".msi"))?
        ["browser_download_url"]
        .as_str()?
        .to_string();
    Some(UpdateAvailable { version: tag, url: dl })
}

fn download_and_install(url: &str, version: &str) -> Result<PathBuf, String> {
    let ua = format!("{}/{}", crate::APP_NAME, env!("APP_VERSION"));
    let client = reqwest::blocking::Client::builder()
        .user_agent(ua)
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))?;
    let bytes = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(|e| format!("Download: {e}"))?;

    let path = std::env::temp_dir().join(format!("{}-{version}.msi", crate::APP_NAME));
    std::fs::write(&path, &bytes).map_err(|e| format!("Write MSI: {e}"))?;

    let msi = path.to_string_lossy();
    std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Start-Process msiexec -ArgumentList '/i \"{msi}\" /passive /norestart' -Verb RunAs"
            ),
        ])
        .spawn()
        .map_err(|e| format!("Launch installer: {e}"))?;

    Ok(path)
}

pub fn render(
    ui: &mut egui::Ui,
    state: &mut UpdateState,
    error: &mut Option<String>,
    rx: &mut Option<mpsc::Receiver<Option<UpdateAvailable>>>,
) {
    // Drain background check result.
    if let Some(r) = rx.as_ref() {
        if let Ok(result) = r.try_recv() {
            *state = match result {
                Some(av) => UpdateState::Available(av),
                None => UpdateState::Idle,
            };
            *rx = None;
        }
    }
    // Drain download result.
    if let UpdateState::Downloading(r) = state {
        if let Ok(res) = r.try_recv() {
            match res {
                Ok(_) => std::process::exit(0),
                Err(e) => {
                    *error = Some(e);
                    *state = UpdateState::Idle;
                }
            }
        }
    }

    let label = format!("v{}", env!("APP_VERSION"));
    let response = ui.add(egui::Label::new(label).sense(egui::Sense::click()));
    if response.clicked() && matches!(state, UpdateState::Idle) {
        *state = UpdateState::Checking;
        let (tx, r) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(check_latest_release());
        });
        *rx = Some(r);
    }

    match state {
        UpdateState::Idle => {
            if let Some(e) = error.as_ref() {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            }
        }
        UpdateState::Checking => {
            ui.label("checking…");
        }
        UpdateState::Available(av) => {
            let msg = format!("v{} available — click to install", av.version);
            if ui.add(egui::Button::new(msg)).clicked() {
                let (tx, r) = mpsc::channel();
                let url = av.url.clone();
                let ver = av.version.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(download_and_install(&url, &ver));
                });
                *state = UpdateState::Downloading(r);
            }
        }
        UpdateState::Downloading(_) => {
            ui.label("downloading…");
        }
    }
}
