//! Multitrack playback player.
//!
//! Owns a single cpal output stream that mixes every project track live
//! through their per-track correction chain (when set and not bypassed).
//! Tracks are pre-loaded into memory at `Player::new` time as `Vec<i16>`
//! and converted to f32 per-sample inside the audio callback — modest
//! memory footprint, zero disk I/O on the hot path.
//!
//! Threading model:
//!   * UI thread mutates per-track state (gain, mute, A/B bypass, current
//!     correction profile) via the atomic / Mutex helpers on `TrackPlay`.
//!   * Audio thread polls `correction_generation` per callback; when it
//!     changes for a track, the audio thread takes a brief lock, clones
//!     the new profile, and rebuilds its locally-owned `FilterChainStereo`.
//!     The chain itself never crosses thread boundaries.
//!
//! The Mix tab reads `position_frames` once per UI frame to draw the
//! synchronized playhead. Position is sample-accurate.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Stream, StreamConfig};
use parking_lot::Mutex;
use std::io::{Cursor, Read};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use std::collections::HashMap;

use crate::automation::{AutomationLane, SplineSampler};
use crate::dsp::{FilterChainStereo, Profile};
use crate::project::Project;
use crate::tib::TibDb;

/// Number of samples summed into one waveform-display peak bin.
/// ~5.3 ms at 48 kHz — plenty for the on-screen waveform without
/// blowing memory on a 5-minute track.
const PEAKS_BIN_SIZE: usize = 256;

/// Where to fetch a track's audio bytes when the owner-thread loads it.
/// Folder projects produce `File(abs_path)`; `.tib` projects produce
/// `TibRev { db_path, rev_id }` — the owner-thread opens its own
/// read-only [`TibDb`] connection (WAL allows concurrent readers
/// alongside the writer the UI thread keeps open) and reads the BLOB
/// via incremental I/O. Both variants are `Send`; the SQLite connection
/// is built on the owner-thread, never crosses it.
#[derive(Clone)]
pub enum AudioSource {
    File(PathBuf),
    TibRev { db_path: PathBuf, rev_id: i64 },
}

/// Per-track audio build inputs, captured cheaply on the UI thread
/// (metadata clones + the source descriptor — no disk I/O). Send, so it
/// can cross to the audio owner-thread where the slow WAV load + flaky
/// cpal device enumeration actually happen. See [`snapshot_project`].
pub struct TrackAudioSnapshot {
    pub source: AudioSource,
    pub name: String,
    /// Free-form display label for diagnostics — the legacy folder
    /// project stores the relative filename here; `.tib` projects store
    /// a synthetic `tib:<rev_id>` tag. Used only in error messages so
    /// the UI can tell tracks apart.
    pub file: String,
    pub gain_db: f32,
    pub mute: bool,
    pub polarity_inverted: bool,
    pub correction: Option<Profile>,
    pub gain_automation: Option<AutomationLane>,
}

/// Whole-project audio build inputs. Everything [`build_player_inner`]
/// needs, in `Send` form, so the build runs off the UI thread.
pub struct ProjectAudioSnapshot {
    pub tracks: Vec<TrackAudioSnapshot>,
    pub master_gain_db: f32,
    pub master_gain_automation: Option<AutomationLane>,
    pub corrections_disabled: bool,
    pub project_track_count: usize,
}

/// Snapshot the live project for an async player build. Runs on the UI
/// thread but does no I/O — just clones per-track metadata and resolves
/// the audio source descriptor, both cheap. The expensive/flaky work
/// (WAV decode, cpal enumeration, stream creation) is deferred to the
/// owner thread.
///
/// When `tib_rev_ids` is `Some`, the project is `.tib`-backed and every
/// track's source becomes `TibRev` keyed by its `current_rev_id` — the
/// `db_path` is `project.root` (which `tib_project::load_project` stamps
/// with the .tib file path). Tracks missing from the map fall through to
/// `File`, which then errors on load and surfaces the per-track skip via
/// the audio-error channel. (The map is the source of truth — if a track
/// has no current revision in the db it can't play.)
pub fn snapshot_project(
    project: &Project,
    tib_rev_ids: Option<&HashMap<String, i64>>,
) -> ProjectAudioSnapshot {
    ProjectAudioSnapshot {
        tracks: project
            .tracks
            .iter()
            .map(|t| {
                let source = match tib_rev_ids.and_then(|m| m.get(&t.id)) {
                    Some(&rev_id) => AudioSource::TibRev {
                        db_path: project.root.clone(),
                        rev_id,
                    },
                    None => AudioSource::File(project.track_abs_path(t)),
                };
                TrackAudioSnapshot {
                    source,
                    name: t.name.clone(),
                    file: t.file.clone(),
                    gain_db: t.gain_db,
                    mute: t.mute,
                    polarity_inverted: t.polarity_inverted,
                    correction: t.correction.clone(),
                    gain_automation: t.gain_automation.clone(),
                }
            })
            .collect(),
        master_gain_db: project.master_gain_db,
        master_gain_automation: project.master_gain_automation.clone(),
        corrections_disabled: project.corrections_disabled,
        project_track_count: project.tracks.len(),
    }
}

/// Top-level transport state. UI sets, audio thread reads.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Stopped = 0,
    Playing = 1,
    Paused = 2,
}

impl PlayState {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Playing,
            2 => Self::Paused,
            _ => Self::Stopped,
        }
    }
}

/// Per-track playback state. Atomics + Mutex are arranged so the audio
/// thread does (cheap) atomic loads on every callback and only takes the
/// `correction_profile` lock when the generation counter increments.
pub struct TrackPlay {
    pub name: String,
    /// Interleaved samples — length = frame_count for mono, 2×frame_count for stereo.
    samples: Vec<i16>,
    pub channels: u16,
    pub sample_rate: u32,
    pub frame_count: u64,

    /// Pre-computed peak table — abs-max per `peaks_bin_size` samples.
    pub peaks: Vec<f32>,
    #[allow(dead_code)] // Used by future click-to-seek logic in Phase 3.
    pub peaks_bin_size: usize,

    gain_db_bits: AtomicU32, // f32 bits
    pub mute: AtomicBool,
    /// When set on any track, every non-solo track is silenced.
    pub solo: AtomicBool,
    /// Polarity flip — when true, the per-buffer cache folds a -1.0
    /// factor into the static linear gain, so the inversion costs zero
    /// extra multiplies in the per-frame hot path. UI mutates via
    /// [`Self::set_polarity_inverted`]; project save copies it back to
    /// `Track.polarity_inverted` so it survives reload.
    pub polarity_inverted: AtomicBool,
    /// When true, the correction chain AND automation are skipped
    /// during playback (raw source for A/B comparison).
    pub bypass_correction: AtomicBool,
    /// When true, the audio thread skips the spline lookup even if a
    /// lane exists — used during re-record so the user's hand is
    /// authoritative.
    pub recording_armed: AtomicBool,

