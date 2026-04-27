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

use crate::automation::AutomationLane;

pub const MANIFEST_NAME: &str = "project.tinybooth";
pub const TRACKS_DIR: &str = "tracks";

fn default_master_gain_db() -> f32 { 0.0 }
fn default_next_suno_ordinal() -> u32 { 1 }

/// What kind of source a track came from. Drives downstream UX (e.g. the
/// Clean tab can dispatch role-aware processing on Suno stems while
/// leaving Recorded takes alone).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TrackSource {
    /// Default — captured by TinyBooth's own Record tab.
    Recorded,
    /// Imported from a Suno stem bundle.
    SunoStem {
        role: StemRole,
        original_filename: String,
        /// Unix epoch seconds, parsed from the WAV's `LIST/INFO/ICMT`
        /// `created=<ISO>` field. Identical across every stem in one
        /// Suno render; distinct between re-renders. Sortable directly
        /// for "newest / oldest" ordering. Added v0.3.1.
        #[serde(default)]
        session_epoch: Option<i64>,
        /// Project-relative monotonically-increasing import index.
        /// All tracks from the same import event share an ordinal.
        /// Allows `ORDER BY session_ordinal` to surface most-recently-
        /// imported sessions first regardless of clock skew. Added v0.3.1.
        #[serde(default)]
        session_ordinal: Option<u32>,
        /// Free-form provenance string from the WAV (e.g. "made with
        /// suno studio"). Stored for the record. Added v0.3.1.
        #[serde(default)]
        provenance: Option<String>,
    },
}

impl Default for TrackSource {
    fn default() -> Self { Self::Recorded }
}

/// Stem identity inferred from a Suno bundle's filenames. Covers the
/// documented 12-stem set plus the legacy 2-stem export and a Master/
/// Unknown catch-all. Filename → `StemRole` matching is deliberately
/// loose (case-insensitive substring) — Suno's schema is not officially
/// published.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum StemRole {
    Vocals,
    BackingVocals,
    Drums,
    Bass,
    ElectricGuitar,
    AcousticGuitar,
    Keys,
    Synth,
    Pads,
    Strings,
    Brass,
    Percussion,
    FxOther,
    /// Legacy 2-stem export's non-vocal track.
    Instrumental,
    /// Some bundles include the rendered master alongside the stems.
    Master,
    Unknown,
}

impl StemRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vocals => "Vocals",
            Self::BackingVocals => "Backing Vocals",
            Self::Drums => "Drums",
            Self::Bass => "Bass",
            Self::ElectricGuitar => "Electric Guitar",
            Self::AcousticGuitar => "Acoustic Guitar",
            Self::Keys => "Keys",
            Self::Synth => "Synth / Lead",
            Self::Pads => "Pads / Chords",
            Self::Strings => "Strings",
            Self::Brass => "Brass / Wind",
            Self::Percussion => "Percussion",
            Self::FxOther => "FX / Other",
            Self::Instrumental => "Instrumental",
            Self::Master => "Master",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub name: String,
    /// Relative path from the manifest, e.g. `tracks/track-001.wav`.
    pub file: String,
    pub mute: bool,
    pub gain_db: f32,
    pub sample_rate: u32,
    /// For mono takes: which hardware channel (or `None` for mixdown).
    /// For stereo takes: `None` (captures channels 0 and 1 as L/R).
    pub channel_source: Option<u16>,
    #[serde(default)]
    pub duration_secs: f32,
    /// Snapshot of the recording-tone profile used when this take was captured.
    /// Stored verbatim so the project file is self-contained even if the
    /// presets file later changes.
    #[serde(default)]
    pub profile: Option<crate::dsp::Profile>,
    /// True when the underlying WAV has 2 channels (L/R).
    /// Added in v0.2; older manifests default to false (mono).
    #[serde(default)]
    pub stereo: bool,
    /// Where this track originated (recorded vs. imported Suno stem).
    /// Older manifests default to `Recorded`.
    #[serde(default)]
    pub source: TrackSource,
    /// Optional post-processing chain applied at Mix-tab playback and at
    /// export mixdown. `None` = pass-through (track is mixed unprocessed).
    /// Added in v0.2; older manifests default to `None`.
    #[serde(default)]
    pub correction: Option<crate::dsp::Profile>,
    /// Recorded fader-gesture automation. Replayed on the audio thread
    /// via Catmull-Rom interpolation when present and not currently
    /// being re-recorded. Added in v0.3; older manifests default to None.
    #[serde(default)]
    pub gain_automation: Option<AutomationLane>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    pub name: String,
    pub created: DateTime<Utc>,
    pub tracks: Vec<Track>,

    /// Master bus gain in dB, applied after the bus sum and before the
    /// soft-limit at both playback and export. Added v0.3; older
    /// manifests default to 0 dB.
    #[serde(default = "default_master_gain_db")]
    pub master_gain_db: f32,
    /// Master fader automation. Catmull-Rom replayed on the audio
    /// thread when present and not actively being re-recorded.
    #[serde(default)]
    pub master_gain_automation: Option<AutomationLane>,

    /// Monotonic counter for Suno-import ordinals. Bumped at each
    /// successful import; stamped onto every stem the import produced
    /// (via `TrackSource::SunoStem::session_ordinal`). Added v0.3.1.
    #[serde(default = "default_next_suno_ordinal")]
    pub next_suno_ordinal: u32,

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
            master_gain_db: 0.0,
            master_gain_automation: None,
            next_suno_ordinal: 1,
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
