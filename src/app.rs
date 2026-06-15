use crate::audio::{self, DeviceInfo, RecordingSession, SourceMode, VizState};
use crate::config::Config;
use crate::dsp::{self, Profile};
use crate::export::{self, ExportFormat};
use crate::git_update::{UpdateAvailable, UpdateState};
use crate::project::{Project, Track};
use crate::suno_import::{ImportKind, PendingImport};
use crate::tib::TibDb;
use crate::ui;
use anyhow::Context as _;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;

/// Which on-disk format backs the live project, and any open handles that
/// implies. **TBSS-FR-0007 phase 2c step 2 — plumbing only.** Constructed as
/// `Folder` everywhere; step 3 adds the `Tib` producer when the `.tib` open
/// path lands. Lives on [`TinyBoothApp`] rather than on `Project` because
/// `TibDb` owns a `rusqlite::Connection` (not serialisable, not `Clone`),
/// whereas `Project` is a plain serde struct.
pub enum ProjectBacking {
    Folder,
    Tib { db: TibDb },
}

/// A folder project the user opened, awaiting a migrate-or-keep choice.
/// Phase 2c nudges users onto the single-file `.tib` format: opening a
/// legacy `*.tinybooth` shows a modal offering to migrate. The folder
/// stays on disk either way (migration is additive). See
/// [`crate::ui::migrate_to_tib`].
pub struct PendingMigration {
    /// The `*.tinybooth` manifest the user picked.
    pub folder_manifest: PathBuf,
    /// Sibling `.tib` path the migration would write to.
    pub suggested_tib: PathBuf,
}

/// Draft state of the Add-Generator-Track modal (TBSS-FR-0009 step 5).
/// `mode` is mutated live by the modal as the user adjusts the picker
/// and per-mode fields; on commit, the track is created + baked.
pub struct PendingGeneratorParams {
    pub mode: crate::project::GeneratorMode,
}

/// One source loaded into the Crossfade tab — decoded once at load,
/// re-used for waveform render + preview + export. TBSS-FR-0010.
pub struct LoadedCrossfadeTrack {
    pub path: PathBuf,
    /// Interleaved stereo f32. Mono sources are duplicated to L=R at
    /// load time so downstream paths only ever see stereo.
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_secs: f32,
    /// Pre-computed peak vector for the waveform thumbnail (200 bins,
    /// abs-max per bin across both channels).
    pub peaks: Vec<f32>,
}

/// Which preview is currently driving `CrossfadeUiState::preview`. Maps
/// the live playback position onto the per-track playheads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossfadePreviewMode {
    PlayA,
    PlayB,
    Mix,
}

/// Crossfade-tab UI state — survives tab switches. TBSS-FR-0010.
pub struct CrossfadeUiState {
    pub track_a: Option<LoadedCrossfadeTrack>,
    pub track_b: Option<LoadedCrossfadeTrack>,
    /// Track B's start offset relative to A's frame 0, in seconds.
    /// Positive = B starts after A's start; negative = B starts before.
    pub b_offset_secs: f32,
    /// Absolute time (A's coordinate system) where the crossfade
    /// transition begins. Before this point, only A plays.
    pub fade_start_secs: f32,
    /// Absolute time where the crossfade transition ends. After this
    /// point, only B plays. The fade region is `[fade_start, fade_end)`.
    pub fade_end_secs: f32,
    pub curve: crate::crossfade::CrossfadeCurve,
    /// Active preview playback session. `Some` while audio is flowing;
    /// `None` between presses.
    pub preview: Option<crate::crossfade_player::CrossfadePreviewSession>,
    /// What kind of preview is currently in `preview`. `None` when no
    /// preview is active.
    pub preview_mode: Option<CrossfadePreviewMode>,
    /// Playhead position within Track A, in track-local seconds
    /// (0..a.duration_secs). Tracks the active preview when one is
    /// driving A, otherwise holds the user's last drag position.
    pub a_playhead_secs: f32,
    /// Playhead position within Track B, in track-local seconds.
    pub b_playhead_secs: f32,
    /// Left edge of the zoomed view, in global timeline seconds
    /// (origin = `min(0, b_offset)`). Honored only when `zoom_pct < 100`.
    pub zoom_start_secs: f32,
    /// Percentage of the total timeline visible. 100 = fully zoomed
    /// out (no zoom); smaller = more zoomed in. Clamped to ≥0.1 so the
    /// view never collapses to a single column.
    pub zoom_pct: f32,
    /// Timeline-seconds where the in-progress rubber-band drag started
    /// on the zoom strip. `None` between drags. Survives across frames
    /// inside a single drag so we can render the rubber band.
    pub zoom_drag_anchor_secs: Option<f32>,
    /// Reserved for future mix-result caching. The MVP recomputes the
    /// mix on every ▶ Crossfade press and Export click — fast enough
    /// for typical inputs.
    #[allow(dead_code)]
    pub mix_cache_signature: u64,
    pub status: Option<String>,
    /// Export format picker — same enum the Export tab uses.
    pub export_format: crate::export::ExportFormat,
}

impl Default for CrossfadeUiState {
    fn default() -> Self {
        Self {
            track_a: None,
            track_b: None,
            b_offset_secs: 0.0,
            fade_start_secs: 0.0,
            fade_end_secs: 0.0,
            curve: crate::crossfade::CrossfadeCurve::EqualPower,
            preview: None,
            preview_mode: None,
            a_playhead_secs: 0.0,
            b_playhead_secs: 0.0,
            zoom_start_secs: 0.0,
            zoom_pct: 100.0,
            zoom_drag_anchor_secs: None,
            mix_cache_signature: 0,
            status: None,
            export_format: crate::export::ExportFormat::Wav,
        }
    }
}

/// True when a track's telemetry is present and analyzed by the current
/// analyzer version (so a re-dispatch would be wasted work).
fn telemetry_is_current(t: &Track) -> bool {
    matches!(&t.telemetry, Some(tel) if tel.analyzer_version >= crate::telemetry::ANALYZER_VERSION)
}

/// Short stable tag derived from a project path — disambiguates telemetry
/// temp-WAV filenames so two open `.tib` projects that happen to share a
/// track id (e.g. both have `track-001`) can't collide in the temp dir.
fn root_tag(root: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut h);
    h.finish()
}

/// In-flight take metadata. Captured at `start_new_take` time so the
/// Record tab doesn't have to keep a `Project` struct alive for the
/// recordings filespace during the recording — that filespace is
/// loaded fresh on `start_new_take` (to mint a unique track id and
/// determine the rate constraint) and on `stop_take` (to append the
/// finished take). Single source of truth: the manifest on disk.
struct PendingTake {
    /// Recordings-project root the take is being captured into. The
    /// recording WAV at `target_root/file_rel` is written here.
    target_root: PathBuf,
    /// `track-NNN` id minted by the recordings project's
    /// `new_track_slot`. Becomes the `Track.id` on stop.
    track_id: String,
    /// Path relative to `target_root`, forward-slashed. Becomes
    /// `Track.file`.
    file_rel: String,
    /// User-typed track name (or the id, if blank).
    name: String,
    mode: SourceMode,
    /// Recording-time profile snapshot baked into the WAV; preserved
    /// on `Track.profile` for traceability.
    profile: Profile,
    /// Cpal-negotiated rate (matches the recordings project's existing
    /// rate when there is one — see the rate-enforcement check in
    /// `audio::start_recording`).
    sample_rate: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Record,
    Project,
    Mix,
    Export,
    /// Two-track crossfade preview + export. TBSS-FR-0010.
    Crossfade,
}

pub struct TinyBoothApp {
    pub config: Config,

    // Project state.
    pub project: Project,
    pub project_dirty: bool,
    /// Live storage backing for `project` — folder format today, `.tib`
    /// once step 3 lands. Holds the open SQLite handle for `.tib`
    /// projects so per-save SQL doesn't reopen the file each time.
    /// TBSS-FR-0007 phase 2c.
    pub backing: ProjectBacking,

    // Recording state (Record tab).
    pub devices: Vec<DeviceInfo>,
    pub selected_device: Option<String>,
    pub selected_mode: SourceMode,
    pub viz: Arc<VizState>,
    pub session: Option<RecordingSession>,
    /// Metadata for the currently-recording take (target filespace,
    /// minted id, etc.). Set in lockstep with `session`; both `Some`
    /// during recording, both `None` between takes. See [`PendingTake`].
    pending_take: Option<PendingTake>,
    pub pending_track_name: String,

    // Recording-tone profiles.
    pub profiles: Vec<Profile>,
    pub active_profile_idx: usize,
    pub show_admin: bool,
    pub admin_edit_idx: Option<usize>,
    pub admin_status: Option<String>,

    // Export state (Export tab).
    pub export_format: ExportFormat,
    pub export_bitrate: u32,
    pub export_busy: bool,
    pub export_msg: Option<String>,
    pub ffmpeg_available: bool,

    // UI.
    pub tab: Tab,
    pub status: Option<String>,
    pub show_manual: bool,
    pub manual_slug: String,
    pub md_cache: egui_commonmark::CommonMarkCache,

    // Multitrack player (None until the first time the Mix tab is opened
    // for a project, or when tracks change shape and we need to rebuild).
    pub player: Option<crate::player::Player>,
    pub player_error: Option<String>,
    /// Project root the most recent `Player::new` attempt failed on.
    /// Set when the rebuild fails; the Mix-tab lazy-rebuild guard
    /// checks this and skips re-attempting on every frame for the
    /// same project, which is the difference between "fans on full
    /// because we re-allocate 600 MB of WAV samples per render" and
    /// "fans idle, single error banner". Cleared on project change
    /// (so opening a different project re-attempts) or on explicit
    /// Retry click. v0.4.9.
    pub player_attempt_failed_for: Option<PathBuf>,
    /// In-flight async player build, keyed by the project root it's for.
    /// The build runs on a dedicated audio owner-thread (v0.4.39) and is
    /// **two-phase** (v0.4.40): the lanes render as soon as `Loaded`
    /// arrives (track state ready) while the output device is still being
    /// probed; `StreamReady` / `StreamFailed` follow. Polled each frame;
    /// stays `Some` until the stream resolves.
    pub player_pending: Option<(PathBuf, std::sync::mpsc::Receiver<crate::player::BuildMsg>)>,
    /// Index of the track whose Correction editor is open, if any.
    pub editing_correction_for: Option<usize>,

    /// Trim panel — opened from the Project tab, isolated modal.
    /// Survives Mix-tab switches without losing the user's in-progress
    /// time-entry state. Added v0.4.0.
    pub show_trim: bool,
    pub trim_state: crate::ui::trim::TrimState,

    /// Audio-reactive visualizer (v0.4.11). When `show_visualizer` is
    /// true the central panel is taken over by `ui::visualizer::show`,
    /// rendering one of four mathematically-grounded modes
    /// (Lissajous / Mandala / Lorenz / Chladni) driven by the
    /// master-bus sample tap. Toggled via the 🌀 icon in the top
    /// menu bar.
    pub show_visualizer: bool,
    pub visualizer: crate::ui::visualizer::VisualizerState,

    /// Modal dialog shown after every import attempt — success or fail.
    pub import_dialog: Option<crate::suno_import::ImportOutcome>,

    /// Pending Suno import waiting for user resolution because the
    /// target project root already contains a project with a matching
    /// session epoch. The conflict modal shows while this is `Some`.
    pub import_conflict: Option<PendingImport>,

    /// A folder project the user opened, awaiting the migrate-to-`.tib`
    /// prompt. The migrate modal shows while this is `Some`. TBSS-FR-0007
    /// phase 2c.
    pub pending_migration: Option<PendingMigration>,

    /// Draft state for the "Add Generator Track" modal. `Some` while
    /// the modal is open; cleared on commit / cancel. TBSS-FR-0009.
    pub pending_generator_modal: Option<PendingGeneratorParams>,

    /// Crossfade-tab state — sources, offset, curve, preview session.
    /// Survives tab switches. TBSS-FR-0010.
    pub crossfade_state: CrossfadeUiState,

    /// Mixer/automation recorder. Captures fader gestures while a strip's
    /// arm toggle is on and the player is in Playing state. Flushed into
    /// the project on Stop / disarm.
    pub recorder: crate::automation::Recorder,
    /// Resizable split — what fraction of the Mix tab's height is the
    /// console deck (vs. the multitrack lane area).
    pub mix_console_fraction: f32,