    /// Post-correction post-fader peak in [0, 1000] (×1000 fixed-point).
    /// Driven by the audio thread, read by the console-strip meter.
    pub peak_x1000: AtomicU32,

    /// Current correction profile. UI mutates and bumps the generation.
    correction_profile: Mutex<Option<Profile>>,
    pub correction_generation: AtomicU64,
    /// Cheap "is correction set?" mirror for the UI. `Profile` is
    /// expensive to clone (Strings + EQ array + de-ess + …), so the
    /// lanes view checks this atomic instead of locking + cloning the
    /// `correction_profile` Mutex on every frame. Updated in lockstep
    /// with `set_correction`. v0.4.7.
    correction_present: AtomicBool,

    /// Latest automation lane. Audio thread polls a generation counter
    /// to rebuild its `SplineSampler`. None / empty lane = no automation.
    automation_lane: Mutex<Option<AutomationLane>>,
    pub automation_generation: AtomicU64,
}

impl TrackPlay {
    pub fn gain_db(&self) -> f32 {
        f32::from_bits(self.gain_db_bits.load(Ordering::Relaxed))
    }
    pub fn set_gain_db(&self, db: f32) {
        self.gain_db_bits.store(db.to_bits(), Ordering::Relaxed);
    }

    /// Snapshot the current correction profile by cloning under a
    /// brief lock. Reserved for callers that need the actual `Profile`
    /// (e.g. project-save sync paths). Per-frame UI checks should use
    /// [`Self::has_correction`] instead — this clones the entire
    /// `Profile` (Strings + EQ array + de-ess), which on a 30-fps
    /// repaint with 12 tracks adds up to thousands of heap
    /// allocations per second.
    #[allow(dead_code)] // public API for future non-hot-path callers
    pub fn correction(&self) -> Option<Profile> {
        self.correction_profile.lock().clone()
    }

    /// Cheap presence check — atomic load, no Mutex, no Profile clone.
    /// The lanes view + correction-related buttons use this on every
    /// frame; clone-via-lock would burn allocator time on a hot path
    /// that only needs a yes/no. v0.4.7.
    pub fn has_correction(&self) -> bool {
        self.correction_present.load(Ordering::Relaxed)
    }

    /// Replace the correction chain. Pass `None` to disable correction
    /// for this track. Bumps the generation counter so the audio thread
    /// rebuilds its local `FilterChainStereo` on its next callback.
    pub fn set_correction(&self, profile: Option<Profile>) {
        let present = profile.is_some();
        *self.correction_profile.lock() = profile;
        // Mirror the presence flag for cheap UI reads. Order: write
        // the Mutex first so a UI thread that sees `present=true`
        // can lock-and-clone the actual Profile without racing.
        self.correction_present.store(present, Ordering::Release);
        self.correction_generation.fetch_add(1, Ordering::Release);
    }

    /// Snapshot the current automation lane by cloning under a brief
    /// lock. Reserved for callers that need an owned `AutomationLane`
    /// (e.g. saving the project manifest). Per-frame UI use should go
    /// through [`Self::with_automation`] which holds the lock for a
    /// callback's duration with no allocation.
    #[allow(dead_code)] // public API for future non-hot-path callers
    pub fn automation(&self) -> Option<AutomationLane> {
        self.automation_lane.lock().clone()
    }

    /// Borrow the automation lane via a callback without cloning.
    /// The Mutex is held for the duration of `f`, so the closure
    /// should be cheap (waveform-curve drawing, not arbitrary work).
    /// Avoids the per-frame `Vec<AutomationPoint>` clone that
    /// `automation()` would otherwise do on every lane render.
    /// v0.4.7.
    pub fn with_automation<R>(&self, f: impl FnOnce(Option<&AutomationLane>) -> R) -> R {
        let guard = self.automation_lane.lock();
        f(guard.as_ref())
    }

    pub fn set_automation(&self, lane: Option<AutomationLane>) {
        *self.automation_lane.lock() = lane;
        self.automation_generation.fetch_add(1, Ordering::Release);
    }

    pub fn peak(&self) -> f32 {
        self.peak_x1000.load(Ordering::Relaxed) as f32 / 1000.0
    }
}

/// Shared state behind the player. UI and audio thread both hold an `Arc`.
pub struct PlayerState {
    pub play_state: AtomicU8,
    pub position_frames: AtomicU64,
    pub sample_rate: u32,
    pub longest_frames: u64,
    pub tracks: Vec<Arc<TrackPlay>>,

    // ── Master bus state ──
    /// Master fader gain in dB (UI ↔ audio).
    master_gain_db_bits: AtomicU32,
    /// True while any track is in re-record mode (suppresses some
    /// automation reads to keep the UX honest).
    pub master_recording_armed: AtomicBool,
    /// Master automation lane (Catmull-Rom). Same generation pattern as
    /// per-track automation.
    master_automation_lane: Mutex<Option<AutomationLane>>,
    pub master_automation_generation: AtomicU64,
    /// Post-master-fader peak L / R (×1000 fixed-point) for the master
    /// strip's stereo level meter.
    pub master_peak_l_x1000: AtomicU32,
    pub master_peak_r_x1000: AtomicU32,

    /// Most-recent momentary LUFS (400 ms window) of the master bus,
    /// computed by the audio thread per BS.1770-4 K-weighting and
    /// published to the UI via this atomic. Stored as f32 bits;
    /// `f32::NAN` until 400 ms of audio has been measured. v0.4.0.
    pub master_momentary_lufs_bits: AtomicU32,
    /// Gated integrated LUFS (whole-programme) of the master bus,
    /// updated periodically by the audio thread. Same encoding /
    /// NaN-until-ready semantics as the momentary readout. v0.4.0.
    pub master_integrated_lufs_bits: AtomicU32,

    /// Project-wide bypass. ORed with each track's per-track
    /// bypass_correction in the audio callback — when this is true,
    /// every chain is skipped regardless of per-track state. Set from
    /// `Project.corrections_disabled` at load and from either the
    /// persisted-disable button or the ephemeral A/B button. Added v0.3.4.
    pub global_bypass: AtomicBool,

