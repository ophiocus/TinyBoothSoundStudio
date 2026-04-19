use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub dark_mode: bool,
    pub zoom: f32,
    /// Name of the recording-tone profile active at startup.
    #[serde(default = "default_profile_name")]
    pub active_profile: String,
}

fn default_profile_name() -> String { "Guitar".into() }

impl Default for Config {
    fn default() -> Self {
        Self {
            dark_mode: true,
            zoom: 1.0,
            active_profile: default_profile_name(),
        }
    }
}

impl Config {
    pub fn dir() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join(crate::APP_NAME))
    }

    pub fn path() -> Option<PathBuf> {
        Self::dir().map(|p| p.join("config.json"))
    }

    pub fn load() -> Self {
        let Some(p) = Self::path() else { return Self::default() };
        std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(dir) = Self::dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        if let Some(p) = Self::path() {
            if let Ok(s) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, s);
            }
        }
    }
}
