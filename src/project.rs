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

fn default_master_gain_db() -> f32 {
    0.0
}
fn default_next_suno_ordinal() -> u32 {
    1
}

/// What kind of source a track came from. Drives downstream UX (e.g. the
/// Clean tab can dispatch role-aware processing on Suno stems while
/// leaving Recorded takes alone).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TrackSource {
    /// Default — captured by TinyBooth's own Record tab.
    #[default]
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

impl Track {
    /// Construct a freshly-recorded track. Centralises the field list so
    /// future schema additions don't fan out to every literal call site.
    /// Pass `mode` from the Record tab; `recording_profile_snapshot` is
    /// the active recording-tone preset that produced the WAV.
    pub fn recorded(
        id: impl Into<String>,
        display_name: impl Into<String>,
        file_rel: impl Into<String>,
        sample_rate: u32,
        mode: crate::audio::SourceMode,
        duration_secs: f32,
        recording_profile_snapshot: crate::dsp::Profile,
    ) -> Self {
        let channel_source = match mode {
            crate::audio::SourceMode::Channel(c) => Some(c),
            _ => None,
        };
        Self {
            id: id.into(),
            name: display_name.into(),
            file: file_rel.into(),
            mute: false,
            gain_db: 0.0,
            sample_rate,
            channel_source,
            duration_secs,
            profile: Some(recording_profile_snapshot),
            stereo: matches!(mode, crate::audio::SourceMode::Stereo),
            source: TrackSource::Recorded,
            correction: None,
            gain_automation: None,
            polarity_inverted: false,
        }
    }

