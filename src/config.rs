use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const RECENT_CAP: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub dark_mode: bool,
    pub zoom: f32,
    /// Name of the recording-tone profile active at startup.
    #[serde(default = "default_profile_name")]
    pub active_profile: String,
    /// Manifest path of the project the user was working on at quit time.
    /// Auto-restored on next launch when still present.
    /// Added in v0.2.1; older configs default to None.
    #[serde(default)]
    pub last_project_path: Option<PathBuf>,
    /// Up to `RECENT_CAP` recently-opened project manifests, most recent
    /// first. Surfaced under File → Open Recent.
    /// Added in v0.2.1.
    #[serde(default)]
    pub recent_projects: Vec<PathBuf>,
}

fn default_profile_name() -> String { "Guitar".into() }

impl Default for Config {
    fn default() -> Self {
        Self {
            dark_mode: true,
            zoom: 1.0,
            active_profile: default_profile_name(),
            last_project_path: None,
            recent_projects: Vec::new(),
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

    /// Mark this project as the active one — sets `last_project_path` and
    /// prepends to `recent_projects` (deduping, capped to `RECENT_CAP`).
    /// Persists immediately so a crash mid-session doesn't lose the
    /// breadcrumb. Pass the absolute path to the `.tinybooth` manifest.
    pub fn record_project(&mut self, manifest_path: &Path) {
        let p = manifest_path.to_path_buf();
        self.last_project_path = Some(p.clone());
        self.recent_projects.retain(|x| x != &p);
        self.recent_projects.insert(0, p);
        if self.recent_projects.len() > RECENT_CAP {
            self.recent_projects.truncate(RECENT_CAP);
        }
        self.save();
    }

    /// Clear the recent-projects list.
    pub fn clear_recent(&mut self) {
        self.recent_projects.clear();
        self.save();
    }
}
