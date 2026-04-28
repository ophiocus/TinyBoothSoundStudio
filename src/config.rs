use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const RECENT_CAP: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub dark_mode: bool,
    /// Egui `set_zoom_factor` multiplier applied at startup. 1.0× is the
    /// baseline; users adjust via View → UI scale. Without `serde(default)`
    /// any pre-zoom config.json fails to parse and silently resets every
    /// other preference too — that's the bug this attribute prevents.
    #[serde(default = "default_zoom")]
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

fn default_profile_name() -> String {
    "Guitar".into()
}

fn default_zoom() -> f32 {
    1.0
}

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
        let Some(p) = Self::path() else {
            return Self::default();
        };
        std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Persist the config atomically: write to a sibling `.tmp` first,
    /// then rename over the canonical path. A crash or full disk during
    /// the write leaves either the old or the new file intact, never a
    /// truncated one. Returns the chained anyhow error so callers can
    /// surface a useful message.
    pub fn save(&self) -> Result<()> {
        let dir = Self::dir().context("no platform config directory available")?;
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating config dir {}", dir.display()))?;
        let path = Self::path().context("no platform config directory available")?;
        let tmp = path.with_extension("json.tmp");

        let json = serde_json::to_string_pretty(self).context("serialising config to JSON")?;

        std::fs::write(&tmp, json)
            .with_context(|| format!("writing config to {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))?;
        Ok(())
    }

    /// Save and surface any error to stderr — for the legacy callers
    /// that don't yet propagate. Prefer the `Result`-returning `save`
    /// when the caller has somewhere to show the message.
    pub fn save_or_log(&self) {
        if let Err(e) = self.save() {
            eprintln!("config save failed: {e:#}");
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
        self.save_or_log();
    }

    /// Clear the recent-projects list.
    pub fn clear_recent(&mut self) {
        self.recent_projects.clear();
        self.save_or_log();
    }
}