    /// Construct a track imported from a Suno stem bundle. Same role as
    /// [`Self::recorded`] — keeps the field-list fanout localised.
    ///
    /// The constructor's whole purpose is to absorb the field-list growth at
    /// a single site; routing the args through a parameter struct would only
    /// shift the fanout to the caller. The targeted allow is intentional.
    #[allow(clippy::too_many_arguments)]
    pub fn from_suno_stem(
        id: impl Into<String>,
        display_name: impl Into<String>,
        file_rel: impl Into<String>,
        sample_rate: u32,
        channels: u16,
        duration_secs: f32,
        role: StemRole,
        original_filename: String,
        session_epoch: Option<i64>,
        session_ordinal: Option<u32>,
        provenance: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: display_name.into(),
            file: file_rel.into(),
            mute: false,
            gain_db: 0.0,
            sample_rate,
            channel_source: None,
            duration_secs,
            profile: None,
            stereo: channels >= 2,
            source: TrackSource::SunoStem {
                role,
                original_filename,
                session_epoch,
                session_ordinal,
                provenance,
            },
            correction: None,
            gain_automation: None,
            polarity_inverted: false,
        }
    }
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
    /// **Recording-time snapshot** of the chain that was active when this
    /// take was captured (HPF/gate/comp/EQ/de-ess values frozen into the
    /// WAV — they're *baked in*, not applied at playback). Stored
    /// verbatim so the project file is self-contained even if the global
    /// `profiles.json` later changes. **Read-only after capture.**
    /// Distinct from [`Self::correction`] (post-processing chain applied
    /// at playback / export and editable any time).
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
    /// **Post-processing chain** applied at Mix-tab playback and at
    /// export mixdown. `None` = pass-through (track is mixed
    /// unprocessed). User-editable from the Mix tab's Correction window
    /// at any time; takes effect on the next playback cycle. Distinct
    /// from [`Self::profile`] (immutable recording-time snapshot).
    /// Added in v0.2; older manifests default to `None`.
    #[serde(default)]
    pub correction: Option<crate::dsp::Profile>,
    /// Recorded fader-gesture automation. Replayed on the audio thread
    /// via Catmull-Rom interpolation when present and not currently
    /// being re-recorded. Added in v0.3; older manifests default to None.
    #[serde(default)]
    pub gain_automation: Option<AutomationLane>,

    /// **Polarity flip** (a.k.a. phase inversion). When true, every
    /// sample of this track is multiplied by −1 before summing into the
    /// bus. Useful when a Suno stem arrives anti-phase relative to the
    /// other stems and disappears on summation. Implemented by folding
    /// the sign into the player's pre-cached static-gain on every
    /// buffer, so the hot path costs nothing extra.
    /// Added v0.4.0; older manifests default to false.
    #[serde(default)]
    pub polarity_inverted: bool,
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

    /// Persisted desire: when true, every track's correction chain is
    /// bypassed at playback and export, but the chain *config stays*.
    /// Toggle off to bring corrections back without losing tweaks.
    /// Survives reload. Added v0.3.4.
    #[serde(default)]
    pub corrections_disabled: bool,

    /// Project-level default correction profile. Used by "Enable all"
    /// as the seed when a track has no chain yet — cascade is:
    ///
    /// 1. existing track.correction (kept as-is if Some)
    /// 2. this project default (if Some)
    /// 3. feature default (Suno-Clean from builtin_profiles)
    ///
    /// Edit by hand in the manifest until a UI lands. Added v0.3.4.
    #[serde(default)]
    pub default_correction: Option<crate::dsp::Profile>,

    /// Path (relative to the project root) of the Suno mixdown WAV that
    /// came in the imported bundle. The mixdown is **not** added to
    /// `tracks` — it would double the audio if it were. Instead it's
    /// kept aside as a reference: the v0.4.0 import-time coherence
    /// check sums the stems and compares against this file, and v0.4.0
    /// phase 3 surfaces it as the auto-loaded reference for
    /// loudness-matched A/B from the Mix tab.
    /// `None` for non-Suno projects and for Suno bundles that didn't
    /// include a mixdown. Added v0.4.0.
    #[serde(default)]
    pub suno_mixdown_path: Option<String>,

    /// Integrated LUFS (BS.1770-4) of the Suno mixdown, computed once
    /// at import time. Used as the matched-loudness target for the
    /// Mix-tab reference A/B button — the mixdown plays at this
    /// loudness, the user's mix is gain-trimmed to match. `None` when
    /// no mixdown is present, or when LUFS computation failed.
    /// Added v0.4.0.
    #[serde(default)]
    pub suno_mixdown_lufs: Option<f32>,

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
            corrections_disabled: false,
            default_correction: None,
            suno_mixdown_path: None,
            suno_mixdown_lufs: None,
            root,
        }
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_NAME)
    }
    pub fn tracks_dir(&self) -> PathBuf {
        self.root.join(TRACKS_DIR)
    }

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
            .unwrap_or_default();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::{AutomationLane, AutomationPoint};

    fn fixture_track() -> Track {
        Track {
            id: "track-001".into(),
            name: "Vocals".into(),
            file: "tracks/track-001.wav".into(),
            mute: false,
            gain_db: -3.0,
            sample_rate: 48_000,
            channel_source: None,
            duration_secs: 47.2,
            profile: None,
            stereo: true,
            source: TrackSource::SunoStem {
                role: StemRole::Vocals,
                original_filename: "vocals.wav".into(),
                session_epoch: Some(1_777_095_097),
                session_ordinal: Some(1),
                provenance: Some("made with suno studio".into()),
            },
            correction: None,
            gain_automation: Some(AutomationLane {
                points: vec![
                    AutomationPoint {
                        time_secs: 0.0,
                        gain_db: -3.0,
                    },
                    AutomationPoint {
                        time_secs: 5.0,
                        gain_db: -1.5,
                    },
                    AutomationPoint {
                        time_secs: 10.0,
                        gain_db: -3.0,
                    },
                ],
            }),
            polarity_inverted: true,
        }
    }

    fn fixture_project() -> Project {
        Project {
            version: 1,
            name: "test session".into(),
            created: chrono::Utc::now(),
            tracks: vec![fixture_track()],
            master_gain_db: -1.5,
            master_gain_automation: None,
            next_suno_ordinal: 2,
            corrections_disabled: false,
            default_correction: None,
            suno_mixdown_path: Some(format!("{TRACKS_DIR}/mixdown.wav")),
            suno_mixdown_lufs: Some(-14.3),
            root: PathBuf::from("/tmp/test"),
        }
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let original = fixture_project();
        let json = serde_json::to_string_pretty(&original).expect("serialise");
        let restored: Project = serde_json::from_str(&json).expect("deserialise");

        // root is #[serde(skip)] so it's expected to come back as default.
        assert_eq!(original.version, restored.version);
        assert_eq!(original.name, restored.name);
        assert_eq!(original.master_gain_db, restored.master_gain_db);
        assert_eq!(original.next_suno_ordinal, restored.next_suno_ordinal);
        assert_eq!(original.corrections_disabled, restored.corrections_disabled);
        assert_eq!(original.tracks.len(), restored.tracks.len());

        let t0 = &restored.tracks[0];
        assert_eq!(t0.id, "track-001");
        assert_eq!(t0.name, "Vocals");
        assert_eq!(t0.gain_db, -3.0);
        assert!(t0.stereo);
        assert!(t0.gain_automation.is_some());
        assert_eq!(t0.gain_automation.as_ref().unwrap().points.len(), 3);

        match &t0.source {
            TrackSource::SunoStem {
                role,
                session_epoch,
                session_ordinal,
                provenance,
                original_filename,
            } => {
                assert_eq!(*role, StemRole::Vocals);
                assert_eq!(*session_epoch, Some(1_777_095_097));
                assert_eq!(*session_ordinal, Some(1));
                assert_eq!(provenance.as_deref(), Some("made with suno studio"));
                assert_eq!(original_filename, "vocals.wav");
            }
            _ => panic!("source should round-trip as SunoStem"),
        }
    }

    #[test]
    fn old_v0_1_manifest_loads_with_defaults() {
        // Minimal manifest as v0.1 would have written it — no stereo,
        // no profile, no source, no correction, no automation, no
        // master_gain_db, no next_suno_ordinal, no corrections_disabled,
        // no default_correction. All must default cleanly.
        let json = r#"{
            "version": 1,
            "name": "old session",
            "created": "2026-01-01T00:00:00Z",
            "tracks": [
                {
                    "id": "track-001",
                    "name": "take-1",
                    "file": "tracks/track-001.wav",
                    "mute": false,
                    "gain_db": 0.0,
                    "sample_rate": 48000,
                    "channel_source": null
                }
            ]
        }"#;
        let p: Project = serde_json::from_str(json).expect("v0.1 manifest must load");
        assert_eq!(p.tracks.len(), 1);
        assert_eq!(p.master_gain_db, 0.0);
        assert_eq!(p.next_suno_ordinal, 1);
        assert!(!p.corrections_disabled);
        assert!(p.default_correction.is_none());

        let t = &p.tracks[0];
        assert!(!t.stereo);
        assert!(t.profile.is_none());
        assert!(t.correction.is_none());
        assert!(t.gain_automation.is_none());
        // TrackSource defaults to Recorded.
        match &t.source {
            TrackSource::Recorded => {}
            _ => panic!("missing source field should default to Recorded"),
        }
    }
}