    /// Master-bus stereo sample tap (v0.4.11). The audio thread pushes
    /// the most recent `OUTPUT_VIZ_LEN` post-fader L/R samples here;
    /// the visualizer canvas (UI thread) snapshots the buffer when
    /// rendering. parking_lot::Mutex keeps the lock window tiny —
    /// well under any cpal callback budget.
    pub output_viz: Mutex<std::collections::VecDeque<(f32, f32)>>,
}

/// Length of the master-bus sample tap in stereo frames. ~85 ms at
/// 48 kHz — enough for the Lissajous trail and the FFT window the
/// spectral-mandala mode wants, small enough that lock contention
/// stays sub-microsecond.
pub const OUTPUT_VIZ_LEN: usize = 4096;

impl PlayerState {
    pub fn play_state(&self) -> PlayState {
        PlayState::from_u8(self.play_state.load(Ordering::Acquire))
    }
    pub fn set_play_state(&self, s: PlayState) {
        self.play_state.store(s as u8, Ordering::Release);
    }
    pub fn position_secs(&self) -> f32 {
        self.position_frames.load(Ordering::Relaxed) as f32 / self.sample_rate.max(1) as f32
    }
    pub fn duration_secs(&self) -> f32 {
        self.longest_frames as f32 / self.sample_rate.max(1) as f32
    }

    pub fn master_gain_db(&self) -> f32 {
        f32::from_bits(self.master_gain_db_bits.load(Ordering::Relaxed))
    }
    pub fn set_master_gain_db(&self, db: f32) {
        self.master_gain_db_bits
            .store(db.to_bits(), Ordering::Relaxed);
    }
    #[allow(dead_code)] // symmetric with set_master_automation, kept for the Phase-3 lane editor
    pub fn master_automation(&self) -> Option<AutomationLane> {
        self.master_automation_lane.lock().clone()
    }
    pub fn set_master_automation(&self, lane: Option<AutomationLane>) {
        *self.master_automation_lane.lock() = lane;
        self.master_automation_generation
            .fetch_add(1, Ordering::Release);
    }
    pub fn master_peak_left(&self) -> f32 {
        self.master_peak_l_x1000.load(Ordering::Relaxed) as f32 / 1000.0
    }
    pub fn master_peak_right(&self) -> f32 {
        self.master_peak_r_x1000.load(Ordering::Relaxed) as f32 / 1000.0
    }
    pub fn master_momentary_lufs(&self) -> f32 {
        f32::from_bits(self.master_momentary_lufs_bits.load(Ordering::Relaxed))
    }
    pub fn master_integrated_lufs(&self) -> f32 {
        f32::from_bits(self.master_integrated_lufs_bits.load(Ordering::Relaxed))
    }
    /// Whether any track is currently soloed.
    pub fn any_solo(&self) -> bool {
        self.tracks.iter().any(|t| t.solo.load(Ordering::Relaxed))
    }

    #[allow(dead_code)] // Used by Phase-3 click-to-seek on the Mix tab.
    pub fn seek_frames(&self, frames: u64) {
        self.position_frames
            .store(frames.min(self.longest_frames), Ordering::Release);
    }
}

/// The owning handle. Drop = stream stop + cleanup.
pub struct Player {
    pub state: Arc<PlayerState>,
    /// Number of tracks the project had at build time. The Mix-tab
    /// rebuild check keys on this rather than `state.tracks.len()`
    /// — a tolerant load (v0.4.4) may produce fewer surviving tracks
    /// than the project carries, and we don't want a perpetual rebuild
    /// loop on every Mix-tab render when one row is permanently bad.
    pub project_track_count: usize,
    /// Dropping this `Sender` closes the channel the audio owner-thread
    /// parks on, which makes that thread drop its cpal `Stream` and exit.
    /// That *is* the teardown path — there's no explicit stop call, and
    /// the `!Send` `Stream` never has to cross a thread boundary.
    _stop_tx: Sender<()>,
}

/// Phase 1 of the audio build: load every track's WAV into memory and
/// assemble the shared `PlayerState` (peaks included). **No audio device
/// is touched here** — this is the v0.4.40 split that lets the Mix tab
/// render its lanes the instant the WAVs are decoded, whether or not an
/// output device is present or healthy. Runs on the audio owner-thread.
///
/// `error_tx` is a clone of the app's audio-error channel; per-track skip
/// warnings are routed through it so the UI surfaces them.
fn build_state(snap: &ProjectAudioSnapshot, error_tx: &Sender<String>) -> Result<Arc<PlayerState>> {
    if snap.tracks.is_empty() {
        return Err(anyhow!("project has no tracks"));
    }

    // Load tracks tolerantly. v0.4.4: per-track failures (missing
    // file, corrupt WAV) get skipped with a warning routed through
    // `error_tx`. v0.4.5: the per-track conformance check covers
    // BOTH rate AND length — Suno stems are co-rendered so they
    // share a single rate and a single length. A stem whose length
    // differs from the rest by more than `MAX_LENGTH_JITTER_SECS`
    // is by definition an alien (a stray recording, a different-
    // generation take, etc.) and gets the same skip-and-warn
    // treatment as a rate mismatch. The first successful track
    // sets the project's reference rate + length; subsequent
    // tracks must match within tolerance.
    //
    // Tolerance: 100 ms. Tight enough to obviously catch orphan
    // recordings (typically seconds different) without rejecting
    // legitimate Suno stems that may differ by a few frames due
    // to codec-level packet alignment.
    const MAX_LENGTH_JITTER_SECS: f32 = 0.1;

    let mut tracks = Vec::with_capacity(snap.tracks.len());
    let mut sample_rate = 0u32;
    let mut reference_frames: u64 = 0;
    let mut longest_frames = 0u64;
    for t in &snap.tracks {
        let tp = match load_track_play(t) {
            Ok(tp) => tp,
            Err(e) => {
                let _ = error_tx.send(format!("skipped track '{}' ({}): {:#}", t.name, t.file, e));
                continue;
            }
        };
        if sample_rate == 0 {
            // First successful track: it sets both the rate and
            // the reference length the rest must match.
            sample_rate = tp.sample_rate;
            reference_frames = tp.frame_count;
        } else {
            let rate_ok = tp.sample_rate == sample_rate;
            let stem_secs = tp.frame_count as f32 / tp.sample_rate.max(1) as f32;
            let proj_secs = reference_frames as f32 / sample_rate.max(1) as f32;
            let length_ok = (stem_secs - proj_secs).abs() <= MAX_LENGTH_JITTER_SECS;
            if !rate_ok || !length_ok {
                let mut whys: Vec<String> = Vec::new();
                if !rate_ok {
                    whys.push(format!(
                        "rate {} Hz vs project {} Hz",
                        tp.sample_rate, sample_rate
                    ));
                }
                if !length_ok {
                    whys.push(format!(
                        "length {:.2}s vs project {:.2}s",
                        stem_secs, proj_secs
                    ));
                }
                let _ = error_tx.send(format!(
                    "skipped track '{}': {} (resampling / length-fixup not yet supported)",
                    t.name,
                    whys.join("; ")
                ));
                continue;
            }
        }
        longest_frames = longest_frames.max(tp.frame_count);
        tracks.push(Arc::new(tp));
    }
    if tracks.is_empty() {
        return Err(anyhow!(
            "no tracks loaded successfully — see status bar for per-track reasons"
        ));
    }

    let state = Arc::new(PlayerState {
        play_state: AtomicU8::new(PlayState::Stopped as u8),
        position_frames: AtomicU64::new(0),
        sample_rate,
        longest_frames,
        tracks,
        master_gain_db_bits: AtomicU32::new(snap.master_gain_db.to_bits()),
        master_recording_armed: AtomicBool::new(false),
        master_automation_lane: Mutex::new(snap.master_gain_automation.clone()),
        master_automation_generation: AtomicU64::new(1),
        master_peak_l_x1000: AtomicU32::new(0),
        master_peak_r_x1000: AtomicU32::new(0),
        master_momentary_lufs_bits: AtomicU32::new(f32::NAN.to_bits()),
        master_integrated_lufs_bits: AtomicU32::new(f32::NAN.to_bits()),
        global_bypass: AtomicBool::new(snap.corrections_disabled),
        output_viz: Mutex::new(std::collections::VecDeque::with_capacity(OUTPUT_VIZ_LEN)),
    });

    Ok(state)
}

