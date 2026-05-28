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
    // Wired up in TBSS-FR-0007 phase 2c step 3 (the `.tib` open path).
    #[allow(dead_code)]
    Tib {
        db: TibDb,
    },
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
            recorder: crate::automation::Recorder::default(),
            mix_console_fraction: 0.42,
            recordings_page: 0,
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
    #[allow(dead_code)] // first consumer lands in step 3
    pub fn is_tib(&self) -> bool {
        matches!(self.backing, ProjectBacking::Tib { .. })
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

    pub fn save_project(&mut self) {
        match self.project.save() {
            Ok(()) => {
                let manifest = self.project.manifest_path();
                self.config.record_project(&manifest);
                self.status = Some(format!("saved {}", manifest.display()));
                self.project_dirty = false;
            }
            Err(e) => self.status = Some(format!("save error: {e}")),
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
        if let Some(proj) = outcome.project.as_ref() {
            let manifest = proj.manifest_path();
            self.config.record_project(&manifest);
        }
        if outcome.success {
            if let Some(proj) = outcome.project.clone() {
                self.project = proj;
                self.project_dirty = false;
                self.player = None;
                self.tab = Tab::Project;
                // Kick off background telemetry analysis for every
                // freshly-imported track. Drum / Percussion stems
                // additionally get drum-kit classification.
                self.dispatch_telemetry_for_active_project();
            }
        }
        self.status = Some(if outcome.success {
            format!("Imported into {}", self.project.manifest_path().display())
        } else {
            "Suno import did not produce any tracks — see dialog".into()
        });
        self.import_dialog = Some(outcome);
    }

    /// Walk `self.project.tracks` and dispatch a telemetry analysis
    /// request for every track whose `telemetry` is `None` or whose
    /// `analyzer_version` is older than the current one. The profile
    /// (drum / guitar / bass / etc.) is resolved per track via
    /// `TelemetryProfile::resolve`. Cheap on the all-fresh path —
    /// just iterates and pushes to a channel.
    pub fn dispatch_telemetry_for_active_project(&mut self) {
        let root = self.project.root.clone();
        for t in &self.project.tracks {
            // Skip tracks whose telemetry is already current.
            if let Some(tel) = &t.telemetry {
                if tel.analyzer_version >= crate::telemetry::ANALYZER_VERSION {
                    continue;
                }
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

    pub fn invalidate_telemetry_for_track(&mut self, idx: usize) {
        let Some(track) = self.project.tracks.get_mut(idx) else {
            return;
        };
        track.telemetry = None;
        let profile = track.telemetry_profile.resolve(&track.source);
        let track_id = track.id.clone();
        let abs = self.project.root.join(&track.file);
        if abs.is_file() {
            self.telemetry.dispatch(crate::telemetry::TelemetryRequest {
                project_root: self.project.root.clone(),
                track_id,
                abs_path: abs,
                profile,
                settings: self.telemetry_settings.clone(),
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
                if let Ok(tel) = r.outcome {
                    if let Some(track) = self.project.tracks.iter_mut().find(|t| t.id == r.track_id)
                    {
                        // Verify the WAV path hasn't changed under us
                        // (e.g. Trim moved it). If it has, drop the
                        // result — Trim re-dispatches with the new path.
                        let cur = self.project.root.join(&track.file);
                        if cur == r.abs_path {
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
            if let Err(e) = self.project.save() {
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
            .add_filter("TinyBooth project", &["tinybooth"])
            .pick_file()
        {
            self.open_project_path(&p);
        }
    }

    /// Load a project manifest from a known path. Used by the Open
    /// dialog and by the File → Open Recent submenu.
    pub fn open_project_path(&mut self, path: &Path) {
        match Project::load(path) {
            Ok(proj) => {
                self.config.record_project(path);
                self.project = proj;
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