    /// Page index for the Record-tab "Recent recordings" list (10
    /// entries per page, newest first). Survives tab switches.
    pub recordings_page: usize,

    /// Cached thumbnail data per recording WAV, keyed by absolute
    /// path. Built lazily on first Record-tab render (sync UI-thread
    /// decode — acceptable MVP hitch). Carries both the peak vector
    /// (for rendering) and `duration_secs` (for converting drag-pixel-x
    /// to selection-seconds in Phase B). TBSS-FR-0008 item (4).
    pub recordings_peaks_cache:
        std::collections::HashMap<PathBuf, Arc<crate::ui::record::CachedThumb>>,

    /// Per-take selection range `(start_secs, end_secs)` for the
    /// recordings list's Export Selection action. Keyed by abs path.
    /// Lives in UI state only — not persisted. TBSS-FR-0008 item (4)
    /// Phase B/C.
    pub recordings_selection: std::collections::HashMap<PathBuf, (f32, f32)>,
    /// When the user hits ▶ on a recording entry, we swap `project`
    /// to the recordings project and queue this flag so the Mix-tab
    /// view starts playback automatically on its next render. Cleared
    /// once acted on. v0.4.0.
    pub mix_autoplay_pending: bool,
    /// Optional track index to solo on autoplay — the entry the user
    /// actually clicked. `None` = autoplay without changing solos.
    pub mix_autoplay_solo_idx: Option<usize>,

    // Self-update plumbing.
    pub update_state: UpdateState,
    pub update_error: Option<String>,
    pub update_rx: Option<mpsc::Receiver<Option<UpdateAvailable>>>,
    /// Last time we kicked off a background release check. The
    /// auto-recheck logic compares this against
    /// `git_update::RECHECK_INTERVAL` each frame; on overshoot
    /// (and idle state + no pending rx) a fresh check is dispatched.
    /// Added v0.4.23. `None` at startup so the very first frame after
    /// `new()` doesn't double-fire (the constructor already spawns one).
    pub last_update_check_at: Option<std::time::Instant>,
    /// Tab as of the previous frame — used to detect tab transitions
    /// and force an immediate re-check on the next frame. v0.4.23.
    pub last_tab_seen: Option<Tab>,

    // Audio-thread error channel. cpal's err_fn closures get a Sender;
    // every frame the UI thread drains the Receiver and surfaces the
    // most recent message into the status bar. Survival-guide §3.3:
    // never `eprintln!` from the audio thread.
    pub audio_err_tx: mpsc::Sender<String>,
    pub audio_err_rx: mpsc::Receiver<String>,

    /// Background telemetry analyzer (TBSS-FR-0005). Owns one worker
    /// thread; takes per-track analysis requests, ships results back
    /// via mpsc. The UI thread drains results in `update()` and
    /// patches them onto `app.project`. v0.4.13.
    pub telemetry: crate::telemetry::TelemetryService,

    /// User-tweakable analyzer thresholds (pick velocity, YIN
    /// tolerance, polyphony cutoff). Persisted to
    /// `telemetry_settings.json`. Edited via Admin → Telemetry
    /// settings…. Snapshotted into each `TelemetryRequest` at
    /// dispatch time so in-flight requests use the values that were
    /// active when they were queued. Added v0.4.14.
    pub telemetry_settings: crate::telemetry::TelemetrySettings,
    pub show_telemetry_settings: bool,

    /// Admin → Audio devices… modal. Lets the user pick the master
    /// input / output device. Added v0.4.27.
    pub show_audio_devices: bool,

    /// Set to `true` at construction; cleared on first `update()`
    /// frame after dispatching the initial backfill scan over the
    /// auto-restored project. Without this, the auto-restored
    /// project would never get analyzed because `new()` can't call
    /// methods on itself before returning. v0.4.13.
    pub initial_telemetry_pending: bool,

    /// Project Health panel (TBSS-FR-0005 §"Health"). Modal showing
    /// per-track telemetry weight, totals, and stale rows. v0.4.13.
    pub show_health: bool,

    /// Per-bin peak-decay trail for the Mix-tab spectrum panel.
    /// Length matches the FFT bin count. Each frame: trail[i] =
    /// max(current_db[i], trail[i] * 0.95) — fast-attack / slow-
    /// release peak hold. UI thread only; no audio-thread coupling.
    /// Added v0.4.18.
    pub spectrum_trail: Vec<f32>,
}