/// Phase 2 of the audio build: probe the output device and create the
/// live cpal `Stream`. This is the slow / flaky / panic-prone part (cpal
/// device enumeration on a bad driver), kept entirely separate from
/// [`build_state`] so it can fail — or hang, or panic — without ever
/// taking the Mix display down. Runs on the audio owner-thread.
fn build_stream(
    state: Arc<PlayerState>,
    error_tx: Sender<String>,
    output_device_name: Option<&str>,
) -> Result<Stream> {
    if crate::audio::output_device_by_name(output_device_name).is_none() {
        return Err(anyhow!(
            "no audio output device — pick one in Admin → Audio devices… \
             or connect headphones/speakers and click Retry above"
        ));
    }
    let stream = build_output_stream(state, error_tx, output_device_name)?;
    stream.play().context("starting cpal output stream")?;
    Ok(stream)
}

/// Messages from the audio owner-thread to the UI during a build.
pub enum BuildMsg {
    /// Track WAVs are decoded and the shared state (peaks included) is
    /// ready — the Mix lanes can render *now*, before the output device
    /// is even probed. Carries the playback handle.
    Loaded(Player),
    /// The cpal output stream is live; playback (Play) is now possible.
    StreamReady,
    /// The output device / stream build failed, hung-then-errored, or
    /// panicked. If a `Loaded` was already delivered the lanes stay
    /// visible and this just drives the "no audio output" banner.
    StreamFailed(String),
}

/// Spawn the audio owner-thread. **Two-phase** (v0.4.40): it first loads
/// the track state and hands the UI a `Player` (`BuildMsg::Loaded`) so
/// the Mix lanes render immediately — *before* any audio device is
/// touched — then probes the device and builds the cpal stream
/// (`StreamReady` / `StreamFailed`). The owner-thread holds the `!Send`
/// `Stream` alive (parked) until the UI drops its `Player`. All the
/// slow / flaky / panic-prone cpal work happens here, off the UI thread;
/// a panic is caught and reported, and the global hook (main.rs) logs the
/// backtrace to `logs/panic.log`.
pub fn spawn_build(
    snapshot: ProjectAudioSnapshot,
    error_tx: Sender<String>,
    output_device_name: Option<String>,
) -> Receiver<BuildMsg> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("tbss-audio-owner".into())
        .spawn(move || {
            // ── Phase 1: load track state (no device) ────────────────
            let state = match std::panic::catch_unwind(AssertUnwindSafe(|| {
                build_state(&snapshot, &error_tx)
            })) {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    let _ = tx.send(BuildMsg::StreamFailed(format!("{e:#}")));
                    return;
                }
                Err(_) => {
                    let _ = tx.send(BuildMsg::StreamFailed(
                        "track load panicked (see logs/panic.log)".to_string(),
                    ));
                    return;
                }
            };
            // Hand the display handle to the UI immediately — lanes render
            // before the (slow/flaky) output device is even probed.
            let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
            let player = Player {
                state: state.clone(),
                project_track_count: snapshot.project_track_count,
                _stop_tx: stop_tx,
            };
            if tx.send(BuildMsg::Loaded(player)).is_err() {
                return; // UI gone before we finished.
            }
            // ── Phase 2: build the cpal stream (slow / flaky / panicky) ─
            match std::panic::catch_unwind(AssertUnwindSafe(|| {
                build_stream(
                    state.clone(),
                    error_tx.clone(),
                    output_device_name.as_deref(),
                )
            })) {
                Ok(Ok(stream)) => {
                    let _ = tx.send(BuildMsg::StreamReady);
                    let _ = stop_rx.recv(); // park, keeping the !Send Stream alive
                    drop(stream);
                }
                Ok(Err(e)) => {
                    let _ = tx.send(BuildMsg::StreamFailed(format!("{e:#}")));
                }
                Err(_) => {
                    let _ = tx.send(BuildMsg::StreamFailed(
                        "audio output init panicked — likely a flaky or virtual \
                         device driver. Pick a different device in Admin → Audio \
                         devices…, then Retry. (Details in logs/panic.log.)"
                            .to_string(),
                    ));
                }
            }
        });
    rx
}

impl Player {
    pub fn play(&self) {
        if self.state.play_state() == PlayState::Stopped {
            self.state.position_frames.store(0, Ordering::Release);
        }
        self.state.set_play_state(PlayState::Playing);
    }
    pub fn pause(&self) {
        self.state.set_play_state(PlayState::Paused);
    }
    pub fn stop(&self) {
        self.state.set_play_state(PlayState::Stopped);
        self.state.position_frames.store(0, Ordering::Release);
    }
}

