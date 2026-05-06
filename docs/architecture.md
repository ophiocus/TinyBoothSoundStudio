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
├── main.rs           — eframe entrypoint, viewport icon load, mod declarations
├── app.rs            — TinyBoothApp central state + eframe::App impl + bulk-action methods + PendingTake
│
│   ── Audio path ──
├── audio.rs          — cpal input stream, SourceMode, recording session, viz ring buffer
│                       (now accepts `required_sample_rate` so the Record tab can pin the
│                        capture rate to the recordings project's existing rate)
├── player.rs         — cpal output stream, Player, PlayerState, TrackPlay, TrackBufCache
│                       (master-bus LUFS readouts published as atomics for the UI)
├── dsp.rs            — Profile, EqBand, FilterChain (mono), FilterChainStereo
│                       Built-in presets: Guitar / Vocals / Wind / Drums / Raw / Suno-Clean
│                       + 11-preset Suno-X library (Vocal / BackingVocal / Drums / Bass /
│                       ElectricGuitar / AcousticGuitar / Keys / Synth / Pads / Percussion /
│                       FxOther). DC-remove and Nyquist-clean are first-class chain stages.
│                       `role_to_preset_name` for auto-seeding at Suno import.
├── automation.rs     — AutomationLane, SplineSampler (Catmull-Rom via `splines` crate), Recorder
├── analysis.rs       — FFT spectrum (rustfft), waveform peak decimation
├── coherence.rs      — Suno-import coherence: sum-vs-mixdown residual + Pearson per-stem
│                       polarity check. f32-mono, 4 kHz decimation, memory-bounded.
├── lufs.rs           — BS.1770-4 K-weighting + integrated loudness with absolute / relative
│                       gating. `LufsMeter` for streaming the master bus; `integrated_lufs_i16`
│                       one-shot helper for "what's the LUFS of this WAV" at import.
├── trim.rs           — Project-level batch trim: crops every WAV to a shared `[start, end]`
│                       range, atomically (`.tmp` + rename). `reference_waveform` for the
│                       trim panel's thumbnail; mm:ss.mmm parse / format helpers.
│
│   ── Project + I/O ──
├── project.rs        — Project, Track, TrackSource, StemRole schema; load/save manifest
│                       `Project::open_or_create_recordings()` for the persistent recordings
│                       filespace at %APPDATA%\TinyBooth Sound Studio\recordings\.
├── suno_import.rs    — Folder + zip ingestion, ImportLog, ImportOutcome, conflict probe
│                       Auto-seeds per-role Suno-X presets onto detected stems; runs the
│                       coherence + polarity-vs-mixdown checks; computes mixdown LUFS.
├── suno_meta.rs      — RIFF/INFO/ICMT walker — Suno session epoch + provenance
├── export.rs         — Mixdown (correction-aware), WAV native, ffmpeg subprocess for lossy
│
│   ── App-level glue ──
├── config.rs         — Config (dark mode, zoom, last project, recent projects)
│                       `Config::recordings_root()` for the dedicated recordings filespace.
├── git_update.rs     — GitHub releases polling, MSI download, elevated msiexec
├── manual.rs         — Page list with include_str! of every docs/manual/*.md
│
│   ── UI ──
└── ui/
    ├── mod.rs              — module declarations only
    ├── record.rs           — Record tab: header (device picker, source mode, transport, viz)
    │                         + paged Recordings list with ▶ play-in-mixer / 🗑 delete actions
    ├── project.rs          — Project tab: track table with role-tagged source column,
    │                         "✂ Trim project…" button
    ├── mix.rs              — Mix tab: lanes + console deck + bulk-correction buttons + transport
    │                         (LUFS readout, polarity-flip Ø button per strip, autoplay hand-off
    │                         from Record-tab ▶ buttons)
    ├── export.rs           — Export tab: format picker, bitrate, output dialog
    ├── admin.rs            — Floating profile editor (Recording-tone → DSP)
    ├── correction.rs       — Floating per-track correction editor (Mix tab → button)
    ├── profile_editor.rs   — Shared body for the Admin + Correction windows (Suno-cleanup
    │                         section with DC-remove + Nyquist-clean toggles)
    ├── trim.rs             — Project-trim panel (waveform thumbnail + mm:ss.mmm entries)
    ├── manual.rs           — Floating Help → Manual window (TOC + markdown body)
    ├── import_dialog.rs    — Modal: import-result (success or fail + coherence summary + log)
    ├── import_conflict.rs  — Modal: duplicate-import resolution (Replace / Cancel)
    └── viz.rs              — Shared waveform / spectrum / peak-meter primitives
```

Line counts intentionally omitted — they drift on every commit and are not load-bearing for understanding the architecture. The shape (which modules exist, what each is responsible for) is what matters.

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
audio.rs::start_recording(required_sample_rate)
  ↓  freezes profile into FilterChain or FilterChainStereo
Audio thread (cpal callback)
  ↓  per-frame: pick channel(s) → run chain → write WAV
hound::WavWriter::write_sample(i16)   ← mono or stereo
  +
viz.push_mono / push_stereo(f32)      ← UI thread reads each frame
                                         for live waveform + FFT
```

Key constraint: the cpal callback runs on a high-priority audio thread. **No allocations.** Locks are taken only for the WAV writer (per-frame, but it's a `parking_lot::Mutex<Option<WavWriter>>` — tiny lock window). Filter chain state is owned by the closure and never crosses thread boundaries.

**The recording lands in a dedicated app-owned filespace, never the active stem-mixing project.** `start_new_take` loads the persistent recordings project from `%APPDATA%\TinyBooth Sound Studio\recordings\` (creating it on first run), mints a unique `track-NNN` id, computes the recording path under that root, starts cpal. The `required_sample_rate` argument keys on the recordings project's existing first-track rate so every take in that filespace shares one rate (the player has no resampler yet, so mixed-rate projects break the Mix tab — better to refuse up-front than land a broken WAV). Take metadata lives on `app.pending_take: Option<PendingTake>` for the duration of the recording; `stop_take` re-loads the recordings project from disk, appends the finished `Track` row, saves. Two disk loads per take (start: mint id + read rate constraint; stop: append + save) — keeps the manifest as the single source of truth, no in-memory dual-project state.

The user's `app.project` (Suno import or anything else) is never touched by the Record tab. To review or mix recordings, **File → Open Recordings** swaps `app.project` to the recordings project; alternatively, the Record-tab "Recordings" list has a ▶ button per entry that does the same swap + Mix-tab switch + solo + autoplay in one click.

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

Two workflows, both pinned to the same Rust toolchain (`1.95.0` at time of writing) with `components: rustfmt, clippy` declared explicitly.

**`.github/workflows/release.yml`** — tag-push → MSI → GitHub Release:

1. **Trigger**: push of any tag matching `v*`.
2. **Sanity check**: refuse if Cargo.toml's `version` doesn't match the tag (defensive — added after a CNDL0288 collision in v0.1.1).
3. **Quality gates** (added v0.3.6): `cargo fmt --check`, `cargo clippy --release --all-targets -- -D warnings`, `cargo test --release`. Same three commands as `ci.yml`, run inline before the build.
4. **Build**: `cargo build --release` on `windows-latest`.
5. **WiX 3.11 portable** downloaded fresh each run (no runner-local install dependency).
6. **MSI**: `cargo wix --nocapture` (no `-C dVersion`; cargo-wix derives from Cargo.toml).
7. **Artifact upload**: MSI + bare exe.
8. **Release job** on `ubuntu-latest`: downloads artefacts, creates a GitHub Release with auto-generated notes.

Tag → MSI → Release usually takes ~9 min end-to-end.

**`.github/workflows/ci.yml`** — PR + push-to-main → gates only (added v0.3.10):

Runs the same three gates as release.yml on every PR to `main` and every push to `main`. Catches lint / test / format regressions at edit time rather than at tag-push, after twice in this project's history (v0.3.6→.7, v0.3.8→.9) the ship-time gate burned a version number on a problem a PR-time gate would have caught. Concurrency-grouped per ref so rapid-fire pushes cancel in-flight runs. Skips on doc/asset-only diffs via `paths-ignore`.

#### 6.2.1 The cost of running gates in two places

Splitting gates across `release.yml` and `ci.yml` introduces a deliberate, bounded sync tax. Three things must stay aligned across both files or the second gate's whole point is defeated:

1. **Toolchain version**. Both pin `dtolnay/rust-toolchain@<X.Y.Z>` to the same `<X.Y.Z>`. If they diverge, CI passes on toolchain A while the ship gate runs on B — the original drift problem we wanted to eliminate.
2. **Toolchain components**. Both declare `components: rustfmt, clippy`. A versioned-tag pin without this is the regression that burned v0.3.8.
3. **Gate command list**. The three gate commands are spelled out identically in both files. Adding a fourth gate (e.g. `cargo doc --no-deps`) means editing both.

There is **no reusable-workflow indirection** on purpose. A `workflow_call` shared definition would compress the gate list into one place but adds:
- An extra runner spin-up on every release (~1–2 min) since the gate workflow and the build workflow can no longer share toolchain install + cache;
- A new "two callers + one callee" topology that still needs maintenance discipline (the toolchain version becomes a `with:` input that has to be passed correctly from each caller).

The discipline cost is roughly the same either way; the runtime cost is not. So the project keeps the duplication and makes drift visible at edit time via cross-referenced `KEEP IN SYNC WITH …` comments at the top of `ci.yml` and on the toolchain step of `release.yml`. Reconsider the trade-off if the gate count grows past five or six commands.

A second-order overhead worth naming:

- **Runner cost**. `windows-latest` is 2× the per-minute cost of `ubuntu-latest`. CI gates would compile and test fine on Linux (only `build.rs`'s `winres` block is Windows-gated); we stay on Windows anyway because gate-on-Linux/ship-on-Windows reintroduces the cross-platform drift class we're trying to eliminate. For a solo project at this PR volume the cost is rounding error.
- **False positives blocking PRs**. If a future toolchain bump introduces a noisy lint, CI blocks until the lint is fixed or the toolchain pin is rolled back. That's a feature, not overhead — it's exactly why the gate exists — but it does mean toolchain bumps are non-trivial commits.

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