impl TinyBoothApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let config = Config::load();
        cc.egui_ctx.set_visuals(if config.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        cc.egui_ctx.set_zoom_factor(config.zoom);

        // Background update check.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::git_update::check_latest_release());
        });

        // Audio-thread error channel. Sender clones go to every cpal
        // err_fn closure; the UI thread drains the receiver each frame.
        let (audio_err_tx, audio_err_rx) = mpsc::channel::<String>();

        // Enumerate input devices once at startup; user can refresh later.
        // v0.4.27 — restore the persisted device pick from Config if its
        // name still matches a currently-enumerated device. Otherwise
        // fall through to the platform default (= first in the list,
        // since list_input_devices puts the default at index 0).
        let devices = audio::list_input_devices();
        let selected_device = config
            .input_device
            .as_deref()
            .filter(|saved| devices.iter().any(|d| d.name == *saved))
            .map(|s| s.to_string())
            .or_else(|| devices.first().map(|d| d.name.clone()));

        // Default scratch project in %APPDATA%\TinyBooth Sound Studio\sessions\unnamed.
        let default_root = Config::dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("sessions")
            .join(format!(
                "session-{}",
                chrono::Utc::now().format("%Y%m%d-%H%M%S")
            ));

        // Try to auto-restore the last project. Fall back to a fresh
        // scratch session if the path is missing, the file's gone, or
        // the manifest fails to parse — and clear the stale breadcrumb.
        let mut config = config; // shadow to allow mut for recovery
        let mut startup_status: Option<String> = None;
        let project = match config.last_project_path.clone() {
            Some(p) if p.is_file() => match Project::load(&p) {
                Ok(proj) => {
                    startup_status = Some(format!("Restored: {}", proj.name));
                    proj
                }
                Err(e) => {
                    config.last_project_path = None;
                    config.save_or_log();
                    startup_status = Some(format!("Could not restore last project: {e}"));
                    Project::new("Untitled session", default_root.clone())
                }
            },
            Some(_) => {
                // Path was recorded but file's gone — clear it.
                config.last_project_path = None;
                config.save_or_log();
                Project::new("Untitled session", default_root.clone())
            }
            None => Project::new("Untitled session", default_root.clone()),
        };

        // Load recording-tone profiles, seed defaults on first run, and
        // pick the last-used one (Guitar if nothing is saved).
        let profiles = dsp::load_or_seed();
        let active_profile_idx = profiles
            .iter()
            .position(|p| p.name == config.active_profile)
            .unwrap_or(0);

        Self {
            config,
            project,
            project_dirty: false,
            backing: ProjectBacking::Folder,
            devices,
            selected_device,
            selected_mode: SourceMode::Mixdown,
            viz: VizState::new(),
            session: None,
            pending_take: None,
            pending_track_name: String::new(),
            profiles,
            active_profile_idx,
            show_admin: false,
            admin_edit_idx: None,
            admin_status: None,
            export_format: ExportFormat::Wav,
            export_bitrate: 192,
            export_busy: false,
            export_msg: None,
            ffmpeg_available: export::ffmpeg_available(),
            tab: Tab::Record,
            status: startup_status,
            show_manual: false,
            manual_slug: crate::manual::DEFAULT_SLUG.to_string(),
            md_cache: egui_commonmark::CommonMarkCache::default(),
            player: None,
            player_error: None,
            player_attempt_failed_for: None,
            player_pending: None,
            editing_correction_for: None,
            show_trim: false,
            trim_state: crate::ui::trim::TrimState::default(),
            show_visualizer: false,
            visualizer: crate::ui::visualizer::VisualizerState::default(),
            import_dialog: None,
            import_conflict: None,
            pending_migration: None,
            pending_generator_modal: None,
            crossfade_state: CrossfadeUiState::default(),
            recorder: crate::automation::Recorder::default(),
            mix_console_fraction: 0.42,
            recordings_page: 0,
            recordings_peaks_cache: std::collections::HashMap::new(),
            recordings_selection: std::collections::HashMap::new(),
            mix_autoplay_pending: false,
            mix_autoplay_solo_idx: None,
            update_state: UpdateState::Checking,
            update_error: None,
            update_rx: Some(rx),
            last_update_check_at: Some(std::time::Instant::now()),
            last_tab_seen: None,
            audio_err_tx,
            audio_err_rx,
            telemetry: crate::telemetry::TelemetryService::spawn(),
            telemetry_settings: crate::telemetry::TelemetrySettings::load(),
            show_telemetry_settings: false,
            show_audio_devices: false,
            initial_telemetry_pending: true,
            show_health: false,
            spectrum_trail: Vec::new(),
        }
    }

    pub fn active_profile(&self) -> &Profile {
        &self.profiles[self.active_profile_idx.min(self.profiles.len() - 1)]
    }

    /// True when the live project is backed by a `.tib` SQLite file
    /// rather than the legacy folder format. Used by the load/save/
    /// player/trim/import/export call sites that take different code
    /// paths under each backing. TBSS-FR-0007 phase 2c.
    pub fn is_tib(&self) -> bool {
        matches!(self.backing, ProjectBacking::Tib { .. })
    }

    /// `current_rev_id` map for every playable track in the active
    /// `.tib` project, or `None` for folder-backed projects. The Mix-tab
    /// player snapshot uses this to build `TibRev` audio sources keyed
    /// by track id; the owner-thread then opens its own read-only
    /// `TibDb` connection per track (WAL allows concurrent readers).
    /// TBSS-FR-0007 phase 2c.
    pub fn tib_rev_id_map(&self) -> Option<std::collections::HashMap<String, i64>> {
        match &self.backing {
            ProjectBacking::Folder => None,
            ProjectBacking::Tib { db } => match crate::tib_project::current_rev_id_map(db) {
                Ok(m) => Some(m),
                Err(_) => Some(std::collections::HashMap::new()),
            },
        }
    }

    pub fn set_active_profile(&mut self, idx: usize) {
        if idx >= self.profiles.len() {
            return;
        }
        self.active_profile_idx = idx;
        self.config.active_profile = self.profiles[idx].name.clone();
        self.config.save_or_log();
    }

    pub fn save_profiles(&mut self) {
        match dsp::save_profiles(&self.profiles) {
            Ok(()) => self.admin_status = Some("Profiles saved.".into()),
            Err(e) => self.admin_status = Some(format!("Save failed: {e}")),
        }
    }

    pub fn reset_profiles_to_defaults(&mut self) {
        self.profiles = dsp::builtin_profiles();
        // Keep the active selection pointing at a valid index.
        self.active_profile_idx = self
            .profiles
            .iter()
            .position(|p| p.name == self.config.active_profile)
            .unwrap_or(0);
        self.save_profiles();
    }

    pub fn start_new_take(&mut self) -> anyhow::Result<()> {
        let Some(dev) = self.selected_device.clone() else {
            anyhow::bail!("select an input device first");
        };
        // Routing rule (v0.4.20): TinyDAW projects capture into their
        // own filespace; everything else (Suno imports, untitled
        // scratch sessions, the Recordings filespace itself) routes
        // to the canonical recordings project at
        // %APPDATA%\TinyBooth Sound Studio\recordings\. The
        // segregation rule for stem-mixing projects (don't
        // contaminate a Suno project with the user's takes) still
        // holds — TinyDAW projects opt INTO receiving takes, Suno
        // projects opt OUT by default.
        let rec = if self.project.captures_own_recordings()
            && !matches!(self.project.kind, crate::project::ProjectKind::Recordings)
        {
            // TinyDAW path — re-load the active project from disk so
            // we get a fresh snapshot for the take-slot allocation.
            let manifest = self.project.manifest_path();
            Project::load(&manifest).context("re-loading TinyDAW project for take capture")?
        } else {
            Project::open_or_create_recordings().context("opening recordings project")?
        };
        let (id, abs) = rec.new_track_slot();
        let name = if self.pending_track_name.trim().is_empty() {
            id.clone()
        } else {
            self.pending_track_name.trim().to_string()
        };
        let target_root = rec.root.clone();
        std::fs::create_dir_all(target_root.join(crate::project::TRACKS_DIR))?;
        let profile = self.active_profile().clone();
        let mode = self.selected_mode;
        // Force the recording rate to match the recordings project's
        // existing rate (the rate of its first track ever captured).
        // The player has no resampler yet (TBSS-FR-0002 §6); a take
        // at a mismatched rate would break the Mix tab on the
        // recordings project. cpal refuses up-front rather than
        // landing a broken take on disk.
        let required_sample_rate = rec.tracks.first().map(|t| t.sample_rate);
        let session = audio::start_recording(
            &dev,
            mode,
            &abs,
            self.viz.clone(),
            profile.clone(),
            self.audio_err_tx.clone(),
            required_sample_rate,
        )?;
        let sample_rate = session.sample_rate;
        let file_rel = abs
            .strip_prefix(&target_root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| format!("tracks/{id}.wav"));
        self.session = Some(session);
        self.pending_take = Some(PendingTake {
            target_root,
            track_id: id,
            file_rel,
            name,
            mode,
            profile,
            sample_rate,
        });
        self.pending_track_name.clear();
        Ok(())
    }

    pub fn stop_take(&mut self) {
        let Some(sess) = self.session.take() else {
            return;
        };
        let dur = sess.duration_secs();
        drop(sess);
        let Some(pt) = self.pending_take.take() else {
            self.status = Some(
                "internal error: recording stopped but no pending-take metadata was set.".into(),
            );
            return;
        };
        // Re-load the target-of-record project from disk, append the
        // take, save. Loading fresh (rather than carrying a Project
        // struct across the recording's lifetime) keeps disk as the
        // single source of truth. v0.4.20: this can be either the
        // canonical recordings filespace (default) or the active
        // TinyDAW project — `pt.target_root` was set at start_take
        // to whichever was the routing target at that moment.
        let mut rec = if self.project.root == pt.target_root
            && self.project.captures_own_recordings()
            && !matches!(self.project.kind, crate::project::ProjectKind::Recordings)
        {
            // TinyDAW path — re-load the active project's manifest.
            let manifest = pt.target_root.join(crate::project::MANIFEST_NAME);
            match Project::load(&manifest) {
                Ok(p) => p,
                Err(e) => {
                    self.status = Some(format!(
                        "could not re-load TinyDAW project to register take: {e:#}"
                    ));
                    return;
                }
            }
        } else {
            match Project::open_or_create_recordings() {
                Ok(p) => p,
                Err(e) => {
                    self.status = Some(format!("could not open recordings project: {e:#}"));
                    return;
                }
            }
        };
        // Sanity check: the recordings root we recorded under should
        // match the current recordings root. If the user reset config
        // dirs mid-recording (extremely unlikely) we'd otherwise lose
        // the track entry — guard with a clear status.
        if rec.root != pt.target_root {
            self.status = Some(format!(
                "recordings root changed during recording ({} → {}); take saved on disk \
                 but not registered.",
                pt.target_root.display(),
                rec.root.display()
            ));
            return;
        }
        let new_track = Track::recorded(
            pt.track_id,
            pt.name,
            pt.file_rel,
            pt.sample_rate,
            pt.mode,
            dur,
            pt.profile,
        );
        rec.tracks.push(new_track.clone());
        match rec.save() {
            Ok(()) => {
                // Dispatch telemetry analysis for the new take. The
                // resolved profile is `Auto`'s default for Recorded
                // sources → `UniversalOnly`. (Users can switch the
                // profile to Guitar from the Mix-tab lane to re-run
                // pitch analysis on a take.) The worker patches the
                // result onto the recordings manifest when it lands;
                // we go directly through the recordings project root
                // so the result applies even if the user doesn't
                // currently have the recordings project active.
                let abs = rec.track_abs_path(&new_track);
                let profile = new_track.telemetry_profile.resolve(&new_track.source);
                self.telemetry.dispatch(crate::telemetry::TelemetryRequest {
                    project_root: rec.root.clone(),
                    track_id: new_track.id.clone(),
                    abs_path: abs,
                    profile,
                    settings: self.telemetry_settings.clone(),
                    temp_source: false,
                });
                // If the take landed in the currently-active project
                // (Open Recordings OR an active TinyDAW project), keep
                // `app.project` in sync so the new take appears in
                // Project / Mix views without a manual reopen. Drop
                // the player so it rebuilds with the new track count
                // on the next Mix-tab visit.
                if self.project.root == rec.root {
                    self.project.tracks.push(new_track);
                    self.project_dirty = false; // disk already up to date
                    self.player = None;
                }
                let was_tinydaw = matches!(rec.kind, crate::project::ProjectKind::TinyDAW);
                self.status = Some(if was_tinydaw {
                    format!(
                        "Saved take into TinyDAW project '{}' ({} track{}).",
                        rec.name,
                        rec.tracks.len(),
                        if rec.tracks.len() == 1 { "" } else { "s" }
                    )
                } else {
                    "Saved take to Recordings — File → Open Recordings to review / mix.".into()
                });
            }
            Err(e) => {
                self.status = Some(format!("recordings save error: {e:#}"));
            }
        }
    }

    /// Run the v0.4.2 cleanse protocol on the currently-active project.
    /// If it's a Suno-shaped project (has `suno_mixdown_path`) and
    /// contains pre-v0.4.0-bug `Recorded` orphans in its tracks list,
    /// migrate them out into the recordings filespace, save both
    /// manifests, and drop the player so it rebuilds without the
    /// offending tracks.
    ///
    /// Idempotent and cheap — early-returns when the active project
    /// isn't Suno-shaped or has no orphans. Safe to call from the
    /// Mix-tab `show()` on every visit.
    pub fn cleanse_active_project(&mut self) {
        // .tib projects have no Recorded orphans to migrate: every track
        // is a `tracks` row with its audio in `revisions`, addressed by
        // id rather than by sibling-file path. The cleanse protocol's
        // whole job — moving stray WAVs out of a Suno folder into the
        // recordings filespace — doesn't apply. Step 5 grows the
        // .tib ↔ recordings-folder bridge (the recordings filespace
        // stays folder-format through 2c MVP); until then, skip.
        if self.is_tib() {
            return;
        }
        match crate::cleanup::cleanse_recordings_in_suno_project(&mut self.project) {
            Ok(report) if report.is_empty() => {
                // No-op; don't clutter status.
            }
            Ok(report) => {
                // Persist the active project (the orphans were removed
                // from its tracks list); the recordings manifest was
                // saved inside the cleanse.
                if let Err(e) = self.project.save() {
                    self.status = Some(format!(
                        "cleanse migrated tracks but project save failed: {e:#}"
                    ));
                } else {
                    self.status = Some(report.summary());
                }
                // Drop the player so it rebuilds with the new track
                // count on the next Mix-tab render this same frame.
                self.player = None;
                self.player_error = None;
            }
            Err(e) => {
                self.status = Some(format!("cleanse failed: {e:#}"));
            }
        }
    }

    /// Open the persistent recordings project as the active project.
    /// Same shape as `open_project_path` but skips the recents-list
    /// bookkeeping (recordings aren't a "project the user is working
    /// on" in the recents sense — they're scratch).
    pub fn open_recordings_project(&mut self) {
        match Project::open_or_create_recordings() {
            Ok(proj) => {
                self.project = proj;
                self.project_dirty = false;
                self.player = None;
                self.status = Some("Opened Recordings.".into());
                self.dispatch_telemetry_for_active_project();
            }
            Err(e) => {
                self.status = Some(format!("could not open Recordings: {e:#}"));
            }
        }
    }

    /// Send a recording to the main mixer for playback in one click:
    /// swap `project` to the recordings project, switch to the Mix tab,
    /// solo the selected take, and queue auto-play for the next Mix
    /// render. The Mix-tab show() consumes the auto-play flags after
    /// the player rebuilds itself for the new project.
    ///
    /// Called from the Record-tab recordings list ▶ buttons. `idx` is
    /// the index in the recordings project's `tracks` list (loaded
    /// fresh by the caller — we re-load here to guard against stale
    /// indices if the file changed between frames).
    pub fn play_recording_in_mixer(&mut self, idx: usize) {
        let rec = match Project::open_or_create_recordings() {
            Ok(p) => p,
            Err(e) => {
                self.status = Some(format!("could not open Recordings: {e:#}"));
                return;
            }
        };
        if idx >= rec.tracks.len() {
            self.status = Some("recording entry no longer exists.".into());
            return;
        }
        self.project = rec;
        self.project_dirty = false;
        self.player = None;
        self.tab = Tab::Mix;
        self.mix_autoplay_pending = true;
        self.mix_autoplay_solo_idx = Some(idx);
    }

    /// Delete a recording by index in the recordings project's
    /// `tracks` list. Removes the WAV from disk and the `Track` row
    /// from the recordings manifest. Caller should refresh its view
    /// of the recordings filespace afterward (the Record-tab list
    /// re-loads on every frame, so just calling this is enough).
    pub fn delete_recording(&mut self, idx: usize) {
        let mut rec = match Project::open_or_create_recordings() {
            Ok(p) => p,
            Err(e) => {
                self.status = Some(format!("could not open Recordings: {e:#}"));
                return;
            }
        };
        if idx >= rec.tracks.len() {
            self.status = Some("recording entry no longer exists.".into());
            return;
        }
        let removed = rec.tracks.remove(idx);
        let abs = rec.root.join(&removed.file);
        let _ = std::fs::remove_file(&abs);
        match rec.save() {
            Ok(()) => {
                self.status = Some(format!("Deleted recording '{}'.", removed.name));
                // If the user has the recordings project open as the
                // active one, drop the player so it rebuilds without
                // the deleted track on the next Mix visit.
                if Config::recordings_root()
                    .map(|root| self.project.root == root)
                    .unwrap_or(false)
                {
                    // Reflect on the active project too.
                    if let Some(pos) = self.project.tracks.iter().position(|t| t.id == removed.id) {
                        self.project.tracks.remove(pos);
                    }
                    self.player = None;
                }
            }
            Err(e) => {
                self.status = Some(format!("recordings save error: {e:#}"));
            }
        }
    }

    pub fn set_project_root(&mut self, root: PathBuf, name: String) {
        self.project = Project::new(name, root);
        self.project_dirty = true;
    }

    /// Create a new TinyDAW project at `root` and switch to it.
    /// Saves the empty manifest so the folder is a valid project
    /// immediately (the Record tab can start writing takes into it
    /// without a manual Save first). v0.4.20.
    pub fn create_tinydaw_project(&mut self, root: PathBuf, name: String) {
        let project = Project::new_tinydaw(name, root);
        match project.save() {
            Ok(()) => {
                let manifest = project.manifest_path();
                self.config.record_project(&manifest);
                self.project = project;
                self.project_dirty = false;
                self.player = None;
                self.tab = Tab::Record;
                self.status = Some(format!(
                    "TinyDAW project ready — recordings will land in {}",
                    self.project.root.display()
                ));
            }
            Err(e) => {
                self.status = Some(format!("could not create TinyDAW project: {e:#}"));
            }
        }
    }

    /// Persist the active project per its backing — folder writes the
    /// JSON manifest + sibling WAVs stay put; `.tib` writes the meta +
    /// stem/track rows in one transaction (never the audio BLOBs). Does
    /// not touch `status` / `project_dirty`; callers that want user
    /// feedback wrap it (see [`Self::save_project`]). TBSS-FR-0007.
    pub fn persist_project(&mut self) -> anyhow::Result<()> {
        match &mut self.backing {
            ProjectBacking::Folder => self.project.save(),
            ProjectBacking::Tib { db } => {
                let project = &self.project;
                db.transaction(|conn| crate::tib_project::save_metadata(project, conn))
            }
        }
    }

    pub fn save_project(&mut self) {
        let recorded_path = match &self.backing {
            ProjectBacking::Folder => self.project.manifest_path(),
            ProjectBacking::Tib { .. } => self.project.root.clone(),
        };
        match self.persist_project() {
            Ok(()) => {
                self.config.record_project(&recorded_path);
                self.status = Some(format!("saved {}", recorded_path.display()));
                self.project_dirty = false;
            }
            Err(e) => self.status = Some(format!("save error: {e:#}")),
        }
    }

    /// Pack the active (folder) project into a single `.tib` SQLite file
    /// (TBSS-FR-0007). Additive + non-destructive — the folder project is
    /// untouched; this just writes a self-contained sibling artifact.
    /// First user-facing step toward the `.tib` format; migration is then
    /// proven on real projects before the live load/save flip.
    pub fn export_project_as_tib(&mut self) {
        if self.project.tracks.is_empty() {
            self.status = Some("nothing to export — open or import a project first".to_string());
            return;
        }
        let default_name = format!("{}.tib", self.project.name.replace(['/', '\\', ':'], "-"));
        let Some(path) = rfd::FileDialog::new()
            .add_filter("TinyBooth project", &["tib"])
            .set_file_name(&default_name)
            .save_file()
        else {
            return;
        };
        match crate::tib_project::migrate_folder_to_tib(&self.project, &path) {
            Ok(()) => {
                self.status = Some(format!(
                    "exported {} ({} stems) → {}",
                    self.project.name,
                    self.project.tracks.len(),
                    path.display()
                ));
            }
            Err(e) => self.status = Some(format!("export failed: {e:#}")),
        }
    }

    /// Open a folder of Suno stems and turn it into a fresh `.tinybooth`
    /// project. The new project is saved as a sibling of the source folder
    /// and immediately becomes the active project.
    pub fn import_suno_folder(&mut self) {
        let Some(src) = rfd::FileDialog::new()
            .set_title("Pick a folder of Suno stems")
            .pick_folder()
        else {
            return;
        };
        let name = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Suno session".into());
        let parent = src.parent().unwrap_or_else(|| Path::new("."));
        let project_root = parent.join(format!("{name} (TinyBooth)"));
        let probe = crate::suno_import::probe_folder(&src, &project_root);
        if probe.is_duplicate() {
            self.import_conflict = Some(PendingImport {
                kind: ImportKind::Folder,
                source: src,
                project_root,
                project_name: name,
                probe,
            });
            return;
        }
        let outcome = crate::suno_import::import_folder(&src, &project_root, &name);
        self.apply_import_outcome(outcome);
    }

    /// Same as [`import_suno_folder`] but for a "Download All" zip archive.
    pub fn import_suno_zip(&mut self) {
        let Some(src) = rfd::FileDialog::new()
            .set_title("Pick a Suno stems zip archive")
            .add_filter("Zip archive", &["zip"])
            .pick_file()
        else {
            return;
        };
        let name = src
            .file_stem()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Suno session".into());
        let parent = src.parent().unwrap_or_else(|| Path::new("."));
        let project_root = parent.join(format!("{name} (TinyBooth)"));
        let probe = crate::suno_import::probe_zip(&src, &project_root);
        if probe.is_duplicate() {
            self.import_conflict = Some(PendingImport {
                kind: ImportKind::Zip,
                source: src,
                project_root,
                project_name: name,
                probe,
            });
            return;
        }
        let outcome = crate::suno_import::import_zip(&src, &project_root, &name);
        self.apply_import_outcome(outcome);
    }

    /// Apply a default correction chain to every track that doesn't
    /// already carry one. Cascade for the seed profile:
    ///
    ///   1. **From project** — keeps any `track.correction` already set.
    ///   2. **From project defaults** — `Project.default_correction`.
    ///   3. **From feature default** — Suno-Clean in `builtin_profiles()`.
    ///
    /// Tracks already carrying a chain are left untouched.
    pub fn enable_all_corrections(&mut self) {
        let (seed, source_label) = if let Some(p) = self.project.default_correction.clone() {
            (Some(p), "project default")
        } else if let Some(p) = self
            .profiles
            .iter()
            .find(|p| p.name == "Suno-Clean")
            .cloned()
        {
            (Some(p), "Suno-Clean (feature default)")
        } else {
            (self.profiles.first().cloned(), "first available preset")
        };
        let Some(seed) = seed else {
            self.status = Some("No profiles available to seed corrections.".into());
            return;
        };

        let mut changed = 0;
        let mut already = 0;
        for (i, track) in self.project.tracks.iter_mut().enumerate() {
            if track.correction.is_some() {
                already += 1;
                continue;
            }
            track.correction = Some(seed.clone());
            changed += 1;
            if let Some(player) = self.player.as_ref() {
                if let Some(t) = player.state.tracks.get(i) {
                    t.set_correction(Some(seed.clone()));
                }
            }
        }
        if changed > 0 {
            self.project_dirty = true;
            self.status = Some(if already > 0 {
                format!("Enabled corrections from {source_label} on {changed} track(s) — {already} already had chains.")
            } else {
                format!("Enabled corrections from {source_label} on all {changed} track(s).")
            });
        } else {
            self.status = Some(format!("All {already} track(s) already have corrections."));
        }
    }

    /// Ephemeral A/B — flips the player's `global_bypass` atomic
    /// without touching `Project.corrections_disabled`. Lost on reload
    /// (the persisted flag wins on the next `Player::new`). Picks up
    /// mid-playback — the audio callback reads `global_bypass` once
    /// per buffer and ORs it with each track's per-track bypass.
    pub fn toggle_global_bypass(&mut self) -> bool {
        let Some(player) = self.player.as_ref() else {
            return false;
        };
        let cur = player
            .state
            .global_bypass
            .load(std::sync::atomic::Ordering::Relaxed);
        let new_state = !cur;
        player
            .state
            .global_bypass
            .store(new_state, std::sync::atomic::Ordering::Relaxed);
        self.status = Some(if new_state {
            "Global bypass ON — playback is now the raw source on every track.".into()
        } else {
            "Global bypass OFF — corrections live again.".into()
        });
        new_state
    }

    /// Flip the **persisted** project-level disable flag. Saves to the
    /// manifest on next File → Save; the player picks up the change
    /// instantly via `PlayerState.global_bypass`. Survives project
    /// reload — the manifest carries the flag and `Player::new`
    /// initialises the atomic from it.
    pub fn toggle_corrections_disabled(&mut self) {
        self.project.corrections_disabled = !self.project.corrections_disabled;
        if let Some(player) = self.player.as_ref() {
            player.state.global_bypass.store(
                self.project.corrections_disabled,
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        self.project_dirty = true;
        self.status = Some(if self.project.corrections_disabled {
            "Project-wide corrections DISABLED (persisted). Save to keep this on next reload."
                .into()
        } else {
            "Project-wide corrections ENABLED (persisted).".into()
        });
    }

    /// Strip every track's correction chain. **Destructive** — chain
    /// configs are gone after this; re-enabling re-seeds from the
    /// cascade. Used when the user wants a clean slate.
    /// (Renamed from `disable_all_corrections` in v0.3.4 — the previous
    /// name now belongs to `toggle_corrections_disabled`.)
    pub fn reset_all_corrections(&mut self) {
        let mut changed = 0;
        for (i, track) in self.project.tracks.iter_mut().enumerate() {
            if track.correction.is_none() {
                continue;
            }
            track.correction = None;
            changed += 1;
            if let Some(player) = self.player.as_ref() {
                if let Some(t) = player.state.tracks.get(i) {
                    t.set_correction(None);
                }
            }
        }
        if changed > 0 {
            self.project_dirty = true;
            self.status = Some(format!(
                "Reset (cleared) corrections on {changed} track(s)."
            ));
        } else {
            self.status = Some("No tracks had corrections to reset.".into());
        }
    }

    /// Resolve a pending import (called by the conflict modal).
    /// `replace = true` wipes the existing project and re-imports.
    pub fn resolve_import_conflict(&mut self, replace: bool) {
        let Some(pending) = self.import_conflict.take() else {
            return;
        };
        if !replace {
            return;
        } // Cancel — do nothing
        if let Err(e) = crate::suno_import::wipe_project_root(&pending.project_root) {
            self.status = Some(format!("Could not wipe existing project: {e}"));
            return;
        }
        let outcome = match pending.kind {
            ImportKind::Folder => crate::suno_import::import_folder(
                &pending.source,
                &pending.project_root,
                &pending.project_name,
            ),
            ImportKind::Zip => crate::suno_import::import_zip(
                &pending.source,
                &pending.project_root,
                &pending.project_name,
            ),
        };
        self.apply_import_outcome(outcome);
    }

    /// Common post-import handling. Updates state on success and always
    /// pops the modal regardless of outcome — silence-on-failure is what
    /// made this whole flow feel broken.
    fn apply_import_outcome(&mut self, outcome: crate::suno_import::ImportOutcome) {
        if outcome.success {
            if let Some(proj) = outcome.project.clone() {
                // Phase 2c: imports land in the live `.tib` format. The
                // import has already written a folder project (raw Suno
                // WAVs + manifest); migrate that to a sibling `.tib` and
                // open it. The folder staging stays on disk as a backup.
                // Telemetry is dispatched once, by the open path, over
                // the .tib BLOBs — we deliberately don't activate the
                // folder project, so there's no double analysis.
                let tib_path = proj.root.with_extension("tib");
                match crate::tib_project::migrate_folder_to_tib(&proj, &tib_path) {
                    Ok(()) => {
                        self.open_project_path(&tib_path);
                        self.tab = Tab::Project;
                        self.status = Some(format!(
                            "Imported → {} ({} tracks)",
                            tib_path.display(),
                            self.project.tracks.len()
                        ));
                    }
                    Err(e) => {
                        // Migration failed — fall back to the folder
                        // project so the import isn't lost.
                        self.config.record_project(&proj.manifest_path());
                        self.project = proj;
                        self.backing = ProjectBacking::Folder;
                        self.project_dirty = false;
                        self.player = None;
                        self.tab = Tab::Project;
                        self.dispatch_telemetry_for_active_project();
                        self.status =
                            Some(format!("Imported as folder — .tib migration failed: {e:#}"));
                    }
                }
            }
        } else {
            self.status = Some("Suno import did not produce any tracks — see dialog".into());
        }
        self.import_dialog = Some(outcome);
    }

    /// Walk `self.project.tracks` and dispatch a telemetry analysis
    /// request for every track whose `telemetry` is `None` or whose
    /// `analyzer_version` is older than the current one. The profile
    /// (drum / guitar / bass / etc.) is resolved per track via
    /// `TelemetryProfile::resolve`. Cheap on the all-fresh path —
    /// just iterates and pushes to a channel.
    pub fn dispatch_telemetry_for_active_project(&mut self) {
        if self.is_tib() {
            self.dispatch_telemetry_tib();
        } else {
            self.dispatch_telemetry_folder();
        }
    }

    /// Folder-backed dispatch: every track's WAV is read straight off
    /// disk by `abs_path`.
    fn dispatch_telemetry_folder(&mut self) {
        let root = self.project.root.clone();
        for t in &self.project.tracks {
            if telemetry_is_current(t) {
                continue;
            }
            let abs = self.project.track_abs_path(t);
            if !abs.is_file() {
                continue;
            }
            let profile = t.telemetry_profile.resolve(&t.source);
            self.telemetry.dispatch(crate::telemetry::TelemetryRequest {
                project_root: root.clone(),
                track_id: t.id.clone(),
                abs_path: abs,
                profile,
                settings: self.telemetry_settings.clone(),
                temp_source: false,
            });
        }
    }

    /// `.tib`-backed dispatch (TBSS-FR-0007 phase 2c): the analyzer reads
    /// WAVs by path, so extract each stale track's current-revision BLOB
    /// to a throwaway temp WAV and analyze that. The worker deletes the
    /// temp after reading; the drain matches the result back by track id
    /// (the temp path is ephemeral). Heavy projects extract one stem at a
    /// time — bounded by the analyzer's own one-at-a-time worker.
    fn dispatch_telemetry_tib(&mut self) {
        // Collect (track_id, profile) first to release the project borrow
        // before touching `self.backing` / `self.telemetry`.
        let jobs: Vec<(String, crate::telemetry::ResolvedProfile)> = self
            .project
            .tracks
            .iter()
            .filter(|t| !telemetry_is_current(t))
            .map(|t| (t.id.clone(), t.telemetry_profile.resolve(&t.source)))
            .collect();
        if jobs.is_empty() {
            return;
        }
        let root = self.project.root.clone();
        let temp_dir = std::env::temp_dir().join("tbss-telemetry");
        if std::fs::create_dir_all(&temp_dir).is_err() {
            return;
        }
        for (track_id, profile) in jobs {
            let bytes = match &self.backing {
                ProjectBacking::Tib { db } => match db.read_current_audio(&track_id) {
                    Ok(b) => b,
                    Err(_) => continue, // no current revision → nothing to analyze
                },
                ProjectBacking::Folder => return,
            };
            let safe_id: String = track_id
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect();
            let temp_path = temp_dir.join(format!(
                "{}-{}-{}.wav",
                safe_id,
                std::process::id(),
                root_tag(&root)
            ));
            if std::fs::write(&temp_path, &bytes).is_err() {
                continue;
            }
            self.telemetry.dispatch(crate::telemetry::TelemetryRequest {
                project_root: root.clone(),
                track_id,
                abs_path: temp_path,
                profile,
                settings: self.telemetry_settings.clone(),
                temp_source: true,
            });
        }
    }

    /// Force-re-analyze a single track (typically because its
    /// `telemetry_profile` just changed). Sets `telemetry = None`
    /// then dispatches with the new resolved profile. Saves so the
    /// `None` is persisted — otherwise the user could close the
    /// app between profile-change and analysis-completion and the
    /// stale telemetry would still be on disk.
    /// Hot-load a fresh WAV into an existing track, keeping every
    /// other field of the manifest (gain, correction, automation,
    /// telemetry_profile, polarity_inverted, name, role). The on-disk
    /// audio is overwritten via an atomic copy, TBSS metadata is
    /// injected into the new file, telemetry is invalidated and
    /// re-dispatched, and the project is auto-saved. The player
    /// drops itself so the next Mix-tab frame rebuilds with the new
    /// audio in its in-memory cache.
    ///
    /// Refuses the swap when the new WAV's sample rate doesn't match
    /// the project's existing rate (other tracks share a rate; we
    /// have no resampler) — caller gets a clear status message and
    /// nothing on disk changes. v0.4.20.
    pub fn hot_load_swap(&mut self, idx: usize, source: &Path) -> anyhow::Result<()> {
        // Generator tracks are baked output, not swappable — re-bake
        // with new parameters instead. TBSS-FR-0009.
        if self
            .project
            .tracks
            .get(idx)
            .map(|t| t.is_locked())
            .unwrap_or(false)
        {
            anyhow::bail!(
                "generator tracks are baked from parameters, not hot-swapped — \
                 change the generator settings and re-bake instead"
            );
        }
        // .tib projects: a swap is a new destructive revision + a
        // current_rev_id repoint, not a WAV overwrite — so the pre-swap
        // take stays recoverable in history. (TBSS-FR-0007 phase 2c.)
        if self.is_tib() {
            return self.hot_load_swap_tib(idx, source);
        }
        let track = self
            .project
            .tracks
            .get(idx)
            .ok_or_else(|| anyhow::anyhow!("no track at index {idx}"))?;

        // Probe the source WAV header — sample rate, channels, duration.
        let reader = hound::WavReader::open(source)
            .with_context(|| format!("opening source {}", source.display()))?;
        let spec = reader.spec();
        let frames = reader.duration() as u64;
        let new_sr = spec.sample_rate;
        let new_channels = spec.channels;
        let new_duration_secs = frames as f32 / new_sr.max(1) as f32;
        drop(reader);

        // Sample-rate enforcement: every track in a project must share
        // a rate (no resampler yet — TBSS-FR-0002 §6). Check against
        // the OTHER tracks (skip ours since we're replacing it).
        let project_rate = self
            .project
            .tracks
            .iter()
            .enumerate()
            .find(|(i, _)| *i != idx)
            .map(|(_, t)| t.sample_rate);
        if let Some(rate) = project_rate {
            if rate != new_sr {
                anyhow::bail!(
                    "sample-rate mismatch: project is at {} Hz, new file is {} Hz. \
                     Re-export the file at {} Hz and try again.",
                    rate,
                    new_sr,
                    rate
                );
            }
        }

        // Compute destination — keep the existing relative path so
        // the manifest doesn't change, just the bytes.
        let dest_abs = self.project.root.join(&track.file);
        if let Some(parent) = dest_abs.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        // Atomic-style copy: write source bytes to a `.swap-tmp`
        // sibling, then rename over the live file. Same shape as
        // `Project::save` and `trim::trim_project` so a process
        // crash mid-swap can never leave us with a half-written WAV.
        let tmp = dest_abs.with_extension("wav.swap-tmp");
        std::fs::copy(source, &tmp)
            .with_context(|| format!("copying {} → {}", source.display(), tmp.display()))?;

        // Inject TBSS metadata into the temp copy BEFORE renaming —
        // so even if rename fails, the live file is untouched.
        let meta = crate::wav_meta::TbssWavMeta::from_track(&self.project, track);
        crate::wav_meta::inject_tbss_meta(&tmp, &meta)
            .with_context(|| format!("injecting TBSS metadata into {}", tmp.display()))?;

        std::fs::rename(&tmp, &dest_abs)
            .with_context(|| format!("renaming {} → {}", tmp.display(), dest_abs.display()))?;

        // Patch the manifest fields that depend on the audio: new
        // duration, sample rate (we already verified match — store
        // it anyway in case this is the first track in an empty
        // project), stereo flag. Telemetry zeroed; the worker
        // re-runs below.
        let track_mut = &mut self.project.tracks[idx];
        track_mut.duration_secs = new_duration_secs;
        track_mut.sample_rate = new_sr;
        track_mut.stereo = new_channels >= 2;
        track_mut.telemetry = None;

        // Drop the player — next Mix render rebuilds with the new WAV.
        self.player = None;
        self.player_error = None;
        self.player_attempt_failed_for = None;

        // Autosave (v0.4.20 — the whole point of "auto save on swap").
        self.project.save().context("saving project after swap")?;

        // Re-dispatch telemetry for the new audio.
        self.invalidate_telemetry_for_track(idx);

        // Recompute song key — old key estimate was derived from the
        // previous audio's events.
        self.project.song_key_estimate = crate::telemetry::estimate_song_key(&self.project.tracks);

        Ok(())
    }

    /// `.tib` counterpart of [`Self::hot_load_swap`]. Reads the source
    /// WAV bytes and commits them as a new `destructive` revision on the
    /// track (repointing `current_rev_id`, FIFO-5 pruning) — the pre-swap
    /// audio remains a recoverable revision. Metadata is persisted via the
    /// normal `.tib` save; telemetry re-analysis is skipped until the
    /// analyzer learns to read BLOBs (step beyond 2c MVP).
    fn hot_load_swap_tib(&mut self, idx: usize, source: &Path) -> anyhow::Result<()> {
        let track_id = self
            .project
            .tracks
            .get(idx)
            .ok_or_else(|| anyhow::anyhow!("no track at index {idx}"))?
            .id
            .clone();

        // Probe the source WAV header.
        let reader = hound::WavReader::open(source)
            .with_context(|| format!("opening source {}", source.display()))?;
        let spec = reader.spec();
        let frames = reader.duration() as u64;
        let new_sr = spec.sample_rate;
        let new_channels = spec.channels;
        let new_duration_secs = frames as f32 / new_sr.max(1) as f32;
        drop(reader);

        // Same single-rate enforcement as the folder path: check against
        // the other tracks (skip ours — we're replacing it).
        let project_rate = self
            .project
            .tracks
            .iter()
            .enumerate()
            .find(|(i, _)| *i != idx)
            .map(|(_, t)| t.sample_rate);
        if let Some(rate) = project_rate {
            if rate != new_sr {
                anyhow::bail!(
                    "sample-rate mismatch: project is at {} Hz, new file is {} Hz. \
                     Re-export the file at {} Hz and try again.",
                    rate,
                    new_sr,
                    rate
                );
            }
        }

        // Read the whole source WAV and commit it as a destructive
        // revision. We store the file bytes as-is (no TBSS-meta injection
        // — the .tib carries that state in columns, not in WAV chunks).
        let bytes = std::fs::read(source)
            .with_context(|| format!("reading source {}", source.display()))?;
        let stereo = new_channels >= 2;
        if let ProjectBacking::Tib { db } = &mut self.backing {
            db.commit_destructive_revision(
                &track_id,
                "hot-swap",
                new_sr,
                stereo,
                new_duration_secs,
                &bytes,
                5,
            )
            .context("committing hot-swap revision")?;
            db.incremental_vacuum().ok();
        }

        let track_mut = &mut self.project.tracks[idx];
        track_mut.duration_secs = new_duration_secs;
        track_mut.sample_rate = new_sr;
        track_mut.stereo = stereo;
        track_mut.telemetry = None;

        self.player = None;
        self.player_error = None;
        self.player_attempt_failed_for = None;

        self.save_project();
        // Re-analyze the swapped track from its new BLOB (telemetry was
        // cleared above; dispatch only picks up the now-stale track).
        self.dispatch_telemetry_tib();
        Ok(())
    }

    pub fn invalidate_telemetry_for_track(&mut self, idx: usize) {
        {
            let Some(track) = self.project.tracks.get_mut(idx) else {
                return;
            };
            track.telemetry = None;
        }
        // .tib: persist the cleared telemetry, then re-run the BLOB-aware
        // dispatch (it only re-analyzes the now-stale track, since the
        // others are still current). TBSS-FR-0007 phase 2c.
        if self.is_tib() {
            let _ = self.persist_project();
            self.dispatch_telemetry_tib();
            return;
        }
        let Some(track) = self.project.tracks.get(idx) else {
            return;
        };
        let profile = track.telemetry_profile.resolve(&track.source);
        let track_id = track.id.clone();
        let abs = self.project.root.join(&track.file);
        let project_root = self.project.root.clone();
        let settings = self.telemetry_settings.clone();
        if abs.is_file() {
            self.telemetry.dispatch(crate::telemetry::TelemetryRequest {
                project_root,
                track_id,
                abs_path: abs,
                profile,
                settings,
                temp_source: false,
            });
        }
        let _ = self.project.save();
    }

    /// Drain every result the worker has produced since the last
    /// frame. For results matching the active project, patch the
    /// telemetry onto the matching track and persist incrementally
    /// (one save per drain — the manifest is cheap relative to
    /// blocking on every result individually). Results targeting a
    /// different project root (typically: recordings analysis while
    /// the user has a Suno project active) get written through to
    /// the manifest on disk. Stale results (file path no longer
    /// matches, track id gone) are silently dropped.
    pub fn drain_telemetry_results(&mut self) {
        let results = self.telemetry.drain();
        if results.is_empty() {
            return;
        }
        let mut applied_active = 0;
        let mut errored = 0;
        // Group off-project results by root so each foreign manifest
        // is loaded + saved once per drain rather than per result.
        let mut foreign: std::collections::HashMap<
            PathBuf,
            Vec<crate::telemetry::TelemetryResult>,
        > = std::collections::HashMap::new();
        for r in results {
            if let Err(e) = &r.outcome {
                errored += 1;
                eprintln!("telemetry: failed to analyze track {}: {e}", r.track_id);
                continue;
            }
            if r.project_root == self.project.root {
                let temp_source = r.temp_source;
                if let Ok(tel) = r.outcome {
                    if let Some(track) = self.project.tracks.iter_mut().find(|t| t.id == r.track_id)
                    {
                        // Folder: verify the WAV path hasn't changed under
                        // us (Trim writes a fresh WAV, so the path may have
                        // moved — drop stale results). .tib (temp_source):
                        // the path is an ephemeral temp extraction, so the
                        // track id IS the identity — accept it.
                        let still_valid =
                            temp_source || self.project.root.join(&track.file) == r.abs_path;
                        if still_valid {
                            track.telemetry = Some(tel);
                            applied_active += 1;
                        }
                    }
                }
            } else {
                foreign.entry(r.project_root.clone()).or_default().push(r);
            }
        }
        if applied_active > 0 {
            // Re-estimate the project-level key from the union of
            // every melodic track's pitch-class histogram (cheap —
            // a couple of hundred adds, one Pearson over 24 keys).
            self.project.song_key_estimate =
                crate::telemetry::estimate_song_key(&self.project.tracks);
            // Backing-aware: folder writes the manifest, .tib writes the
            // metadata rows in a transaction (never the audio BLOBs).
            if let Err(e) = self.persist_project() {
                self.status = Some(format!("telemetry save error: {e:#}"));
            }
        }
        // For each foreign root, load the manifest, patch matching
        // tracks, save back. Silently no-op on load failure — we
        // don't want a missing recordings manifest to spam the user.
        for (root, group) in foreign {
            let manifest = root.join(crate::project::MANIFEST_NAME);
            let mut proj = match Project::load(&manifest) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let mut changed = 0;
            for r in group {
                if let Ok(tel) = r.outcome {
                    if let Some(track) = proj.tracks.iter_mut().find(|t| t.id == r.track_id) {
                        let cur = proj.root.join(&track.file);
                        if cur == r.abs_path {
                            track.telemetry = Some(tel);
                            changed += 1;
                        }
                    }
                }
            }
            if changed > 0 {
                proj.song_key_estimate = crate::telemetry::estimate_song_key(&proj.tracks);
                let _ = proj.save();
            }
        }
        if errored > 0 {
            self.status = Some(format!(
                "telemetry: {errored} track(s) failed to analyze; see console"
            ));
        }
    }

    pub fn open_project_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("TinyBooth project", &["tib", "tinybooth"])
            .pick_file()
        {
            self.open_project_path(&p);
        }
    }

    /// Load a project from a known path — either a `.tib` SQLite file
    /// (TBSS-FR-0007) or a legacy folder-format `.tinybooth` manifest.
    /// Used by the Open dialog and by the File → Open Recent submenu.
    pub fn open_project_path(&mut self, path: &Path) {
        let is_tib = path.extension().and_then(|e| e.to_str()) == Some("tib");
        if is_tib {
            self.open_tib_path(path);
        } else {
            // Legacy folder project: nudge toward .tib via the migrate
            // modal instead of opening straight away. The user can still
            // choose "Open as folder". The .tib lands as a sibling of the
            // project root (the manifest's parent dir).
            let suggested_tib = path
                .parent()
                .map(|root| root.with_extension("tib"))
                .unwrap_or_else(|| path.with_extension("tib"));
            self.pending_migration = Some(PendingMigration {
                folder_manifest: path.to_path_buf(),
                suggested_tib,
            });
        }
    }

    /// Resolve the migrate-to-`.tib` prompt. `migrate = true` converts the
    /// folder project to its sibling `.tib` and opens it (folder kept as
    /// backup); `migrate = false` opens the folder project as-is. Either
    /// way the pending prompt is cleared.
    pub fn resolve_migration(&mut self, migrate: bool) {
        let Some(pending) = self.pending_migration.take() else {
            return;
        };
        if !migrate {
            self.open_folder_manifest_path(&pending.folder_manifest);
            return;
        }
        match Project::load(&pending.folder_manifest) {
            Ok(proj) => {
                match crate::tib_project::migrate_folder_to_tib(&proj, &pending.suggested_tib) {
                    Ok(()) => {
                        self.open_tib_path(&pending.suggested_tib);
                        self.status = Some(format!(
                            "Migrated → {} (folder kept as backup)",
                            pending.suggested_tib.display()
                        ));
                    }
                    Err(e) => {
                        // Migration failed — fall back to opening the folder.
                        self.open_folder_manifest_path(&pending.folder_manifest);
                        self.status =
                            Some(format!("Opened as folder — .tib migration failed: {e:#}"));
                    }
                }
            }
            Err(e) => {
                self.status = Some(format!("could not load project to migrate: {e:#}"));
            }
        }
    }

    fn open_folder_manifest_path(&mut self, path: &Path) {
        match Project::load(path) {
            Ok(proj) => {
                self.config.record_project(path);
                self.project = proj;
                self.backing = ProjectBacking::Folder;
                self.project_dirty = false;
                self.player = None; // force player rebuild for new project
                self.status = Some(format!("opened {}", path.display()));
                // Backfill telemetry for projects saved before TBSS-FR-0005
                // landed (or analyzer-version mismatches). Cheap on the
                // already-current path — dispatch_telemetry_for_active_project
                // skips tracks whose analyzer_version is current.
                self.dispatch_telemetry_for_active_project();
            }
            Err(e) => {
                // Stale recent — drop it so the menu cleans up over time.
                self.config.recent_projects.retain(|p| p != path);
                self.config.save_or_log();
                self.status = Some(format!("open error: {e}"));
            }
        }
    }

    fn open_tib_path(&mut self, path: &Path) {
        match crate::tib::TibDb::open(path) {
            Ok(db) => match crate::tib_project::load_project(&db, path.to_path_buf()) {
                Ok(proj) => {
                    self.config.record_project(path);
                    self.project = proj;
                    self.backing = ProjectBacking::Tib { db };
                    self.project_dirty = false;
                    self.player = None;
                    self.status = Some(format!("opened {}", path.display()));
                    // Backfill telemetry via the BLOB→temp-WAV bridge —
                    // the .tib branch of dispatch handles the extraction.
                    self.dispatch_telemetry_for_active_project();
                }
                Err(e) => {
                    self.config.recent_projects.retain(|p| p != path);
                    self.config.save_or_log();
                    self.status = Some(format!("open error: {e:#}"));
                }
            },
            Err(e) => {
                self.config.recent_projects.retain(|p| p != path);
                self.config.save_or_log();
                self.status = Some(format!("open error: {e:#}"));
            }
        }
    }

    // ── TBSS-FR-0009: Generator-track bake plumbing ────────────────

    /// Bake the generator track at `track_idx` — resolve duration from
    /// the longest other stem, render via [`crate::generator::bake`],
    /// store the WAV bytes through the project's backing (`.tib` via a
    /// new destructive revision with FIFO-5 history; folder via a
    /// `tracks/<id>.wav` write), drop a timestamped copy under
    /// `<project>/exports/generator-bakes/`, stamp the track's
    /// `last_bake_at` + `last_bake_master_signature`, persist the
    /// project, and drop the player so it rebuilds from the new audio.
    ///
    /// Returns the path of the timestamped export file. The track's
    /// dirty indicator clears as soon as the stamped signature matches
    /// the current project signature again (immediately post-bake by
    /// construction).
    pub fn bake_generator(&mut self, track_idx: usize) -> anyhow::Result<PathBuf> {
        let exported = bake_generator_impl(&mut self.project, &mut self.backing, track_idx)?;
        self.persist_project()?;
        self.player = None;
        self.player_error = None;
        self.player_attempt_failed_for = None;
        Ok(exported)
    }

    /// True when the generator track at `track_idx` needs re-baking:
    /// either it has never been baked, or the current project state's
    /// [`MasterSignature`] no longer matches the stamp from the last
    /// bake. Returns `false` for non-Generator tracks. Read on every
    /// Mix-tab visit by the dirty-indicator render in step 5.
    #[allow(dead_code)] // wired in by the per-lane indicator follow-up
    pub fn is_generator_dirty(&self, track_idx: usize) -> bool {
        let Some(track) = self.project.tracks.get(track_idx) else {
            return false;
        };
        let crate::project::TrackSource::Generator {
            last_bake_master_signature,
            ..
        } = &track.source
        else {
            return false;
        };
        match last_bake_master_signature {
            None => true, // never baked → dirty
            Some(stamped) => {
                let current = crate::project::compute_master_signature(&self.project, track_idx);
                current != *stamped
            }
        }
    }

    /// Open the Add-Generator-Track modal with default Binaural params.
    /// Step-5 UI entry point — called from the File menu item.
    pub fn open_add_generator_modal(&mut self) {
        self.pending_generator_modal = Some(PendingGeneratorParams {
            mode: crate::project::GeneratorMode::default(),
        });
    }

    /// Resolve the Add-Generator-Track modal. `commit = true` creates
    /// the track (using the modal's draft `mode`) and immediately tries
    /// to bake it. If the project has no other tracks to anchor the
    /// bake duration, the track is added in a "dirty / not yet baked"
    /// state — surfaced in the status bar — and the user can bake later
    /// once stems are imported. `commit = false` just closes the modal.
    pub fn resolve_generator_modal(&mut self, commit: bool) {
        let Some(pending) = self.pending_generator_modal.take() else {
            return;
        };
        if !commit {
            return;
        }
        match self.add_generator_track(pending.mode) {
            Ok(_idx) => {
                // status is set by add_generator_track / bake_generator.
            }
            Err(e) => {
                self.status = Some(format!("could not add generator track: {e:#}"));
            }
        }
    }

    /// Create a new Generator track in `project.tracks` with the given
    /// `mode`, then attempt to bake it. The bake fails if the project
    /// has no other tracks to anchor the duration; in that case the
    /// track is left in its un-baked state and the user can re-bake
    /// once stems exist.
    pub fn add_generator_track(
        &mut self,
        mode: crate::project::GeneratorMode,
    ) -> anyhow::Result<usize> {
        use crate::project::{Track, TrackSource};
        // Mint a unique `gen-NNN` id.
        let id = mint_generator_track_id(&self.project);
        let track = Track {
            id: id.clone(),
            name: default_generator_track_name(&mode),
            file: String::new(), // will be set on bake (folder); .tib leaves empty
            mute: false,
            gain_db: -6.0, // conservative default — generator at unity is loud
            sample_rate: 48_000,
            channel_source: None,
            duration_secs: 0.0,
            profile: None,
            stereo: true,
            source: TrackSource::Generator {
                mode,
                last_bake_at: None,
                last_bake_master_signature: None,
            },
            correction: None,
            gain_automation: None,
            polarity_inverted: false,
            telemetry: None,
            telemetry_profile: crate::telemetry::TelemetryProfile::default(),
        };
        self.project.tracks.push(track);
        let idx = self.project.tracks.len() - 1;
        self.project_dirty = true;
        // Try to bake immediately. If no other tracks exist, the bake
        // errors clearly — surface that and leave the track unbaked.
        match self.bake_generator(idx) {
            Ok(export) => {
                self.status = Some(format!(
                    "Added generator '{id}' → baked, exported {}",
                    export.display()
                ));
            }
            Err(e) => {
                self.status = Some(format!(
                    "Added generator '{id}' (not yet baked — import a stem then re-add: {e:#})"
                ));
            }
        }
        Ok(idx)
    }
}

