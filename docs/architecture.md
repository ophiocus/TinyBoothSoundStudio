# TinyBooth Sound Studio — Architecture

**As of v0.3.5.** Living document — refresh whenever the public surface shifts. The detailed code-walk audit is at [`docs/audit/2026-04-27-codebase-audit.md`](audit/2026-04-27-codebase-audit.md).

## 1. What this app is

TinyBooth is a single-binary Windows desktop app that does three things end-to-end:

1. **Records** audio from a chosen input device, optionally in stereo, applying a real-time DSP "recording-tone" preset before the WAV writer touches disk.
2. **Mixes** a project's tracks (recorded takes or imported Suno stems) live through per-track correction chains and master automation, with a hardware-style fader console for ride-the-mix listening.
3. **Exports** the result to WAV (native) or FLAC / MP3 / Ogg / Opus / M4A (via `ffmpeg` subprocess).

It also acts as the receiving end of Suno's stem-export — drop a "Download All" zip in and you get a TinyBooth project ready to mix and clean.

The whole stack is ~6,200 lines of Rust across 25 files, ships as a 12 MB exe + 6 MB MSI installer, builds in ~80 s release on a mid-tier laptop, runs at <50 MB resident with no project loaded.

## 2. Top-level module map

```
src/
├── main.rs            58 lines  — eframe entrypoint, viewport icon load, mod declarations
├── app.rs            727 lines  — TinyBoothApp central state + eframe::App impl + bulk-action methods
│
│   ── Audio path ──
├── audio.rs          388 lines  — cpal input stream, SourceMode, recording session, viz ring buffer
├── player.rs         598 lines  — cpal output stream, Player, PlayerState, TrackPlay, TrackBufCache
├── dsp.rs            639 lines  — Profile, EqBand, FilterChain (mono), FilterChainStereo, builtin presets
├── automation.rs     139 lines  — AutomationLane, SplineSampler (Catmull-Rom via `splines` crate), Recorder
├── analysis.rs        69 lines  — FFT spectrum (rustfft), waveform peak decimation
│
│   ── Project + I/O ──
├── project.rs        245 lines  — Project, Track, TrackSource, StemRole schema; load/save manifest
├── suno_import.rs    754 lines  — Folder + zip ingestion, ImportLog, ImportOutcome, conflict probe
├── suno_meta.rs      128 lines  — RIFF/INFO/ICMT walker — Suno session epoch + provenance
├── export.rs         322 lines  — Mixdown (correction-aware), WAV native, ffmpeg subprocess for lossy
│
│   ── App-level glue ──
├── config.rs          86 lines  — Config (dark mode, zoom, last project, recent projects)
├── git_update.rs     156 lines  — GitHub releases polling, MSI download, elevated msiexec
├── manual.rs         125 lines  — Page list with include_str! of every docs/manual/*.md
│
│   ── UI ──
└── ui/
    ├── mod.rs         10 lines  — module declarations only
    ├── record.rs     193 lines  — Record tab: device picker, source mode, transport, viz
    ├── project.rs    121 lines  — Project tab: track table with role-tagged source column
    ├── mix.rs        610 lines  — Mix tab: lanes + console deck + bulk-correction buttons + transport
    ├── export.rs     115 lines  — Export tab: format picker, bitrate, output dialog
    ├── admin.rs      227 lines  — Floating profile editor (Recording-tone → DSP)
    ├── correction.rs 194 lines  — Floating per-track correction editor (Mix tab → button)
    ├── manual.rs      57 lines  — Floating Help → Manual window (TOC + markdown body)
    ├── import_dialog.rs   79 lines  — Modal: import-result (success or fail + log links)
    ├── import_conflict.rs 98 lines  — Modal: duplicate-import resolution (Replace / Cancel)
    └── viz.rs         90 lines  — Shared waveform / spectrum / peak-meter primitives
```

External-facing pieces that aren't `.rs` source:

- `Cargo.toml` — package metadata + cargo-wix metadata for MSI.
- `build.rs` — derives `APP_VERSION` from the latest git tag; embeds icon + version resource on Windows via `winres`.
- `wix/main.wxs` — WiX installer template (parameterised, fresh GUIDs per app via the Skeleton bootstrap).
- `assets/` — `icon.ico` (multi-size), `icon.png` (source), `icon_viewport.png` (256×256 embedded), `banner.jpg` (README).
- `docs/manual/` — Markdown chapters embedded into the binary via `include_str!`; same files render on github.com.
- `docs/feature-requests/` — formal RFCs (TBSS-FR-NNNN).
- `tools/compare.py` — external Python comparator for export-quality verification.
- `.github/workflows/release.yml` — tag-push triggers MSI build + GitHub Release.

