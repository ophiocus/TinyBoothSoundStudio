//! TinyBooth project format (`*.tinybooth` — JSON manifest + sibling WAV tracks).
//!
//! Directory layout:
//! ```text
//!   my-session/
//!     project.tinybooth       # JSON manifest (this struct serialised)
//!     tracks/
//!       track-001.wav
//!       track-002.wav
//! ```

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MANIFEST_NAME: &str = "project.tinybooth";
pub const TRACKS_DIR: &str = "tracks";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub name: String,
    /// Relative path from the manifest, e.g. `tracks/track-001.wav`.
    pub file: String,
    pub mute: bool,
    pub gain_db: f32,
    pub sample_rate: u32,
    pub channel_source: Option<u16>,
    #[serde(default)]
    pub duration_secs: f32,
    /// Snapshot of the recording-tone profile used when this take was captured.
    /// Stored verbatim so the project file is self-contained even if the
    /// presets file later changes.
    #[serde(default)]
    pub profile: Option<crate::dsp::Profile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    pub name: String,
    pub created: DateTime<Utc>,
    pub tracks: Vec<Track>,

    /// Filled in at load time; not serialised.
    #[serde(skip)]
    pub root: PathBuf,
}

impl Project {
    pub fn new(name: impl Into<String>, root: PathBuf) -> Self {
        Self {
            version: 1,
            name: name.into(),
            created: Utc::now(),
            tracks: Vec::new(),
            root,
        }
    }

    pub fn manifest_path(&self) -> PathBuf { self.root.join(MANIFEST_NAME) }
    pub fn tracks_dir(&self) -> PathBuf { self.root.join(TRACKS_DIR) }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(&self.root).context("creating project dir")?;
        std::fs::create_dir_all(self.tracks_dir()).context("creating tracks dir")?;
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(self.manifest_path(), json).context("writing manifest")?;
        Ok(())
    }

    pub fn load(manifest: &Path) -> Result<Self> {
        let s = std::fs::read_to_string(manifest)
            .with_context(|| format!("reading {}", manifest.display()))?;
        let mut p: Project = serde_json::from_str(&s).context("parsing manifest")?;
        p.root = manifest
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(PathBuf::new);
        Ok(p)
    }

    /// Mint an unused `track-NNN` id and relative file path.
    pub fn new_track_slot(&self) -> (String, PathBuf) {
        let next = (1..=999)
            .map(|i| format!("track-{i:03}"))
            .find(|id| !self.tracks.iter().any(|t| &t.id == id))
            .unwrap_or_else(|| format!("track-{}", self.tracks.len() + 1));
        let file_rel = format!("{TRACKS_DIR}/{next}.wav");
        let abs = self.root.join(&file_rel);
        (next, abs)
    }

    pub fn track_abs_path(&self, track: &Track) -> PathBuf {
        self.root.join(&track.file)
    }
}