fn mint_generator_track_id(project: &Project) -> String {
    for n in 1..1000 {
        let candidate = format!("gen-{n:03}");
        if !project.tracks.iter().any(|t| t.id == candidate) {
            return candidate;
        }
    }
    // Shouldn't happen for any real project — fall back to a UUID-like
    // tag so we never collide.
    format!("gen-{}", std::process::id())
}

fn default_generator_track_name(mode: &crate::project::GeneratorMode) -> String {
    match mode {
        crate::project::GeneratorMode::Binaural { .. } => "Binaural Focus".into(),
        crate::project::GeneratorMode::Isochronic { .. } => "Isochronic Focus".into(),
        crate::project::GeneratorMode::Layered => "Layered Focus".into(),
    }
}

/// Free-function form of the bake — does the work without depending on
/// `TinyBoothApp` state so the bake-and-store cycle is unit-testable.
/// The caller persists the project + drops the player.
#[allow(dead_code)] // gated until step-5 UI lands the bake button caller
fn bake_generator_impl(
    project: &mut Project,
    backing: &mut ProjectBacking,
    track_idx: usize,
) -> anyhow::Result<PathBuf> {
    use anyhow::{anyhow, bail};

    // ── 1. Read everything the bake needs in one immutable pass ─────
    let mode;
    let track_id;
    let sample_rate;
    let dur_secs;
    let sig;
    {
        let track = project
            .tracks
            .get(track_idx)
            .ok_or_else(|| anyhow!("no track at index {track_idx}"))?;
        mode = match &track.source {
            crate::project::TrackSource::Generator { mode, .. } => mode.clone(),
            _ => bail!("track {track_idx} is not a Generator track"),
        };
        track_id = track.id.clone();
        // Duration: longest other track. If nothing else exists, bail
        // — there's no implicit anchor and silently picking a default
        // would surprise the user.
        let longest_other = project
            .tracks
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != track_idx)
            .map(|(_, t)| t.duration_secs)
            .fold(0.0_f32, |a, b| a.max(b));
        if longest_other <= 0.0 {
            bail!(
                "cannot bake generator '{track_id}' — no other track has a length to anchor to. \
                 Add or import at least one stem first."
            );
        }
        dur_secs = longest_other;
        // Sample rate: first other track's rate. Falls back to 48 kHz
        // only when no other tracks (unreachable given the bail above).
        sample_rate = project
            .tracks
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != track_idx)
            .map(|(_, t)| t.sample_rate.max(1))
            .next()
            .unwrap_or(48_000);
        sig = crate::project::compute_master_signature(project, track_idx);
    }

    // ── 2. Bake the WAV bytes (pure DSP, no I/O) ───────────────────
    let bytes = crate::generator::bake(&mode, dur_secs, sample_rate)?;

    // ── 3. Store via the backing ───────────────────────────────────
    // Folder: write tracks/<id>.wav and record the relative path.
    // .tib: commit as a new destructive revision (free FIFO-5 history
    // of past bakes — TBSS-FR-0007 phase 2c primitive reuse).
    let file_rel: String = match backing {
        ProjectBacking::Tib { db } => {
            db.commit_destructive_revision(
                &track_id,
                "bake",
                sample_rate,
                /* stereo */ true,
                dur_secs,
                &bytes,
                /* keep */ 5,
            )?;
            db.incremental_vacuum().ok();
            // .tib tracks carry `file = ""` by convention.
            String::new()
        }
        ProjectBacking::Folder => {
            let rel = format!("{}/{}.wav", crate::project::TRACKS_DIR, track_id);
            let abs = project.root.join(&rel);
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&abs, &bytes)?;
            rel
        }
    };

    // ── 4. Write the timestamped export ────────────────────────────
    let now_utc = chrono::Utc::now();
    let exports_root = match backing {
        // .tib: project.root is the .tib file path; drop exports/
        // alongside it. Fall back to project.root itself if no parent
        // (shouldn't happen with a real path).
        ProjectBacking::Tib { .. } => project
            .root
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| project.root.clone()),
        // Folder: exports/ inside the project root.
        ProjectBacking::Folder => project.root.clone(),
    };
    let exports_dir = exports_root.join("exports").join("generator-bakes");
    std::fs::create_dir_all(&exports_dir)?;
    let safe_id: String = track_id
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let ts = now_utc.format("%Y%m%dT%H%M%SZ").to_string();
    let exported_path = exports_dir.join(format!("{safe_id}-{ts}.wav"));
    std::fs::write(&exported_path, &bytes)?;

    // ── 5. Stamp the track's in-memory state ───────────────────────
    {
        let track = &mut project.tracks[track_idx];
        track.duration_secs = dur_secs;
        track.sample_rate = sample_rate;
        track.stereo = true;
        if !file_rel.is_empty() {
            track.file = file_rel;
        }
        match &mut track.source {
            crate::project::TrackSource::Generator {
                last_bake_at,
                last_bake_master_signature,
                ..
            } => {
                *last_bake_at = Some(now_utc);
                *last_bake_master_signature = Some(sig);
            }
            _ => unreachable!("checked above"),
        }
    }

    Ok(exported_path)
}