## 3. Three principal flows

### 3.1 Recording

```
Mic / Interface
  ↓  cpal::Device → Stream
audio.rs::start_recording()
  ↓  freezes profile into FilterChain or FilterChainStereo
Audio thread (cpal callback)
  ↓  per-frame: pick channel(s) → run chain → write WAV
hound::WavWriter::write_sample(i16)   ← mono or stereo
  +
viz.push_mono / push_stereo(f32)      ← UI thread reads each frame
                                         for live waveform + FFT
```

Key constraint: the cpal callback runs on a high-priority audio thread. **No allocations.** Locks are taken only for the WAV writer (per-frame, but it's a `parking_lot::Mutex<Option<WavWriter>>` — tiny lock window). Filter chain state is owned by the closure and never crosses thread boundaries.

### 3.2 Multitrack playback (Mix tab)

```
project.tinybooth + tracks/*.wav
  ↓  Player::new at first Mix-tab visit
Pre-load every WAV into Vec<i16>     ← memory-resident, ~140 MB for 12 stems × 3 min
  ↓
cpal default output Stream
  ↓
Audio callback per-buffer:
  ┌─ refresh chain rebuild generation (rare lock)
  ├─ fill TrackBufCache: skip / bypass / armed / static_gain_lin (5 atomics × N tracks)
  ├─ per frame:
  │    ├─ skip muted / non-soloed
  │    ├─ read i16 → f32
  │    ├─ chains[i].process(L,R) if !bypass && has_chain
  │    ├─ effective gain: spline.sample(t) if has_automation, else cached static_gain_lin
  │    ├─ accumulate into stereo bus
  │    └─ track_peaks[i] = max(peak, post_l|r)
  ├─ master gain + master automation
  ├─ soft-limit, write to cpal buffer
  ├─ publish per-track + master peaks to UI atomics
  └─ advance position_frames atomic
```

Per-buffer cache (added v0.3.4 Phase B) drops per-callback atomic loads from ~11.5k to ~50 for a typical 9-stem project at 256-frame buffers.

UI thread reads `position_frames` once per frame for the playhead; reads peaks for meters; reads `play_state` for transport widget state.

### 3.3 Export

```
project.tinybooth (active project)
  ↓  user picks format + path on Export tab
export.rs::export()
  ↓
mixdown(project, active_tracks):
  for each track:
    read WAV (hound) → f32 with track.gain_db pre-applied
    if track.correction is Some:
      run through FilterChainStereo (centre-pan mono inputs)
    if has gain_automation:
      per-frame spline.sample → effective gain = ratio × baseline-applied-gain
    write into mix buffer (stereo if any track is stereo, else mono)
  apply Project.master_gain_db + master_gain_automation per frame
  soft-limit to [-1, 1]
  ↓
WAV: hound::WavWriter writes interleaved i16
non-WAV: write temp WAV → spawn ffmpeg subprocess with codec args → wait
```

Export reproduces Mix-tab playback within rounding — same chain code, same gain logic. Soft-limit at the end matches the live path's per-sample clamp.

## 4. State management

### 4.1 The single owner

Every piece of mutable app state lives on one struct:

```rust
// src/app.rs
pub struct TinyBoothApp {
    pub config: Config,                            // %APPDATA% persistence
    pub project: Project,                          // active project
    pub project_dirty: bool,
    pub devices: Vec<DeviceInfo>,                  // recording inputs
    pub selected_device: Option<String>,
    pub selected_mode: SourceMode,
    pub viz: Arc<VizState>,                        // shared with audio thread
    pub session: Option<RecordingSession>,         // when recording
    pub player: Option<Player>,                    // when Mix tab opened
    pub recorder: Recorder,                        // automation scratch lanes
    pub profiles: Vec<Profile>,                    // recording-tone presets
    pub active_profile_idx: usize,
    pub tab: Tab,                                  // Record / Project / Mix / Export
    pub status: Option<String>,                    // bottom-bar status line
    // ... modal states, update-checker, manual viewer, etc.
}
```

eframe's `App::update(&mut self, ctx, frame)` runs ~30–60 fps on the UI thread. Every UI submodule receives `&mut TinyBoothApp` and mutates it directly. There is no separate "model" / "store" layer.

### 4.2 Audio-thread-shared state

Two patterns for crossing the thread boundary:

**`Arc<VizState>` for recording**
```rust
pub struct VizState {
    pub left:   parking_lot::Mutex<VecDeque<f32>>,  // ring buffer
    pub right:  parking_lot::Mutex<VecDeque<f32>>,
    pub stereo: AtomicBool,
    peak_l_x1000: AtomicU32,
    peak_r_x1000: AtomicU32,
    pub sample_rate: AtomicU32,
}
```
The audio thread pushes samples; the UI thread snapshots ranges. Tiny ring buffer (~4 s at 48 kHz mono).

**`Arc<PlayerState>` for playback**
```rust
pub struct PlayerState {
    pub play_state: AtomicU8,                       // PlayState repr-u8
    pub position_frames: AtomicU64,
    pub global_bypass: AtomicBool,
    pub master_gain_db_bits: AtomicU32,             // f32 bits
    pub master_recording_armed: AtomicBool,
    pub master_peak_l_x1000: AtomicU32,
    pub master_peak_r_x1000: AtomicU32,
    pub master_automation_lane: parking_lot::Mutex<Option<AutomationLane>>,
    pub master_automation_generation: AtomicU64,    // bumped on UI mutation
    pub tracks: Vec<Arc<TrackPlay>>,                // each carries its own atomics
}
```

Generation-counter pattern: when the UI thread mutates a Mutex-protected resource, it bumps an `AtomicU64` generation. The audio thread compares its last-seen generation per callback; only takes the lock when they differ.

### 4.3 The borrow-checker pattern in egui

eframe gives the UI submodules `&mut TinyBoothApp` AND `&mut egui::Ui`. Inside an egui closure (`ui.horizontal(|ui| { … })`) the closure captures parts of `app` immutably (e.g. `app.player.as_ref()`) — the borrow lives until the closure returns. Calling a mutating method on `app` inside such a closure fails to compile.

**Idiom used everywhere**:
```rust
let mut click_play = false;
let mut click_save = false;
ui.horizontal(|ui| {
    if ui.button("▶ Play").clicked()  { click_play = true; }
    if ui.button("Save").clicked()    { click_save = true; }
});
// Closure unborrows app here.
if click_play { app.player.as_ref().unwrap().play(); }
if click_save { app.save_project(); }
```

This is consistent across `ui/mix.rs`, `ui/correction.rs`, `ui/import_dialog.rs`, etc.

## 5. Schema versioning

The project file (`project.tinybooth`) and profile file (`profiles.json`) both follow the same rule: **every field added after v0.1 is `#[serde(default)]`**. Older manifests load identically; newer fields fall to default values until the user touches them.

| Schema | Field | Added | Default |
|---|---|---|---|
| `Track.stereo` | bool | v0.1.1 | `false` |
| `Track.profile` | Option<Profile> | v0.1.6 | `None` |
| `Track.source` | tagged enum | v0.1.4 | `Recorded` |
| `Track.correction` | Option<Profile> | v0.2.0 | `None` |
| `Track.gain_automation` | Option<AutomationLane> | v0.3.0 | `None` |
| `Project.master_gain_db` | f32 | v0.3.0 | `0.0` |
| `Project.master_gain_automation` | Option<AutomationLane> | v0.3.0 | `None` |
| `Project.next_suno_ordinal` | u32 | v0.3.1 | `1` |
| `TrackSource::SunoStem.{session_epoch, session_ordinal, provenance}` | Option<i64/u32/String> | v0.3.1 | `None` |
| `Project.corrections_disabled` | bool | v0.3.4 | `false` |
| `Project.default_correction` | Option<Profile> | v0.3.4 | `None` |
| `Profile.eq_bands` | [EqBand; 4] | v0.1.6 | 4× `Bypass` |
| `Profile.deess_*` | bool/f32 | v0.1.6 | disabled |

A v0.1.0 project from the very first release loads fine on v0.3.5 today.

## 6. Build & release pipeline

### 6.1 Local build

```
cargo build --release           # 12 MB exe, ~80 s on a laptop
cargo wix --bin-path WIX_PATH   # 6 MB MSI
```

`build.rs` runs `git describe --tags --match v* --abbrev=0` to derive `APP_VERSION` for embedding into the exe (Windows version resource via `winres`) and into the in-app `git_update.rs` for self-update comparisons. Falls back to `Cargo.toml` if not in a git checkout.

### 6.2 CI (GitHub Actions)

A single workflow at `.github/workflows/release.yml`:

1. **Trigger**: push of any tag matching `v*`.
2. **Sanity check**: refuse if Cargo.toml's `version` doesn't match the tag (defensive — added after a CNDL0288 collision in v0.1.1).
3. **Build**: `cargo build --release` on `windows-latest`.
4. **WiX 3.11 portable** downloaded fresh each run (no runner-local install dependency).
5. **MSI**: `cargo wix --nocapture` (no `-C dVersion`; cargo-wix derives from Cargo.toml).
6. **Artifact upload**: MSI + bare exe.
7. **Release job** on `ubuntu-latest`: downloads artefacts, creates a GitHub Release with auto-generated notes.

Tag → MSI → Release usually takes ~9 min end-to-end.

### 6.3 Self-update

`src/git_update.rs` is the in-app updater:

1. On startup, a background thread calls `https://api.github.com/repos/ophiocus/TinyBoothSoundStudio/releases/latest`.
2. If the latest tag is greater than `APP_VERSION` (4-part semver compare, missing parts default to 0), the bottom-bar version label becomes a clickable button.
3. Clicking downloads the `.msi` from the release's assets to temp.
4. Launches `msiexec /i tmp.msi /passive /norestart` via `Start-Process … -Verb RunAs` so Windows UAC prompts for elevation.
5. After successful spawn, TinyBooth exits — Windows takes over the upgrade.

No version check on every action — just at startup and on user click. No telemetry, no analytics.

## 7. Logging & diagnostics

User-data lives at `%APPDATA%\TinyBooth Sound Studio\`:

```
config.json                        Config (dark mode, last project, recent projects, …)
profiles.json                      Recording-tone presets
sessions/<auto-name>/              Default scratch project root if user doesn't pick a folder
logs/import-{mode}-{name}-{ts}.log Per-import diagnostic log (every entry's KEEP/SKIP decision)
```

Import logs are the primary debugging surface: every Suno bundle ingestion writes a fresh log file with raw entry names, classifications, and a final summary. The import-result modal links to it.

Crashes and unexpected errors go to stderr; running TinyBooth from a terminal surfaces them.

## 8. Distribution model

- **Binary**: 12 MB Windows-x86_64 PE with embedded icon, version resource, and the entire docs/manual.
- **MSI**: 6 MB. Installs to `Program Files\tinyboothsoundstudio\bin\`, creates a Desktop shortcut, optionally adds `bin\` to PATH (advertised feature). Major-upgrade-aware: a newer version's MSI cleanly replaces the old one.
- **No runtime dependencies** for the core app. ffmpeg is *optional* — only required for non-WAV export — and discovered at runtime from three search paths (next to exe, `./ffmpeg/bin/`, system PATH).
- **Single distribution channel**: GitHub Releases. The in-app updater queries the same endpoint; no parallel download infrastructure.

## 9. What's not in here

Honest scope-limits documented for future readers:

- **No plugins.** No VST/CLAP hosting, no scripting, no extension API. Every feature lives in the binary.
- **No telemetry.** No crash reporting, no usage analytics, no auto-call-home. The version-check API call is the only outbound network hit, and it's a public unauthenticated GitHub endpoint.
- **No cloud.** No accounts, no sync, no collaborative editing. Projects are folders on disk.
- **No undo.** Mutations are direct. Reset and Replace are destructive without a "are you sure?" preceding modal beyond the one already on Suno-import conflicts.
- **No multi-device recording or WASAPI exclusive-mode.** Single cpal input stream, default WASAPI shared mode. Aggregator devices (Voicemeeter, VB-CABLE) are the recommended route for users who need more.
- **No resampling.** All tracks in a project must share a sample rate. Player and exporter both error out clearly on mismatch.
- **No automation beyond gain.** No EQ-band sweeps, comp-threshold rides, etc. Just per-track and master fader.

These boundaries keep the binary small, the audio thread fast, and the codebase comprehensible. Every "no" is a deliberate choice; some have RFCs proposing future "yes" but none has shipped.