// ───────────────────── helpers ─────────────────────

fn load_track_play(t: &TrackAudioSnapshot) -> Result<TrackPlay> {
    let (spec, samples, frame_count) = match &t.source {
        AudioSource::File(path) => {
            let reader = hound::WavReader::open(path)
                .with_context(|| format!("reading track {}", path.display()))?;
            decode_wav(reader).with_context(|| format!("decoding track {}", path.display()))?
        }
        AudioSource::TibRev { db_path, rev_id } => {
            // WAL allows this read connection alongside the app's writer.
            let db = TibDb::open(db_path)
                .with_context(|| format!("opening .tib for playback: {}", db_path.display()))?;
            let bytes = db
                .read_revision_audio(*rev_id)
                .with_context(|| format!("reading revision {rev_id} from {}", db_path.display()))?;
            let reader = hound::WavReader::new(Cursor::new(bytes))
                .with_context(|| format!("parsing in-memory WAV for rev {rev_id}"))?;
            decode_wav(reader)
                .with_context(|| format!("decoding in-memory WAV for rev {rev_id}"))?
        }
    };
    let channels = spec.channels.max(1);
    let peaks = compute_peaks(&samples, channels as usize, PEAKS_BIN_SIZE);

    Ok(TrackPlay {
        name: t.name.clone(),
        solo: AtomicBool::new(false),
        recording_armed: AtomicBool::new(false),
        peak_x1000: AtomicU32::new(0),
        automation_lane: Mutex::new(t.gain_automation.clone()),
        automation_generation: AtomicU64::new(1),
        samples,
        channels,
        sample_rate: spec.sample_rate,
        frame_count,
        peaks,
        peaks_bin_size: PEAKS_BIN_SIZE,
        gain_db_bits: AtomicU32::new(t.gain_db.to_bits()),
        mute: AtomicBool::new(t.mute),
        polarity_inverted: AtomicBool::new(t.polarity_inverted),
        bypass_correction: AtomicBool::new(false),
        correction_profile: Mutex::new(t.correction.clone()),
        correction_present: AtomicBool::new(t.correction.is_some()),
        correction_generation: AtomicU64::new(1), // ≠0 forces audio thread to build chain on first callback
    })
}