// Conventional fix is to put `#[cfg(test)] mod` last, but
// `impl eframe::App for TinyBoothApp` lives at the end of this file
// and is much bigger — moving it to silence a stylistic check is the
// wrong trade.
#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod generator_bake_tests {
    //! Exercises [`bake_generator_impl`] without spinning up an egui
    //! context — that's the whole point of factoring it as a free
    //! function. Covers the folder-backing path end-to-end; the .tib
    //! path is the same shape with TibDb in place of fs::write.
    use super::*;
    use crate::project::{GeneratorMode, Project, Track, TrackSource};

    fn temp_root(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("tbss-genbake-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn other_track(id: &str, duration_secs: f32, sample_rate: u32) -> Track {
        Track {
            id: id.into(),
            name: id.into(),
            file: format!("tracks/{id}.wav"),
            mute: false,
            gain_db: 0.0,
            sample_rate,
            channel_source: None,
            duration_secs,
            profile: None,
            stereo: true,
            source: TrackSource::Recorded,
            correction: None,
            gain_automation: None,
            polarity_inverted: false,
            telemetry: None,
            telemetry_profile: crate::telemetry::TelemetryProfile::default(),
        }
    }

    fn generator_track(id: &str) -> Track {
        Track {
            id: id.into(),
            name: "Focus".into(),
            file: String::new(),
            mute: false,
            gain_db: 0.0,
            sample_rate: 48_000,
            channel_source: None,
            duration_secs: 0.0,
            profile: None,
            stereo: true,
            source: TrackSource::Generator {
                mode: GeneratorMode::Binaural {
                    carrier_hz: 200.0,
                    beat_hz: 10.0,
                    amplitude: 0.3,
                },
                last_bake_at: None,
                last_bake_master_signature: None,
            },
            correction: None,
            gain_automation: None,
            polarity_inverted: false,
            telemetry: None,
            telemetry_profile: crate::telemetry::TelemetryProfile::default(),
        }
    }

    #[test]
    fn bake_into_folder_project_writes_wav_export_and_stamps_track() {
        let root = temp_root("ok");
        let mut project = Project::new("test", root.clone());
        // One regular track (duration 0.5 s anchors the bake length)
        // and one Generator track to bake.
        project.tracks.push(other_track("trk-other", 0.5, 8_000));
        project.tracks.push(generator_track("trk-gen"));
        let mut backing = ProjectBacking::Folder;

        let exported =
            bake_generator_impl(&mut project, &mut backing, 1).expect("bake should succeed");

        // ── audio storage ─────────────────────────────────────────
        let bake_wav = root.join("tracks").join("trk-gen.wav");
        assert!(bake_wav.is_file(), "bake WAV at {bake_wav:?} should exist");
        assert!(
            exported.is_file(),
            "timestamped export at {exported:?} should exist"
        );
        assert_eq!(
            std::fs::read(&bake_wav).unwrap(),
            std::fs::read(&exported).unwrap(),
            "exported copy must be byte-identical to the bake"
        );
        // The export landed under <root>/exports/generator-bakes/.
        assert!(exported.starts_with(root.join("exports").join("generator-bakes")));

        // ── manifest stamp ────────────────────────────────────────
        let gen = &project.tracks[1];
        assert_eq!(gen.file, "tracks/trk-gen.wav");
        assert_eq!(gen.sample_rate, 8_000, "matches the other track's rate");
        assert!((gen.duration_secs - 0.5).abs() < 1e-3);
        match &gen.source {
            TrackSource::Generator {
                last_bake_at,
                last_bake_master_signature,
                ..
            } => {
                assert!(last_bake_at.is_some(), "last_bake_at must be stamped");
                assert!(
                    last_bake_master_signature.is_some(),
                    "master signature must be stamped"
                );
                assert_eq!(
                    last_bake_master_signature
                        .unwrap()
                        .longest_other_duration_centisecs,
                    50, // 0.5 s × 100
                );
            }
            _ => panic!("source should still be Generator"),
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bake_with_no_other_tracks_errors_clearly() {
        let root = temp_root("noanchor");
        let mut project = Project::new("test", root.clone());
        project.tracks.push(generator_track("trk-gen"));
        let mut backing = ProjectBacking::Folder;

        let err = bake_generator_impl(&mut project, &mut backing, 0).unwrap_err();
        assert!(
            err.to_string().contains("no other track has a length"),
            "error must surface the no-anchor reason; got: {err}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bake_on_non_generator_track_errors() {
        let root = temp_root("notgen");
        let mut project = Project::new("test", root.clone());
        project.tracks.push(other_track("trk-other", 0.5, 8_000));
        let mut backing = ProjectBacking::Folder;

        let err = bake_generator_impl(&mut project, &mut backing, 0).unwrap_err();
        assert!(
            err.to_string().contains("not a Generator track"),
            "error must refuse non-Generator; got: {err}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

impl eframe::App for TinyBoothApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Cleanse protocol (v0.4.8: hoisted from Mix-tab to top of
        // update so the Project tab and every other surface gets the
        // benefit — orphans stop showing up the moment the user
        // opens a project, regardless of which tab they land on
        // first). Idempotent + cheap on the no-orphan path: a single
        // iter().any() over project.tracks before any mutation.
        self.cleanse_active_project();

        // Initial backfill — runs once, on the first frame after
        // `new()` finishes. Walks the auto-restored project and
        // dispatches analysis for any track without telemetry.
        if self.initial_telemetry_pending {
            self.initial_telemetry_pending = false;
            self.dispatch_telemetry_for_active_project();
        }

        // Drain any telemetry-analysis results the worker thread has
        // produced. Runs every frame; cheap on the all-quiet path
        // (one try_recv that returns Empty). When results arrive the
        // matching tracks get their `telemetry` field populated and
        // the manifest is saved once per drain.
        self.drain_telemetry_results();

        // Update-recheck heartbeat (v0.4.23). Closes the long-standing
        // known issue where the bottom-bar version label stayed stale
        // for the entire session because `check_latest_release` only
        // fired once at startup. Two triggers, rate-limited via
        // `git_update::RECHECK_INTERVAL = 300 s`:
        //   • A 5-minute idle timer.
        //   • Every tab transition — by the time the user switches
        //     between Record / Project / Mix / Export the API call is
        //     cheap relative to all the UI rebuild work, and "I just
        //     came back to the app" usually coincides with a tab click.
        let tab_changed = self.last_tab_seen != Some(self.tab);
        self.last_tab_seen = Some(self.tab);
        if let Some(rx) = crate::git_update::maybe_spawn_recheck(
            &self.update_state,
            &self.update_rx,
            self.last_update_check_at,
            tab_changed,
        ) {
            self.update_state = crate::git_update::UpdateState::Checking;
            self.update_rx = Some(rx);
            self.last_update_check_at = Some(std::time::Instant::now());
        }

        // Repaint continuously while recording so the visualizer animates.
        if self.session.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }

        // F1 toggles the manual. Skipped when a text field has focus so it
        // doesn't fight typing in the Admin window or track-name input.
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.show_manual = !self.show_manual;
        }

        // v0.4.32 — egui requires panels to be declared in a strict
        // order: ALL `TopBottomPanel::top` first, ALL
        // `TopBottomPanel::bottom` second, `CentralPanel` last. The
        // Mix tab's three panels (transport / console / lanes) live
        // alongside the app's global menu + status bars, so we have
        // to interleave their declarations across this function:
        //   1. top_bar (menu)            ← here, line 1321
        //   2. mix_transport_panel       ← just after, conditional
        //   3. bottom_bar (status)       ← below, line ~1591
        //   4. mix_console_panel         ← just after, conditional
        //   5. CentralPanel (tab body)   ← bottom of function
        // v0.4.31 collapsed (2) and (4) into a single `ctx_panels`
        // call placed AFTER bottom_bar — that broke egui's space
        // accounting because the bottom panel claimed before all
        // tops were declared. Hence the lane overlap bug.
        let mix_active = matches!(self.tab, Tab::Mix)
            && !self.show_visualizer
            && !self.project.tracks.is_empty();
        if mix_active {
            // Player rebuild / autoplay / automation capture must
            // happen before the transport panel reads player state.
            ui::mix::pre_render(self);
        }

        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New project…").clicked() {
                        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                            let name = dir
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| "Session".into());
                            self.set_project_root(dir, name);
                            ui.close_menu();
                        }
                    }
                    if ui
                        .button("New TinyDAW project…")
                        .on_hover_text(
                            "Create a new non-Suno, recording-centric project. \
                             Takes captured from the Record tab land directly \
                             inside this project's filespace instead of the \
                             shared recordings filespace. Same Mix tab, same \
                             Export, no Suno context (mixdown reference, \
                             coherence check, role-driven correction chains).",
                        )
                        .clicked()
                    {
                        if let Some(dir) = rfd::FileDialog::new()
                            .set_title("Pick a folder for the TinyDAW project")
                            .pick_folder()
                        {
                            let name = dir
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| "TinyDAW session".into());
                            self.create_tinydaw_project(dir, name);
                            ui.close_menu();
                        }
                    }
                    if ui.button("Open project…").clicked() {
                        self.open_project_dialog();
                        ui.close_menu();
                    }
                    if ui
                        .button("Open Recordings")
                        .on_hover_text(
                            "Switch to the persistent recordings filespace — \
                             the dedicated app-owned location every Record-tab take \
                             lands in. Always available, fully separate from any \
                             stem-mixing project.",
                        )
                        .clicked()
                    {
                        self.open_recordings_project();
                        ui.close_menu();
                    }
                    let mut recent_clicked: Option<PathBuf> = None;
                    let mut clear_recent = false;
                    ui.menu_button("Open Recent", |ui| {
                        if self.config.recent_projects.is_empty() {
                            ui.label(egui::RichText::new("(none yet)").weak());
                        } else {
                            for path in &self.config.recent_projects {
                                let label = path
                                    .parent()
                                    .and_then(|p| p.file_name())
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| path.display().to_string());
                                if ui
                                    .button(label)
                                    .on_hover_text(path.display().to_string())
                                    .clicked()
                                {
                                    recent_clicked = Some(path.clone());
                                    ui.close_menu();
                                }
                            }
                            ui.separator();
                            if ui.button("Clear list").clicked() {
                                clear_recent = true;
                                ui.close_menu();
                            }
                        }
                    });
                    if let Some(p) = recent_clicked {
                        self.open_project_path(&p);
                    }
                    if clear_recent {
                        self.config.clear_recent();
                    }
                    if ui.button("Save").clicked() {
                        self.save_project();
                        ui.close_menu();
                    }
                    if ui
                        .button("Export as single .tib…")
                        .on_hover_text(
                            "Pack this whole project — every stem + the bundled \
                             mixdown — into one self-contained .tib file (a SQLite \
                             database; TBSS-FR-0007). The folder project is left \
                             untouched. Each stem becomes its `orig` revision, the \
                             baseline for future revision history.",
                        )
                        .clicked()
                    {
                        self.export_project_as_tib();
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.label(egui::RichText::new("Import Suno stems").weak());
                    if ui.button("…from folder").clicked() {
                        self.import_suno_folder();
                        ui.close_menu();
                    }
                    if ui.button("…from zip").clicked() {
                        self.import_suno_zip();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .button("Add Generator Track…")
                        .on_hover_text(
                            "Add a synthesised focus-music stem — binaural beats or \
                             isochronic tones — that bakes from parameters and lays \
                             into the mix at the longest other track's duration. \
                             TBSS-FR-0009.",
                        )
                        .clicked()
                    {
                        self.open_add_generator_modal();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        std::process::exit(0);
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui
                        .checkbox(&mut self.config.dark_mode, "Dark mode")
                        .changed()
                    {
                        ctx.set_visuals(if self.config.dark_mode {
                            egui::Visuals::dark()
                        } else {
                            egui::Visuals::light()
                        });
                        self.config.save_or_log();
                    }
                    ui.separator();
                    ui.label("UI scale");
                    // Slider mutates `config.zoom` directly; we re-apply
                    // the egui zoom factor and persist on change. Range
                    // chosen to cover small-laptop (0.75×) through
                    // accessibility-bumped 4K (2.5×). Step 0.05 keeps the
                    // slider usable without feeling jittery.
                    let resp = ui.add(
                        egui::Slider::new(&mut self.config.zoom, 0.75..=2.5)
                            .step_by(0.05)
                            .custom_formatter(|n, _| format!("{:.0}%", n * 100.0)),
                    );
                    if resp.changed() {
                        ctx.set_zoom_factor(self.config.zoom);
                        self.config.save_or_log();
                    }
                    if ui.button("Reset to 100%").clicked() {
                        self.config.zoom = 1.0;
                        ctx.set_zoom_factor(1.0);
                        self.config.save_or_log();
                    }
                });
                ui.menu_button("Admin", |ui| {
                    if ui.button("Recording-tone profiles…").clicked() {
                        self.show_admin = true;
                        if self.admin_edit_idx.is_none() {
                            self.admin_edit_idx = Some(self.active_profile_idx);
                        }
                        ui.close_menu();
                    }
                    if ui.button("Telemetry settings…").clicked() {
                        self.show_telemetry_settings = true;
                        ui.close_menu();
                    }
                    if ui
                        .button("Audio devices…")
                        .on_hover_text(
                            "Pick the master input device (recording) and output \
                             device (Mix-tab playback). Persists across app \
                             restarts.",
                        )
                        .clicked()
                    {
                        self.show_audio_devices = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    let mut show_spec = self.config.show_spectrum_panel;
                    if ui
                        .checkbox(&mut show_spec, "Show spectrum panel (Mix tab)")
                        .on_hover_text(
                            "Pinned at the top of the Mix tab — live FFT of the master \
                             output bus. Log-frequency X axis, dB Y axis, with a slow-\
                             release peak-decay trail.",
                        )
                        .changed()
                    {
                        self.config.show_spectrum_panel = show_spec;
                        self.config.save_or_log();
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("Manual…  (F1)").clicked() {
                        self.show_manual = true;
                        ui.close_menu();
                    }
                });

                ui.separator();
                ui.selectable_value(&mut self.tab, Tab::Record, "Record");
                ui.selectable_value(&mut self.tab, Tab::Project, "Project");
                ui.selectable_value(&mut self.tab, Tab::Mix, "Mix");
                ui.selectable_value(&mut self.tab, Tab::Export, "Export");
                ui.selectable_value(&mut self.tab, Tab::Crossfade, "Crossfade");

                ui.separator();
                // 🌀 Visualizer toggle (v0.4.11). Selectable so it
                // visually highlights while open. Click to take over
                // the central panel with the audio-reactive canvas;
                // click again to dismiss.
                if ui
                    .selectable_label(self.show_visualizer, "🌀")
                    .on_hover_text(
                        "Visualizer — Lissajous, Spectral Mandala, Lorenz attractor, \
                         Chladni cymatics. Toggle to take over the canvas with audio-reactive \
                         visuals; click again to return.",
                    )
                    .clicked()
                {
                    self.show_visualizer = !self.show_visualizer;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if self.project_dirty {
                        format!("● {}", self.project.name)
                    } else {
                        self.project.name.clone()
                    };
                    ui.label(label);
                    // v0.4.22 — playback readings (time / sample rate /
                    // LUFS) collapsed into the top bar as a right-hand
                    // aside next to the project name. Used to live in
                    // the Mix tab's transport bar; moving them up
                    // frees the transport bar to be a tight strip of
                    // controls, and lets the readings stay visible
                    // even when the user is on Project / Export tabs.
                    if let Some(player) = self.player.as_ref() {
                        ui.separator();
                        let pos = player.state.position_secs();
                        let dur = player.state.duration_secs();
                        let m_lufs = player.state.master_momentary_lufs();
                        let i_lufs = player.state.master_integrated_lufs();
                        let m = if m_lufs.is_nan() {
                            "—".to_string()
                        } else {
                            format!("{:+.1}", m_lufs)
                        };
                        let i = if i_lufs.is_nan() {
                            "—".to_string()
                        } else {
                            format!("{:+.1}", i_lufs)
                        };
                        // Fixed-width readout (monospace + explicit
                        // padding) so the eye doesn't have to chase
                        // jittering digits.
                        let txt = format!(
                            "M {:>6}  I {:>6} LUFS   {} Hz   {}/{}",
                            m,
                            i,
                            player.state.sample_rate,
                            crate::ui::mix::fmt_time(pos),
                            crate::ui::mix::fmt_time(dur),
                        );
                        ui.label(egui::RichText::new(txt).monospace().small())
                            .on_hover_text(
                                "Master-bus playback readings (Mix tab):\n\
                                 • M / I = momentary / integrated LUFS (BS.1770-4)\n\
                                 • Sample rate of the playback engine\n\
                                 • Playhead position / total duration\n\
                                 Streaming targets: Spotify −14, Apple Music −16, broadcast −23.",
                            );
                    }
                });
            });
        });

        // ── Mix tab: top panel ─ slot 2 of the panel order ────────
        // Declared immediately after `top_bar` so all tops sit at
        // the top of the screen before any bottom is claimed.
        if mix_active {
            egui::TopBottomPanel::top("mix_transport_panel")
                .resizable(false)
                .show(ctx, |ui| {
                    ui::mix::render_transport(self, ui);
                });
        }

        // Drain any audio-thread errors into the status bar.
        while let Ok(msg) = self.audio_err_rx.try_recv() {
            self.status = Some(format!("audio: {msg}"));
        }

        let mut should_close_for_update = false;
        egui::TopBottomPanel::bottom("bottom_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                should_close_for_update = crate::git_update::render(
                    ui,
                    &mut self.update_state,
                    &mut self.update_error,
                    &mut self.update_rx,
                );
                ui.separator();
                if let Some(s) = self.status.as_ref() {
                    ui.label(s);
                }
                // Telemetry-batch progress (TBSS-FR-0005). Surfaces
                // "Analyzing N/M…" while the worker is busy. Hidden
                // when idle.
                if self.telemetry.has_pending() {
                    if let Some((done, total)) = self.telemetry.progress() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!("📊 Analyzing {done}/{total}…"))
                                    .small()
                                    .color(egui::Color32::from_rgb(180, 220, 240)),
                            );
                        });
                    }
                }
            });
        });

        // ── Mix tab: bottom panel ─ slot 4 of the panel order ────
        // Declared immediately after `bottom_bar` so all bottoms
        // sit at the bottom of the screen before `CentralPanel`
        // claims what's left.
        if mix_active && self.player.is_some() {
            let console_h = ui::mix::compute_console_h(self, ctx);
            egui::TopBottomPanel::bottom("mix_console_panel")
                .resizable(false)
                .exact_height(console_h)
                .show(ctx, |ui| {
                    ui::mix::render_console(self, ui);
                });
        }

        // Modal overlay during the MSI download. The bundled ffmpeg
        // (~120 MB) made the download big enough to warrant a real
        // dialog with rotating tips instead of a tiny "downloading…"
        // bottom-bar label. See src/ui/update_dialog.rs.
        if matches!(
            self.update_state,
            crate::git_update::UpdateState::Downloading(_)
        ) {
            crate::ui::update_dialog::show(ctx);
        }
        if should_close_for_update {
            // Stop any in-flight recording first so the WAV writer
            // finalises its header before Drop. Save is implicit via
            // stop_take which writes the manifest.
            if self.session.is_some() {
                self.stop_take();
            }
            self.config.save_or_log();
            // eframe 0.28: ask the viewport to close — the runtime
            // tears down GLOW + winit, runs Drop on `Self`, exits the
            // event loop. Strictly cleaner than process::exit().
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // ── CentralPanel ─ slot 5 (last) of the panel order ─────
        // ALWAYS declared, regardless of which tab is active. When
        // the Mix tab is active and populated, the CentralPanel
        // hosts the lane stack (with the Mix transport/console
        // already claimed by the top/bottom panels above). Other
        // tabs render their full body here.
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.show_visualizer {
                ui::visualizer::show(self, ui);
                return;
            }
            if mix_active {
                ui::mix::render_lanes(self, ui);
                return;
            }
            match self.tab {
                Tab::Record => ui::record::show(self, ui),
                Tab::Project => ui::project::show(self, ui),
                Tab::Mix => ui::mix::show(self, ui),
                Tab::Export => ui::export::show(self, ui),
                Tab::Crossfade => ui::crossfade::show(self, ui),
            }
        });

        // Mix-tab transport runs continuously while playing — repaint so
        // the playhead animates.
        if let Some(p) = self.player.as_ref() {
            if p.state.play_state() == crate::player::PlayState::Playing {
                ctx.request_repaint_after(std::time::Duration::from_millis(33));
            }
        }

        // Admin window for editing recording-tone profiles.
        if self.show_admin {
            ui::admin::show(self, ctx);
        }

        // Floating manual window — non-modal, doesn't block anything else.
        if self.show_manual {
            ui::manual::show(self, ctx);
        }

        // Per-track Correction editor — also a floating window.
        if self.editing_correction_for.is_some() {
            ui::correction::show(self, ctx);
        }

        // Project-trim panel — opened via the Project tab's Trim
        // button. Modal-style window, isolated from Mix.
        if self.show_trim {
            ui::trim::show(self, ctx);
        }

        // Import-result modal — always shown after an import completes,
        // success or fail. Can't be missed.
        if self.import_dialog.is_some() {
            ui::import_dialog::show(self, ctx);
        }

        // Duplicate-import conflict modal.
        if self.import_conflict.is_some() {
            ui::import_conflict::show(self, ctx);
        }

        // Migrate-to-.tib prompt (shown when a folder project is opened).
        if self.pending_migration.is_some() {
            ui::migrate_to_tib::show(self, ctx);
        }

        // Add-Generator-Track modal (TBSS-FR-0009 step 5).
        if self.pending_generator_modal.is_some() {
            ui::generator_params::show(self, ctx);
        }

        // Project Health modal (TBSS-FR-0005).
        if self.show_health {
            ui::health::show(self, ctx);
        }

        // Telemetry settings modal (Admin → Telemetry settings…).
        if self.show_telemetry_settings {
            ui::telemetry_settings::show(self, ctx);
        }

        // Audio devices modal (Admin → Audio devices…). v0.4.27.
        if self.show_audio_devices {
            ui::audio_devices::show(self, ctx);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.session.is_some() {
            self.stop_take();
        }
    }
}