/// Decode a WAV stream into the i16-interleaved buffer the audio thread
/// reads. Shared by the file-on-disk path (folder projects) and the
/// in-memory `Cursor<Vec<u8>>` path (.tib BLOBs). Returns the spec,
/// the decoded samples, and the frame count.
fn decode_wav<R: Read>(mut reader: hound::WavReader<R>) -> Result<(hound::WavSpec, Vec<i16>, u64)> {
    let spec = reader.spec();
    let frame_count = (reader.duration() as u64).max(1);

    // Read everything as i16. hound's into_samples::<i16>() works for
    // 16-bit Int files; Suno occasionally exports 24-bit which we
    // currently downsize via i32::clamp(i16). This is fine for playback.
    let samples: Vec<i16> = match spec.sample_format {
        hound::SampleFormat::Int => {
            if spec.bits_per_sample == 16 {
                reader.samples::<i16>().filter_map(|r| r.ok()).collect()
            } else {
                reader
                    .samples::<i32>()
                    .filter_map(|r| r.ok())
                    .map(|s| s.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
                    .collect()
            }
        }
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .filter_map(|r| r.ok())
            .map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect(),
    };
    Ok((spec, samples, frame_count))
}

/// Abs-max per bin across however many channels the file has.
/// One value per bin — the visualiser doesn't render L/R lanes
/// separately, just the envelope.
fn compute_peaks(samples: &[i16], channels: usize, bin: usize) -> Vec<f32> {
    if samples.is_empty() || bin == 0 {
        return Vec::new();
    }
    let frames = samples.len() / channels.max(1);
    let bins = frames.div_ceil(bin);
    let mut out = Vec::with_capacity(bins);
    let denom = i16::MAX as f32;
    for b in 0..bins {
        let f0 = b * bin;
        let f1 = ((b + 1) * bin).min(frames);
        let mut peak = 0.0f32;
        for f in f0..f1 {
            for c in 0..channels {
                let s = samples[f * channels + c] as f32 / denom;
                let a = s.abs();
                if a > peak {
                    peak = a;
                }
            }
        }
        out.push(peak);
    }
    out
}

fn build_output_stream(
    state: Arc<PlayerState>,
    error_tx: std::sync::mpsc::Sender<String>,
    output_device_name: Option<&str>,
) -> Result<Stream> {
    let dev = crate::audio::output_device_by_name(output_device_name)
        .ok_or_else(|| anyhow!("no output device available"))?;

    // Try to match the project's sample rate exactly. Fall back to the
    // device's default config if unsupported (Phase 2 doesn't resample —
    // documented in TBSS-FR-0002 §6).
    let supported = dev
        .supported_output_configs()
        .context("listing output configs")?
        .filter(|c| c.channels() >= 2)
        .find_map(|c| {
            if c.min_sample_rate().0 <= state.sample_rate
                && c.max_sample_rate().0 >= state.sample_rate
            {
                Some(c.with_sample_rate(cpal::SampleRate(state.sample_rate)))
            } else {
                None
            }
        });
    let config: StreamConfig = match supported {
        Some(s) => s.into(),
        None => dev
            .default_output_config()
            .context("default output config")?
            .into(),
    };

    let err_fn = move |e: cpal::StreamError| {
        let _ = error_tx.send(format!("output stream error: {e}"));
    };

    // ── Closure-owned audio-thread state. Allocated once at stream
    //    creation; the callback NEVER allocates (cf. TBSS-FR-0004
    //    follow-up Rust pass — all per-callback Vec allocs gone).
    let n = state.tracks.len();
    let mut chains: Vec<Option<FilterChainStereo>> = (0..n).map(|_| None).collect();
    let mut samplers: Vec<SplineSampler> = (0..n).map(|_| SplineSampler::default()).collect();
    let mut seen_corr_gen: Vec<u64> = vec![0; n];
    let mut seen_auto_gen: Vec<u64> = vec![0; n];
    let mut master_sampler = SplineSampler::default();
    let mut seen_master_auto_gen: u64 = 0;
    // Per-track per-buffer cache of values that DON'T change inside
    // a single callback. Loaded once per buffer instead of once per
    // sample — turns ~5 atomic loads × n_tracks × n_frames per buffer
    // into ~5 atomic loads × n_tracks per buffer (~250× fewer loads
    // for typical 256-frame buffers).
    let mut buf_cache: Vec<TrackBufCache> = vec![TrackBufCache::default(); n];
    let mut track_peaks: Vec<f32> = vec![0.0; n];
    let sample_rate_f = state.sample_rate as f32;
    // Master-bus LUFS meter, owned by the audio thread; pushed once
    // per stereo frame, polled at end-of-callback to publish atomic
    // readouts to the UI. Reset on Stop so each playback starts fresh.
    let mut lufs_meter = crate::lufs::LufsMeter::new(state.sample_rate);
    // Buffer-level latch: if the previous callback saw `Stopped`,
    // reset the meter on the next `Playing` so a re-press of Play
    // doesn't accumulate across stops.
    let mut prev_play_state = state.play_state();

    let stream = dev.build_output_stream(
        &config,
        move |out: &mut [f32], _| {
            let frames = out.len() / 2;

            // Rebuild correction chain / spline samplers whose generation changed.
            for (i, t) in state.tracks.iter().enumerate() {
                let cg = t.correction_generation.load(Ordering::Acquire);
                if seen_corr_gen[i] != cg {
                    let p = t.correction_profile.lock().clone();
                    chains[i] = p.map(|p| FilterChainStereo::new(p, state.sample_rate));
                    seen_corr_gen[i] = cg;
                }
                let ag = t.automation_generation.load(Ordering::Acquire);
                if seen_auto_gen[i] != ag {
                    let lane = t.automation_lane.lock().clone();
                    samplers[i] = match lane {
                        Some(l) => SplineSampler::build(&l),
                        None => SplineSampler::default(),
                    };
                    seen_auto_gen[i] = ag;
                }
            }
            let mg = state.master_automation_generation.load(Ordering::Acquire);
            if seen_master_auto_gen != mg {
                let lane = state.master_automation_lane.lock().clone();
                master_sampler = match lane {
                    Some(l) => SplineSampler::build(&l),
                    None => SplineSampler::default(),
                };
                seen_master_auto_gen = mg;
            }

            let play_state = state.play_state();
            let mut pos = state.position_frames.load(Ordering::Acquire);
            let any_solo = state.any_solo();
            let master_armed = state.master_recording_armed.load(Ordering::Relaxed);
            let global_bypass = state.global_bypass.load(Ordering::Relaxed);

            // ── Per-buffer cache (one atomic-load fan-out per track) ──
            // Polarity becomes a ±1.0 sign factor we fold into the
            // static linear gain *and* the automation gain branch, so
            // the per-frame hot path costs zero extra multiplies.
            for (i, t) in state.tracks.iter().enumerate() {
                let muted = t.mute.load(Ordering::Relaxed);
                let solo = t.solo.load(Ordering::Relaxed);
                let bypass = global_bypass || t.bypass_correction.load(Ordering::Relaxed);
                let armed = t.recording_armed.load(Ordering::Relaxed);
                let gain_db = t.gain_db();
                let polarity_sign = if t.polarity_inverted.load(Ordering::Relaxed) {
                    -1.0
                } else {
                    1.0
                };
                buf_cache[i] = TrackBufCache {
                    skip: muted || (any_solo && !solo),
                    bypass,
                    armed,
                    static_gain_db: gain_db,
                    static_gain_lin: polarity_sign * db_to_lin(gain_db),
                    polarity_sign,
                    has_chain: chains[i].is_some(),
                    has_automation: !samplers[i].is_empty(),
                };
            }
            let master_static_db = state.master_gain_db();
            let master_static_lin = db_to_lin(master_static_db);
            let master_has_auto = !master_sampler.is_empty();

            // Reset per-track peak running maxes for this buffer.
            for p in track_peaks.iter_mut() {
                *p = 0.0;
            }
            let mut peak_l = 0.0f32;
            let mut peak_r = 0.0f32;

            for f in 0..frames {
                let mut l_sum = 0.0f32;
                let mut r_sum = 0.0f32;

                if play_state == PlayState::Playing && pos < state.longest_frames {
                    let t_secs = pos as f32 / sample_rate_f;
                    for (i, t) in state.tracks.iter().enumerate() {
                        let c = &buf_cache[i];
                        if c.skip {
                            continue;
                        }
                        if pos >= t.frame_count {
                            continue;
                        }
                        let (l_raw, r_raw) = read_frame(t, pos);
                        let (l, r) = if !c.bypass && c.has_chain {
                            // SAFETY: has_chain == true ⇒ chains[i] is Some.
                            chains[i].as_mut().unwrap().process(l_raw, r_raw)
                        } else {
                            (l_raw, r_raw)
                        };
                        // Static gain dominates: pre-computed in buf_cache.
                        // Only re-derive when automation is active and not
                        // overridden by bypass / arm.
                        let g = if c.has_automation && !c.bypass && !c.armed {
                            let auto_db = samplers[i].sample(t_secs).unwrap_or(c.static_gain_db);
                            // Static path bakes polarity into static_gain_lin;
                            // the automation path has to fold it in here too.
                            c.polarity_sign * db_to_lin(auto_db)
                        } else {
                            c.static_gain_lin
                        };
                        let post_l = l * g;
                        let post_r = r * g;
                        l_sum += post_l;
                        r_sum += post_r;
                        let peak = post_l.abs().max(post_r.abs());
                        if peak > track_peaks[i] {
                            track_peaks[i] = peak;
                        }
                    }
                    pos += 1;
                }

                // Master fader + automation. Static path uses the
                // pre-computed linear gain; only the automation branch
                // does a per-frame db_to_lin.
                let master_g = if master_has_auto && !master_armed {
                    let db = master_sampler
                        .sample(pos as f32 / sample_rate_f)
                        .unwrap_or(master_static_db);
                    db_to_lin(db)
                } else {
                    master_static_lin
                };
                let out_l = (l_sum * master_g).clamp(-1.0, 1.0);
                let out_r = (r_sum * master_g).clamp(-1.0, 1.0);

                if out_l.abs() > peak_l {
                    peak_l = out_l.abs();
                }
                if out_r.abs() > peak_r {
                    peak_r = out_r.abs();
                }

                // Feed the K-weighted master into the LUFS meter only
                // while playback is live — pushing silence between
                // takes would drag integrated_lufs toward NaN once the
                // gating kicks in. Cheap (two biquads × stereo = ~10
                // muls + adds per frame).
                if play_state == PlayState::Playing {
                    lufs_meter.push(out_l, out_r);
                }

                // Master-bus sample tap for the visualizer (v0.4.11).
                // Always pushed — even at silence — so the viz canvas
                // shows a stable centre dot rather than NaN'ing out
                // when nothing's playing. Brief lock; parking_lot
                // makes the contention path fast.
                {
                    let mut buf = state.output_viz.lock();
                    if buf.len() >= OUTPUT_VIZ_LEN {
                        buf.pop_front();
                    }
                    buf.push_back((out_l, out_r));
                }

                out[f * 2] = out_l;
                out[f * 2 + 1] = out_r;
            }

            // Publish peaks (fast attack — overwrite max; slow release
            // happens by UI sampling rate driving toward 0).
            for (i, p) in track_peaks.iter().enumerate() {
                let new = (p.min(1.0) * 1000.0) as u32;
                let cur = state.tracks[i].peak_x1000.load(Ordering::Relaxed);
                let next = if new > cur {
                    new
                } else {
                    cur.saturating_sub(8)
                };
                state.tracks[i].peak_x1000.store(next, Ordering::Relaxed);
            }
            {
                let new_l = (peak_l.min(1.0) * 1000.0) as u32;
                let cur_l = state.master_peak_l_x1000.load(Ordering::Relaxed);
                state.master_peak_l_x1000.store(
                    if new_l > cur_l {
                        new_l
                    } else {
                        cur_l.saturating_sub(8)
                    },
                    Ordering::Relaxed,
                );
                let new_r = (peak_r.min(1.0) * 1000.0) as u32;
                let cur_r = state.master_peak_r_x1000.load(Ordering::Relaxed);
                state.master_peak_r_x1000.store(
                    if new_r > cur_r {
                        new_r
                    } else {
                        cur_r.saturating_sub(8)
                    },
                    Ordering::Relaxed,
                );
            }

            // Publish LUFS readouts (cheap — sums + log10s, no allocs).
            // momentary_lufs/integrated_lufs return NaN until enough
            // audio has been accumulated; the UI just shows "—".
            state
                .master_momentary_lufs_bits
                .store(lufs_meter.momentary_lufs().to_bits(), Ordering::Relaxed);
            state
                .master_integrated_lufs_bits
                .store(lufs_meter.integrated_lufs().to_bits(), Ordering::Relaxed);

            // Stop / reset transitions: clear the LUFS block history so a
            // new playback doesn't include the tail of the previous one.
            // Filter state stays (re-zeroing it would re-introduce a
            // transient on every Play).
            if prev_play_state != PlayState::Stopped && play_state == PlayState::Stopped {
                lufs_meter.reset_blocks();
                state
                    .master_momentary_lufs_bits
                    .store(f32::NAN.to_bits(), Ordering::Relaxed);
                state
                    .master_integrated_lufs_bits
                    .store(f32::NAN.to_bits(), Ordering::Relaxed);
            }
            prev_play_state = play_state;

            // End-of-track: stop and reset.
            if play_state == PlayState::Playing && pos >= state.longest_frames {
                state.set_play_state(PlayState::Stopped);
                pos = 0;
            }
            state.position_frames.store(pos, Ordering::Release);
        },
        err_fn,
        None,
    )?;
    Ok(stream)
}

/// Per-buffer per-track cache. Lives in the audio callback's closure
/// state; refreshed once per buffer so the per-sample inner loop avoids
/// re-loading atomics and re-computing `db_to_lin` for unchanged values.
#[derive(Debug, Clone, Copy, Default)]
struct TrackBufCache {
    /// True when this track is excluded from the mix (mute, or solo
    /// active elsewhere and this track isn't soloed).
    skip: bool,
    /// True when this track's correction chain (and automation) should
    /// be skipped — global_bypass OR per-track bypass_correction.
    bypass: bool,
    /// True when the track is currently arming an automation lane
    /// recording — disables automation playback so the user's hand is
    /// authoritative.
    armed: bool,
    /// Pre-cached fader value (dB) for the rare automation-active path.
    static_gain_db: f32,
    /// Pre-cached fader value already converted to linear — used by
    /// every sample whose gain isn't being driven by automation. **Has
    /// `polarity_sign` already folded in**, so the static path needs no
    /// extra multiply. The automation path still has to fold polarity
    /// in by hand because it derives gain from a per-frame spline sample.
    static_gain_lin: f32,
    /// ±1.0 — `−1.0` when the track's polarity flip is on. Stored
    /// separately from `static_gain_lin` so the automation-active branch
    /// can also apply it without re-reading the atomic per frame.
    polarity_sign: f32,
    /// Whether the track has a non-empty correction chain installed.
    has_chain: bool,
    /// Whether the track's automation sampler can produce values.
    has_automation: bool,
}

/// Read one frame at the given position. Mono tracks pan to centre
/// (same sample to both channels). Stereo tracks return interleaved L,R.
fn read_frame(t: &TrackPlay, pos: u64) -> (f32, f32) {
    let denom = i16::MAX as f32;
    if t.channels >= 2 {
        let i = (pos as usize) * 2;
        if i + 1 >= t.samples.len() {
            return (0.0, 0.0);
        }
        (t.samples[i] as f32 / denom, t.samples[i + 1] as f32 / denom)
    } else {
        let i = pos as usize;
        if i >= t.samples.len() {
            return (0.0, 0.0);
        }
        let s = t.samples[i] as f32 / denom;
        (s, s)
    }
}

fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[cfg(test)]
mod tib_source_tests {
    //! TBSS-FR-0007 phase 2c step 1: verify `load_track_play` can decode
    //! a WAV BLOB out of a `.tib` via `AudioSource::TibRev`. This is the
    //! stand-alone check that the player can run off the new audio source
    //! without any UI / device wiring yet.
    use super::*;
    use crate::tib::{RevKind, TibDb};
    use hound::{SampleFormat, WavSpec, WavWriter};
    use std::io::Cursor;
    use std::path::PathBuf;

    fn make_wav_bytes(samples: &[i16], rate: u32, channels: u16) -> Vec<u8> {
        let spec = WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = WavWriter::new(Cursor::new(&mut buf), spec).unwrap();
            for s in samples {
                w.write_sample(*s).unwrap();
            }
            w.finalize().unwrap();
        }
        buf
    }

    fn scratch_tib(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("tbss-player-{}-{}.tib", name, std::process::id()));
        for suf in ["", "-wal", "-shm"] {
            let mut q = p.as_os_str().to_os_string();
            q.push(suf);
            let _ = std::fs::remove_file(PathBuf::from(q));
        }
        p
    }

    fn cleanup(p: &std::path::Path) {
        for suf in ["", "-wal", "-shm"] {
            let mut q = p.as_os_str().to_os_string();
            q.push(suf);
            let _ = std::fs::remove_file(PathBuf::from(q));
        }
    }

    #[test]
    fn load_track_play_from_tib_rev_round_trips_stereo() {
        let path = scratch_tib("stereo");
        let frames: Vec<i16> = (0..16).flat_map(|i: i16| [i * 100, -i * 100]).collect();
        let wav = make_wav_bytes(&frames, 48_000, 2);
        let rid;
        {
            let db = TibDb::create(&path).unwrap();
            db.insert_stem("s1", "Vox", 0).unwrap();
            db.insert_track("t1", "s1", "Take 1", 0).unwrap();
            rid = db
                .insert_revision(
                    "t1",
                    RevKind::Orig,
                    "import",
                    48_000,
                    true,
                    16.0 / 48_000.0,
                    &wav,
                )
                .unwrap();
            db.set_current_rev("t1", rid).unwrap();
        }

        let snap = TrackAudioSnapshot {
            source: AudioSource::TibRev {
                db_path: path.clone(),
                rev_id: rid,
            },
            name: "Take 1".into(),
            file: format!("tib:{rid}"),
            gain_db: 0.0,
            mute: false,
            polarity_inverted: false,
            correction: None,
            gain_automation: None,
        };
        let tp = load_track_play(&snap).expect("load from .tib BLOB should succeed");

        assert_eq!(tp.channels, 2);
        assert_eq!(tp.sample_rate, 48_000);
        assert_eq!(tp.frame_count, 16, "16 frames written");
        assert_eq!(tp.samples.len(), 32, "stereo: 2 samples per frame");
        // Spot-check round-trip fidelity at a handful of points.
        assert_eq!(tp.samples[0], 0);
        assert_eq!(tp.samples[1], 0);
        assert_eq!(tp.samples[2], 100);
        assert_eq!(tp.samples[3], -100);
        assert_eq!(tp.samples[30], 1500);
        assert_eq!(tp.samples[31], -1500);

        cleanup(&path);
    }

    #[test]
    fn load_track_play_from_tib_rev_round_trips_mono() {
        let path = scratch_tib("mono");
        let frames: Vec<i16> = (0..8).map(|i: i16| i * 1000).collect();
        let wav = make_wav_bytes(&frames, 44_100, 1);
        let rid;
        {
            let db = TibDb::create(&path).unwrap();
            db.insert_stem("s1", "Drums", 0).unwrap();
            db.insert_track("t1", "s1", "Kick", 0).unwrap();
            rid = db
                .insert_revision(
                    "t1",
                    RevKind::Orig,
                    "import",
                    44_100,
                    false,
                    8.0 / 44_100.0,
                    &wav,
                )
                .unwrap();
            db.set_current_rev("t1", rid).unwrap();
        }

        let snap = TrackAudioSnapshot {
            source: AudioSource::TibRev {
                db_path: path.clone(),
                rev_id: rid,
            },
            name: "Kick".into(),
            file: format!("tib:{rid}"),
            gain_db: 0.0,
            mute: false,
            polarity_inverted: false,
            correction: None,
            gain_automation: None,
        };
        let tp = load_track_play(&snap).expect("mono load");

        assert_eq!(tp.channels, 1);
        assert_eq!(tp.sample_rate, 44_100);
        assert_eq!(tp.frame_count, 8);
        assert_eq!(tp.samples, frames);

        cleanup(&path);
    }

    #[test]
    fn snapshot_emits_tib_rev_for_tracks_in_the_map() {
        use crate::project::{Project, Track, TrackSource};
        use crate::telemetry::TelemetryProfile;
        use std::collections::HashMap;
        let mut proj = Project::new("S", PathBuf::from("/tmp/whatever.tib"));
        proj.tracks.push(Track {
            id: "t-a".into(),
            name: "A".into(),
            file: String::new(),
            mute: false,
            gain_db: 0.0,
            sample_rate: 48_000,
            channel_source: None,
            duration_secs: 1.0,
            profile: None,
            stereo: true,
            source: TrackSource::default(),
            correction: None,
            gain_automation: None,
            polarity_inverted: false,
            telemetry: None,
            telemetry_profile: TelemetryProfile::default(),
        });
        proj.tracks.push(Track {
            id: "t-b".into(),
            name: "B".into(),
            file: String::new(),
            mute: false,
            gain_db: 0.0,
            sample_rate: 48_000,
            channel_source: None,
            duration_secs: 1.0,
            profile: None,
            stereo: true,
            source: TrackSource::default(),
            correction: None,
            gain_automation: None,
            polarity_inverted: false,
            telemetry: None,
            telemetry_profile: TelemetryProfile::default(),
        });

        // Folder path: no map → every source is `File`.
        let snap = snapshot_project(&proj, None);
        for ts in &snap.tracks {
            assert!(matches!(ts.source, AudioSource::File(_)));
        }

        // Tib path: only `t-a` has a rev pointer. `t-b` falls through to
        // `File` and would surface a per-track skip when loaded.
        let mut map = HashMap::new();
        map.insert("t-a".to_string(), 42i64);
        let snap = snapshot_project(&proj, Some(&map));
        match &snap.tracks[0].source {
            AudioSource::TibRev { db_path, rev_id } => {
                assert_eq!(db_path, &PathBuf::from("/tmp/whatever.tib"));
                assert_eq!(*rev_id, 42);
            }
            _ => panic!("t-a should resolve to TibRev"),
        }
        assert!(matches!(snap.tracks[1].source, AudioSource::File(_)));
    }

    #[test]
    fn missing_tib_rev_returns_error_not_panic() {
        // `TibDb::open` is built on `rusqlite::Connection::open`, which
        // creates the file if it doesn't exist — so we can't probe
        // load_track_play with a fictitious path without leaving a
        // stray empty .tib behind. Build a real empty .tib in temp_dir,
        // point at a non-existent rev id, and clean up after.
        let path = scratch_tib("missing-rev");
        {
            let _db = TibDb::create(&path).unwrap();
        }
        let snap = TrackAudioSnapshot {
            source: AudioSource::TibRev {
                db_path: path.clone(),
                rev_id: 99_999,
            },
            name: "Ghost".into(),
            file: "tib:99999".into(),
            gain_db: 0.0,
            mute: false,
            polarity_inverted: false,
            correction: None,
            gain_automation: None,
        };
        assert!(load_track_play(&snap).is_err());
        cleanup(&path);
    }
}
