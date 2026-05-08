# Changelog

All notable changes to TinyBooth Sound Studio.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); this project tracks [Semantic Versioning](https://semver.org/) loosely (the v0.x series treats minor bumps as feature releases, patch bumps as fixes / polish).

## [Unreleased]

### Known issue — in-app updater / CI sync window

If the user has the app open at the moment a new release is published, the bottom-bar version label keeps showing the current install indefinitely — `git_update::check_latest_release` only fires **once at app startup**, not periodically. Workaround today: click the version label in the bottom-left to retrigger the check, or restart the app. Manifested concretely on the v0.4.12 release: CI completed at 16:06:50Z and the GitHub release went live, but a session opened before that timestamp didn't see the new version until the user clicked the label.

Proposed fix (queued for a follow-up patch): re-run the background check on a 5-minute timer while the app is idle, AND on every tab change. Both are cheap, both are bounded, and either one closes the window. ~30 LOC in `src/git_update.rs` plus a `last_check_at: Option<Instant>` field. No new deps.

## [0.4.12] — 2026-05-08

### Added
- **New visualizer mode: "Onion Skin" (multi-timescale trajectory)** — addresses the long-standing critique that audio visualizers are derivative-of-NOW and never show the volumes / cadences / colors a listener navigates while listening. Plots `(spectral_centroid, RMS)` as motion through 2D feature space with three layers of temporal memory: bright recent trail (note / beat scale), faded ghost trail (phrase scale, ~30 s), and a session-wide residency watermark heatmap (section / song scale). Optional anticipated-future projection extends the trajectory linearly via recent-direction averaging. The first mode designed against the "memoryless visualisation is sterile" critique articulated in `docs/sound-vision-philosophy.md`. Axis labels (soft↔loud, dark↔bright) so the listener orients at a glance.
- **Collapsible left-side config panel** in the visualizer screen exposing every per-mode parameter as a slider / checkbox. Every control has a `.on_hover_text(...)` helper explaining what it does, what good values look like, and where defaults came from. Toggle visibility via the new "◀ Hide config" / "▶ Show config" button next to the heading.
- **Temporal smoothing** for the modes that benefit:
  - **Mandala** — exponential moving average on the spectrum (default α=0.6, slider 0..0.95). Reduces jitter without losing responsiveness; reveals the steady-state structure underneath the transient flicker.
  - **Onion Skin** — EMA on the (centroid, RMS) point before plotting (default 0.5). Trades note-level reactivity for trajectory readability.
- **Per-mode parameter structs** (`LissajousParams`, `MandalaParams`, `LorenzParams`, `ChladniParams`, `OnionSkinParams`) on `VisualizerParams`. Defaults reproduce v0.4.11 behaviour exactly; existing users see the same modes unless they tweak the new sliders.
- New top-bar "Hide config" / "Show config" toggle and per-mode hover descriptions on the mode buttons.

### Documentation
- New essay: **[`docs/sound-vision-philosophy.md`](docs/sound-vision-philosophy.md)** — long-form engagement with the question of what it means to transform sound into vision. Argues most audio viz is sterile because it operates only at the sample / note timescale while listeners parse music at five hierarchical timescales simultaneously. Maps "volumes / cadences / colors" onto those timescales. Develops the "onion skin" insight (each visualised moment contextualised by its neighbors across multiple timescales). Includes a substantial DSP detour: the v0.4.11 Mandala's visible jerkiness on AI-generated audio is a *real diagnostic signal* — AI audio has band-decorrelated micro-fluctuations where natural recordings have correlated ones. Sketches a "Coherence Restoration" filter as a v0.5+ feature that would smooth this signature in the modulation domain, taking AI output meaningfully closer to "sounds like a recording". Linked from the README's contributor docs.

### Changed
- README's contributor-docs section now links the new philosophy essay alongside `design-vibes.md`.

## [0.4.11] — 2026-05-08

### Added
- **🌀 Audio-reactive visualizer** — toggleable full-window canvas accessible from the menu bar. Click the icon to take over the central panel; click again to return. Four mathematically-grounded modes, all on egui's 2D painter (no GPU shaders, no extra deps):
  - **Lissajous goniometer** — XY plot of master-bus L vs R with phosphor-trail alpha gradient. Reveals stereo image geometry at a glance: mono content draws a vertical line, anti-phase draws a horizontal 45° line, full stereo draws organic figure-8s. Crosshair guides for the canonical phase angles.
  - **Spectral mandala** — radial FFT, frequencies arranged around the centre with magnitude as petal length. Mirrored across the X axis for mandala symmetry. Hue tracks frequency: warm reds at the bass end, cool cyans at the treble. Tonal balance becomes literally glanceable.
  - **Lorenz attractor (audio-modulated)** — RK4 integration of the Lorenz ODE with σ / ρ / β tugged in real time by spectral centroid and RMS. The strange attractor breathes with the music; auto-fitting projection keeps the orbit centred regardless of parameter drift. Trail of 2000 points coloured by recency through a hue gradient.
  - **Chladni cymatics** — superposition of ten of Ernst Chladni's classic eigenmodes (sin·sin combinations on the unit square) weighted by FFT band energies. Renders the actual mathematical eigenmodes Chladni discovered in 1787. Slow phase drift keeps the figure animated even on steady-state input.
- **Master-bus sample tap** on `PlayerState.output_viz` — `parking_lot::Mutex<VecDeque<(f32, f32)>>` of length `OUTPUT_VIZ_LEN` (4096 stereo frames, ~85 ms at 48 kHz). Audio thread pushes post-fader L/R samples on every callback; UI thread snapshots when rendering the visualizer. Brief lock window keeps the audio callback within budget.
- 4 unit tests on the visualizer's pure helpers: RK4 Lorenz integrator stays bounded over 10k steps on default chaotic parameters; HSV → RGB conversion handles primary colours and zero-value black correctly.

### Research notes
The mode selection synthesises a brief literature scan covering: cymatics / Chladni patterns ([CymaVis](https://cymavis.com/), [Cymatica](https://www.cymatica.app/)), audio-reactive Lorenz visualizers ([3D Music Visualizer](https://github.com/hederhayat/Lorenz-3D-Music-Visualizer), Cherry Audio's Lorenz module), radial-FFT analyzers ([audioMotion](https://github.com/hvianna/audioMotion-analyzer), WaveForge), and phase-space portraits in audio research ([Audio Visualization in Phase Space](https://www.semanticscholar.org/paper/Audio-Visualization-in-Phase-Space-Gerhard/df1b84bc0c759708de2fe657df777d38027b950b), Royal Society's [phasegram paper](https://royalsocietypublishing.org/rsif/article/10/85/20130288)). Reaction-diffusion (Gray-Scott) was considered and skipped — needs GPU shaders to run real-time at fullscreen, scope creep for v0.4.x.

## [0.4.10] — 2026-04-28

### Added
- **Bundled static-LGPL ffmpeg.** TinyBooth's MSI now ships a `ffmpeg.exe` next to the main binary, sourced from [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds)'s nightly LGPL build. FLAC / MP3 / Ogg Vorbis / Ogg Opus / M4A-AAC export Just Works on a fresh install — no separate download, no PATH plumbing, no scavenging binaries off the internet. Trade-off: install size jumps from ~10 MB to ~130 MB. CI workflow downloads + extracts `ffmpeg.exe` to `target/release/` before `cargo wix` packages the MSI; new `binary_ffmpeg` `<Component>` in `wix/main.wxs` references it. License attribution lives in the README's "Built with" line and the Export-tab manual chapter — TinyBooth uses ffmpeg as a separate subprocess (the LGPL-compliant integration mode for non-free apps).
- **Update-download dialog with rotating fortune-cookie tips.** The bigger MSI means a longer self-update download; the existing tiny `"downloading…"` label in the bottom bar got old fast. New `src/ui/update_dialog.rs` shows a centred modal overlay during `UpdateState::Downloading(_)` with a spinner, a one-line note explaining why the download is heftier, and a rotating tip card cycling through 22 workflow facts every 6 seconds (recordings filespace, polarity flip, LUFS targets, F1, per-role presets, the cleanse, trim, automation arm, A/B, coherence, Suno-X chains, recordings list ▶, DC remove, polarity-as-debug-tool, etc.). Hooked from `app::update()` after the bottom-bar render — no-op when `update_state` isn't `Downloading`.

### Documentation
- README's "What it does" Export bullet, manual chapter `01-getting-started.md`, manual chapter `06-export.md`, and `appendix-a-troubleshooting.md` rewritten to reflect the bundled `ffmpeg.exe`. The manual now distinguishes MSI-installed copies (ffmpeg is there, do nothing) from source builds (legacy fallback paths still apply).
- README's "Built with" line gains the FFmpeg attribution + LGPL pointer.

## [0.4.9] — 2026-04-28

### Fixed
- **Missing audio output device froze the Mix tab and spun fans.** When `default_output_device()` returned `None` (no headphones, sound card disabled, etc.), the failure happened at the *end* of `Player::new` — but `Player::new` had already loaded every track WAV into memory by then (~600 MB of `i16` sample arrays for a typical 9-stem Suno project). On Err, those allocations got dropped. The Mix-tab lazy-rebuild then re-called `Player::new` on the next frame because `app.player.is_none()`. Result: 600 MB of WAV decode + allocation per frame, allocator pegged, UI frozen, fans on full. Two fixes:
  - **`Player::new` now probes the output device first**, before any WAV loading. Fast-fail on no device — bails in microseconds with a clear message ("connect headphones or speakers (or check Windows sound settings) and click Retry above") instead of allocating half a gig only to throw it away.
  - **Per-frame retry storm killed by failure cache.** New `app.player_attempt_failed_for: Option<PathBuf>` records the project root that the last `Player::new` attempt failed on. The Mix-tab rebuild guard checks against the current project root and short-circuits when they match. Auto-invalidates on project change (root path comparison). Manually invalidated by a new `↻ Retry` button rendered next to the error banner — the natural recovery path when the user plugs in headphones.

### Changed
- The Mix-tab error banner gains a `↻ Retry` button when there's a failed-rebuild cache. Click rebuilds the player; the natural path back to a working Mix after fixing audio hardware externally.

## [0.4.8] — 2026-04-28

### Fixed
- **Cleanse hoisted from Mix-tab to top of `app::update()`.** Previously the cleanse only ran inside `mix.rs::show()`, so a user who landed on the Project tab first saw their orphans untouched until they specifically clicked Mix. The cleanse is now a once-per-frame call at the top of `update()` regardless of active tab — orphans clear out the moment a project is open. Cheap-path cost is one `iter().any()` over `project.tracks` (microseconds with the v0.4.7 perf fix already in place); no observable cost on idle Project / Export / Record tabs.
- **Missing-source orphans are now dropped from the manifest cleanly.** When a `Recorded` orphan's WAV file no longer exists on disk (user moved it via Explorer, manual delete, etc.), the cleanse used to: try `rename` → fail ENOENT, try `copy` → fail ENOENT, push a "could not move" failure into the report, and **restore the orphan into `project.tracks`**. Result: a forever-failing cleanse, status-bar full of red errors, manifest stuck pointing at a ghost. v0.4.8 detects missing source upfront via `src_abs.exists()`; the orphan gets dropped from the manifest with no migration attempt and the count goes into a new `removed_missing_count` field on `CleanseReport`. Status surfaces as `"Cleanse: removed 1 dangling manifest entry/entries (source WAV missing)"`. Clean, terminal, no retry loop.

## [0.4.7] — 2026-04-28

### Fixed
- **Mix-tab CPU / fan-spin**. Three per-frame allocation hot paths killed perf on the Mix tab — measurable as fans spinning up after a few seconds on the tab:
  - `lanes_view` called `track.correction().is_some()` and `track.automation().as_ref()` once per track per frame. Both methods take a `parking_lot::Mutex` lock and **clone the entire contents** — `Profile` (Strings, 4-band EQ array, de-ess fields) and `AutomationLane` (`Vec<AutomationPoint>`). With 12 tracks at 30 fps that's 720 Profile clones + 720 AutomationLane clones per second, all heap allocation. `TrackPlay` now exposes `has_correction(&self) -> bool` (atomic-bool mirror, no lock) and `with_automation<R>(&self, f: impl FnOnce(Option<&AutomationLane>) -> R) -> R` (callback-style borrow, no clone). Lanes view switched to both. `Profile`/`AutomationLane` cloning is now zero per frame on the Mix tab's idle path.
  - `cleanup::cleanse_recordings_in_suno_project` ran on every Mix-tab frame and unconditionally `drain()`-ed + rebuilt `project.tracks` even when no orphans were present — pointless heap shuffling on the common-case path. New cheap pre-check (`tracks.iter().any(|t| matches!(t.source, Recorded))`) returns the empty report before any mutation when there's nothing to do.
- The `correction()` / `automation()` methods are kept on `TrackPlay` as `#[allow(dead_code)]` for non-hot-path callers (project-save sync, future diff logic) — clone-via-lock is the right shape for those, just not for per-frame UI peeks.

## [0.4.6] — 2026-04-28

### Changed
- **MSI installer relaunches the app on successful install.** The in-app self-updater spawns `msiexec /passive` and exits so the install can replace the running .exe — but the MSI then ended silently, leaving the user staring at an empty desktop with their session gone. v0.4.6 adds a Type-18 custom action keyed off the installed exe that runs at the end of `InstallFinalize`, so the new version comes up automatically and the user lands back where they were. Gated on `UILevel >= 3 AND NOT Installed` — fires on `/passive` (the self-updater path), `/qr`, `/qf`, and standard double-click installs; skips `/qn` silent corporate deploys, repairs, uninstalls, and modify-installs. Runs under user-context impersonation so the app comes up at the user's integrity level, not elevated. `Return="asyncNoWait"` so msiexec doesn't sit blocked waiting for the app to close.

## [0.4.5] — 2026-04-28

### Changed
- **`Player::new`'s per-track conformance check now covers BOTH rate AND length.** Previously a track was skipped only on rate mismatch (and on file-load failure). Suno stems are co-rendered, so they share a single rate *and* a single length — a stem whose length differs from the rest by more than 100 ms is by definition an alien (orphan recording, different-generation take, etc.) and gets the same skip-and-warn treatment as a rate mismatch. The first successful track sets the project's reference rate + reference length; subsequent tracks must match within tolerance. Status-bar warning surfaces both reasons when both fail: `"skipped track 'X': rate Y Hz vs project Z Hz; length F1s vs project F2s"`. Tolerance was chosen to absorb codec-level packet-alignment jitter that legitimate Suno output may exhibit (sub-millisecond) without letting through actual orphans (typically seconds different).

## [0.4.4] — 2026-04-28

### Fixed
- **`Player::new` is now tolerant of per-track failures.** Previously, one missing or unreadable WAV (or a single rate-mismatched row that the cleanse couldn't reach) aborted the whole player and the Mix tab dead-ended on a red error banner. Now each load failure is sent through the audio-error channel as a "skipped track 'X' (file): <reason>" warning that the status bar surfaces, and the player builds from whatever tracks loaded successfully. The fail-fast Err is reserved for the case where *no* track loaded at all.
- **Mix tab no longer early-returns on `player_error`.** A partial player still renders its console; the error banner stays as a warning above the transport bar instead of replacing it. Combined with the tolerant `Player::new`, you can mix the surviving stems while seeing exactly which row went bad.
- **Full anyhow error chain in the player-error banner.** `format!("{e}")` only printed the top-level wrapper ("reading track …/track-010.wav") with the actual hound failure (file missing? corrupt header? path mangled?) hidden in the chain. Switched to `format!("{e:#}")` so the underlying cause renders inline.
- **Mix-tab rebuild loop on permanently-broken track rows.** The lazy-rebuild check compared `state.tracks.len()` (post-tolerant-load survivors) against `project.tracks.len()` (manifest count). With one track permanently failing to load, those values never matched and the player rebuilt every frame — re-loading every WAV every render, re-sending all warnings every render. New `Player.project_track_count` field captures the manifest count at build time; the rebuild check keys on that, so the broken-track case stabilises after one rebuild.

## [0.4.3] — 2026-04-28

### Fixed
- v0.4.2's cleanse protocol gated on `suno_mixdown_path: Some(_)` to identify Suno projects, but that field only exists on bundles imported in v0.4.0+. Suno projects imported in v0.3.x have `suno_mixdown_path: None` (serde default for older manifests) — and those are exactly the projects most likely to contain pre-v0.4.0-bug recording orphans. The cleanse silently no-op'd on every v0.3.x-vintage project. Detection signal expanded: a project is now considered Suno-shaped if EITHER `suno_mixdown_path: Some(_)` OR any track carries `TrackSource::SunoStem { .. }`. New regression test covers the v0.3.x scenario explicitly.

## [0.4.2] — 2026-04-28

### Added
- **Cleanse protocol** for legacy bug residue. Pre-v0.4.0, recordings could be appended to whatever project the user had open at capture time — including imported Suno stem projects. The result: a Suno project's `tracks/` ended up with `TrackSource::Recorded` orphans at the wrong rate, breaking `Player::new`'s uniform-rate check on the next Mix-tab visit. v0.4.2's cleanse runs at the top of every Mix-tab render: scans the active project for `Recorded` entries while `suno_mixdown_path: Some(_)`, moves each WAV out into the recordings filespace via atomic rename (cross-device fallback to copy+delete), mints fresh `track-NNN` ids in the recordings project so we never collide with existing recordings, and removes the orphans from the active project. Every `Track` field is preserved (gain, mute, automation, correction chain, polarity, etc.) — no work lost. Idempotent and cheap when there's nothing to do.
- New module `src/cleanup.rs` with `cleanse_recordings_in_suno_project(&mut Project) -> Result<CleanseReport>` and 4 unit tests covering empty-report behaviour, rate-mismatch flag rendering, failure-line rendering, and the non-Suno-project no-op path.
- Status bar surfaces a multi-line `CleanseReport.summary()` after migration: how many moved, any per-file failures with file name + reason, and a ⚠ warning when migrated takes don't match the recordings project's existing rate (which would break Mix on Recordings).

## [0.4.1] — 2026-04-28

### Fixed
- `stop_take` now keeps `app.project` in sync when the recordings project happens to be the active one (via File → Open Recordings). v0.4.0's `stop_take` saved the new take to the recordings manifest on disk but never updated the in-memory `app.project`, so a user who had the recordings project open and recorded a take saw "the take disappeared" until they reopened the project. Mirrors the existing pattern in `delete_recording`. Player is also dropped so it rebuilds with the new track count on the next Mix-tab visit.

## [0.4.0] — 2026-04-28 — "Suno-aware mixer"

A focused minor release built around the bundle → cleanup → mix → release path. Eleven per-role correction presets, import-time coherence verification, polarity flip, DC trim, Nyquist cleanup, BS.1770 LUFS metering, project-trim panel, and a dedicated recordings filespace with paged Record-tab list. Reference playback A/B and the multi-take browser are deferred to v0.5.0.

### Recordings filespace + Record-tab list UX

- **Recordings now live in a dedicated, app-owned filespace** at `%APPDATA%\TinyBooth Sound Studio\recordings\`, hosting a single persistent `.tinybooth` project that accumulates takes across sessions. Captures never contaminate the active stem-mixing project (Suno bundle or otherwise) — recordings and stem mixing are separate concerns. New `Config::recordings_root()` helper and `Project::open_or_create_recordings()` constructor.
- **Record tab redesigned**: the existing recorder header (profile / device / source / name / ⏺-⏹ / live waveform / spectrum / level meters) sits at the top, and a new paged "Recordings" list takes the rest of the tab. Each entry shows name (hover for the on-disk path), duration, mode, and the recording-tone profile; ▶ button sends a take to the main mixer in one click (swaps `app.project` to the recordings project, switches to Mix, solos that take, starts playback); 🗑 deletes the WAV + manifest entry. Pagination at 10 entries per page, newest first.
- **Mix-tab autoplay hand-off** — new `mix_autoplay_pending` + `mix_autoplay_solo_idx` fields on `TinyBoothApp`, consumed by the Mix-tab show() right after the lazy player rebuild. Solos the target track, rewinds to position 0, and starts playback in one go. Single-frame transition; the user clicks ▶ on a recording entry and hears it through the same console as their stem mixes.
- **Recording-rate enforcement**: `audio::start_recording` gains a `required_sample_rate: Option<u32>` parameter. The Record tab now keys this on the recordings project's existing rate (the rate of the first take ever captured into it), so subsequent takes always match. Cpal refuses up-front rather than landing a broken WAV and breaking the Mix tab on the recordings project later. Project's `tracks` is loaded fresh on every `start_new_take`/`stop_take` so the manifest stays the single source of truth — no in-memory dual-project state to drift.
- **`File → Open Recordings`** menu entry swaps `app.project` to the recordings project. Same shape as the existing `Open project…` flow but skips the recents-list bookkeeping (recordings are scratch, not user-curated).

### Project-trim panel (v0.4.0)

- **New isolated trim panel** opened from the Project tab via a `✂  Trim project…` button. Single batch operation: pick a `[start_secs, end_secs]` range, hit Apply, and every WAV in the project (stems + the bundled Suno mixdown) is cropped in place atomically (`.tmp` sibling + rename so a crash mid-write leaves either the old or the new file intact). Coherence analysis stays valid post-trim because every file in the project shares the same new frame-0.
- Concept and waveform-rendering pattern adapted from the sibling `SoundTrimmer` project; integration is intentionally lightweight — no per-track offsets, no manifest-schema changes, no engine surgery. The trim panel is modal-style and doesn't weave into the Mix tab. Per-track trim offsets and drag-handle visual selection are deferred to v0.5.0.
- `mm:ss.mmm` time entry with live parse feedback and over-end clamping. Small reference waveform thumbnail behind the start / end markers (drawn from the mixdown if present, else the first track). Failure breakdown in the status row when any file fails to trim, so the user can see which file (and why) without digging in the import log.
- New module `src/trim.rs` (backend) + `src/ui/trim.rs` (panel). 6 new unit tests on the `mm:ss.mmm` parse / format round-trip, including the bare-seconds and `ss.mmm`-only formats and the negative / garbage rejection paths.

### Suno-aware mixer — phase 3a of v0.4.0

- **LUFS metering on the master bus** (BS.1770-4). New `src/lufs.rs` module implementing the K-weighting filter cascade (pre-filter shelf + RLB high-pass), 100 ms-slice mean-square accumulation, and gated integrated loudness with the spec's −70 LUFS absolute gate + −10 LU relative gate. Audio thread feeds the master bus into the meter once per frame; UI reads the published readouts via atomics. New labelled monospace block on the Mix-tab transport bar: "M ±X.X · I ±X.X LUFS" — momentary 400 ms window plus gated integrated whole-programme. Hover tooltip names the streaming targets (Spotify −14, Apple Music −16, broadcast −23). Reads `—` until 400 ms have played; resets on Stop.
- **Mixdown loudness measured at import**. New `Project.suno_mixdown_lufs: Option<f32>` populated by a one-shot `compute_wav_integrated_lufs` pass over the bundled mixdown at import time. Logged in the import log alongside the coherence block; lays the groundwork for the matched-loudness reference A/B button (phase 3b).
- 5 new unit tests on the LUFS meter: silence integrates to NaN; a 1 kHz tone at −20 dBFS reads near −20 LUFS (within 1.5 LU); +6 dB amplitude shift produces +6 LU readout (verifies the dB↔LUFS arithmetic); momentary / integrated both return NaN before 400 ms of audio.

### Suno-aware mixer — phase 2 of v0.4.0

- **Import-time coherence analysis**. Every Suno bundle whose extracted contents include a mixdown WAV (filename containing `master`, `mix`, or `final` — the existing `StemRole::Master` heuristic) now triggers a coherence pass: sum all stems at unity gain, subtract the mixdown, compute residual RMS relative to mixdown RMS. Below ~−30 dB ⇒ stems compose cleanly; above ~−10 dB ⇒ a stem is missing, mislabelled, length-mismatched, or polarity-flipped.
- **Per-stem polarity-vs-mixdown check**. Pearson correlation between each stem and the mixdown over its active region. Stems with `r < −0.3` get flagged with an `⚠ ANTI-PHASE` badge in the import log and a "try the Ø button" pointer in the import-result modal. Doesn't auto-flip — that's a user decision — but surfaces the suggestion at exactly the moment the user is reviewing what just imported.
- **Mixdown stored as project reference, not summed track**. The bundled Suno mixdown WAV no longer becomes a regular `Track` (which would double the audio when the user hits Play). It's kept on disk in the project's `tracks/` folder but referenced via a new `Project.suno_mixdown_path: Option<String>`. The matched-loudness reference A/B button that uses this — switching the bus output between user-mix and bundled mixdown — is deferred to v0.5.0; v0.4.0 ships with the meter (phase 3a) and the import-time mixdown LUFS reading.
- New module `src/coherence.rs` — streaming f32-mono RMS / Pearson-correlation analysis at a 4 kHz decimation rate (memory bounded regardless of song length). 6 unit tests covering RMS edge cases, identity / inverted / orthogonal correlation, and the verdict-categorisation summary.

### Suno-aware mixer — phase 1 of v0.4.0

- **Per-role Suno-X preset library**. 11 new built-in presets (`Suno-Vocal`, `Suno-BackingVocal`, `Suno-Drums`, `Suno-Bass`, `Suno-ElectricGuitar`, `Suno-AcousticGuitar`, `Suno-Keys`, `Suno-Synth`, `Suno-Pads`, `Suno-Percussion`, `Suno-FxOther`) with chains tuned for each role's typical Suno artefacts. Added auto-seeding at import: each detected stem gets the matching Suno-X preset as its `correction` chain on import, so projects open with usable defaults instead of a flat unprocessed mix. Strings/Brass map to the closest existing chain (Pads / Synth respectively); Master and Unknown intentionally stay unseeded.
- **Two new processing primitives** on every `Profile`: `dc_remove_enabled` (sub-audible 5 Hz HPF that strips DC drift AI generators sometimes leave in stems) and `nyquist_clean_enabled` + `nyquist_clean_hz` (top-octave LPF, default 18 kHz, that suppresses Suno's characteristic shimmer in the top octave). UI rows for both in the Profile editor (Admin window + per-track Correction window). Signal flow: input gain → DC remove → HPF → EQ → de-esser → gate → comp → makeup → Nyquist clean. Both default off; the Suno-X presets opt in.
- **Polarity flip per track** (`Ø` button on the Mix-tab channel strip; standard audio-gear glyph for phase invert). Persists via `Track.polarity_inverted: bool`. Implemented zero-cost in the player: the per-buffer cache folds the ±1.0 sign factor into the pre-computed static linear gain, and the automation gain branch picks up the same factor — no extra multiplies in the per-frame hot path.
- **Profile-library forward-migration**. `dsp::load_or_seed` now appends any built-in preset whose name isn't already on disk, instead of only seeding a fresh file. Existing user-tuned profiles are preserved verbatim; the new Suno-X library is added once, ever, on next launch.

## [0.3.11] — 2026-04-28

### Fixed
- Mix tab fader sliders rendered as 14-px stubs at the top of their 130-px bounding boxes. v0.3.10 set `ui.style_mut().spacing.slider_width = 14.0` thinking that knob controlled rail *thickness*, but for a vertical slider in egui `slider_width` is the main-axis (rail) *length* — so the rail was clamped to 14 px. Set it to `FADER_H` (130) so the rail fills the bounding box `add_sized` allocates. Rail thickness comes from the cross-axis allocation (`rect.width() / 4` in egui's slider rendering), which is already substantial at the wider `STRIP_W` v0.3.10 introduced.

## [0.3.10] — 2026-04-28

### Added
- **View → UI scale slider** (0.75×–2.5×, 5% steps, percentage-formatted) so the entire interface — fonts *and* widget metrics — grows proportionally for high-DPI / accessibility / small-laptop scenarios. Persists via `Config.zoom`, applied through egui's `set_zoom_factor` so spacing and button hit-targets scale alongside text rather than text-on-tiny-buttons. Reset-to-100% button next to it.
- `.github/workflows/ci.yml` — runs the same three quality gates (`cargo fmt --check`, `cargo clippy --release --all-targets -- -D warnings`, `cargo test --release`) on every PR to `main` and every push to `main`, with concurrency-cancel and doc-only path filtering. Closes the gap that let v0.3.6→.7 and v0.3.8→.9 burn version numbers on toolchain-shape problems a PR-time gate would have caught.

### Changed
- **Mix tab — channel-strip visual pass.** `STRIP_W` 78 → 108 px; track-name font drops `.small()` for an explicit `13.0pt`; dB readout 12.0pt monospace; master strip name 14.0pt. M/S/R buttons grow from 20×18 → 26×22 and the row is `vertical_centered`-wrapped so it sits squarely under the name instead of left-leaning. Slider rail/thumb thickness bumped from the egui ~8 px default to 14 px (scoped per-strip, doesn't leak elsewhere). Frame `inner_margin` 6 → 8 px. Net effect: track names like "Backing Vocals" / "Electric Guitar" / "Synth / Lead" no longer chop mid-word; the dB readout stops wrapping into one-character-per-line stacks; faders read at a glance.
- Track-name truncation switched from a 9-byte hard slice (`&name[..9]`) to a UTF-8-safe ellipsis helper (`ellipsize(name, 14)`). The byte slice would have panicked on multi-byte chars like accented vowels or emoji in track names; the helper operates on `chars()`.

### Fixed
- `Config.zoom` now carries `#[serde(default = "default_zoom")]`. Without it, any `config.json` written before the field existed failed to parse, and the silent `.unwrap_or_default()` reset *every* preference (dark mode, recent projects, last project, profile name) on first launch with the new schema. Standard schema-migration discipline; should have been there from day one.

### Documentation
- `docs/architecture.md §6.2` rewritten to cover both workflows and a new §6.2.1 on the sync-tax trade-off (why duplicated gates beat reusable-workflow indirection at this scale, and what to keep aligned across `ci.yml` ↔ `release.yml`).
- Cross-reference comments at the top of `ci.yml` and on the toolchain step of `release.yml` so drift is visible at edit time.

## [0.3.9] — 2026-04-27

### Fixed
- CI install regression: pinning `dtolnay/rust-toolchain@1.95.0` (v0.3.7) doesn't ship `rustfmt` / `clippy` by default — versioned tags require an explicit `components:` block. v0.3.8's CI failed at `cargo fmt --check` with `'cargo-fmt.exe' is not installed`. Same content as v0.3.8 (which never produced an MSI) plus a two-line workflow change.

## [0.3.8] — 2026-04-27 *(no MSI; CI failed installing rustfmt)*

### Added
- `CHANGELOG.md` — this file. Hand-curated; release notes from the GitHub release page remain auto-generated from commit messages.
- `Track::recorded(...)` and `Track::from_suno_stem(...)` constructors so future schema additions don't fan out to every literal call site.
- Profile editor body shared between **Admin → Recording-tone profiles…** and **Mix → Correction…** windows via a new `ui::profile_editor` module — single source of truth for the input-gain / HPF / EQ / de-esser / gate / compressor / makeup chain UI.

### Changed
- `chrono` now ships with `default-features = false` (audit follow-up; `clock` + `serde` + `std` are the only pieces we use). Smaller dep tree and binary.
- CI's Rust toolchain is now pinned (`dtolnay/rust-toolchain@1.95.0`) — local-vs-CI clippy drift surfaces at PR time, not at tag-push.
- `Config::save` returns `Result<()>` and writes atomically via a `.tmp` sibling + `rename` so a crash or full disk mid-write doesn't leave the file truncated. The UI thread surfaces failures via the status bar.
- `export.rs::mixdown` no longer pre-multiplies samples by static gain at read time; gain is applied per-frame in the same loop as automation. Drops a ten-line apologetic comment about a "gain-undo trick" the previous shape required.
- `audio.rs` sample-format dispatch (mono and stereo branches) gains an inline comment explaining why the six near-identical match arms exist: monomorphisation forces one arm per concrete `T`, and a macro would obscure the call sites for marginal LOC gains. Rated *Nit* in the audit; this captures the decision in-source.

### Documentation
- `Track.profile` and `Track.correction` doc comments now explicitly distinguish their roles (recording-time snapshot vs post-processing chain).

## [0.3.7] — 2026-04-27

### Fixed
- CI clippy regression: `unnecessary_sort_by` on Rust 1.95.0 stable. Same content as v0.3.6 (which never got an MSI built — its CI run failed on this lint) plus a one-line `.sort_by_key(...)` swap.

## [0.3.6] — 2026-04-27 *(no MSI; CI failed on the new gates)*

### Added
- 27 inline unit tests across `automation`, `analysis`, `suno_meta`, `suno_import`, `git_update`, and `project`. Coverage matches the survival guide §9.1 payback list.
- CI quality gates: `cargo fmt --check`, `cargo clippy --release --all-targets -- -D warnings`, `cargo test --release` between version-check and build.
- Audio-thread error channel: `cpal` `err_fn` closures push through a `mpsc::Sender<String>`; the UI thread surfaces messages in the status bar instead of locking stderr.

### Changed
- `git_update::render` returns `bool` (`#[must_use]`); on a successful installer launch the caller closes via `egui::ViewportCommand::Close` so Drop impls run (WAV writers finalise, config saves). Pre-v0.3.6's `process::exit(0)` skipped Drop entirely.
- `git_update.rs` switched from `Result<_, String>` to `anyhow::Result`; `.map_err(format!)` calls become `.context(...)` chains.
- Clippy hygiene: 14 warnings → 0 (redundant closures simplified, manual `div_ceil` → `.div_ceil()`, derived `Default` impls, three `else if` collapses, four `#[allow(too_many_arguments)]` on internal helpers).
- `cargo fmt` ran across the tree; 23 files reflowed.

## [0.3.5] — 2026-04-27

### Changed
- "Enable all corrections" button glyph: `+` → `✓`. The plus read as a small cross next to the destructive `⟲ Reset`; checkmark is the affirmative action.

## [0.3.4] — 2026-04-27

### Added
- Persisted **Disable** button on the Mix tab. Flips `Project.corrections_disabled`, syncs `PlayerState.global_bypass`. Survives reload — non-destructive project-wide bypass.
- `Project.default_correction` field. Drives the Enable cascade: existing `Track.correction` → `Project.default_correction` → feature default (Suno-Clean).

### Changed
- Existing destructive **Disable all** button renamed to **⟲ Reset all** to clarify it strips chain configs.
- `enable_all_corrections` now uses the three-step cascade above.
- Phase-B audio-callback refactor: zero per-callback `Vec` allocations; per-buffer cache for atomic loads (~250× fewer per typical 256-frame buffer); static fader gain pre-converted to linear once per buffer instead of per-sample `db_to_lin`.

## [0.3.3] — 2026-04-27

### Added
- Ephemeral global A/B toggle on the Mix tab transport. Flips player's `global_bypass` atomic without touching the project state. Mid-playback, instant.

## [0.3.2] — 2026-04-27

### Added
- Bulk correction toggles on the Mix tab transport: `+ Enable all corrections` / `− Disable all`. Adaptive labels showing how many tracks each affects.

## [0.3.1] — 2026-04-27

### Added
- Suno session metadata captured at import: epoch (Unix integer seconds, sortable directly), ordinal (project-relative monotonic), provenance.
- Duplicate-import detection: re-importing the same Suno render triggers a Replace/Cancel modal before any files are touched.
- `Project.next_suno_ordinal` counter; bumped on every successful import.

## [0.3.0] — 2026-04-26

### Added
- **Console mixer** on the Mix tab — vertical fader strips per track plus a master strip with stereo meters, M/S/R toggles.
- **Volume automation** — fader gestures recorded during armed playback, replayed via Catmull-Rom splines (`splines` crate). Per-track and per-master.
- `Track.gain_automation`, `Project.master_gain_automation`, `Project.master_gain_db`.

## [0.2.2] — 2026-04-26

### Fixed
- Suno import was silent on failure. Now lenient (skips bad entries instead of bailing); writes a per-import diagnostic log to `%APPDATA%\TinyBooth Sound Studio\logs\`; pops a modal after every import (success or fail) with summary, log path, and Open Log Folder button.

## [0.2.1] — 2026-04-26

### Added
- Auto-restore last project on startup via `config.last_project_path`.
- File → Open Recent (eight most-recently-opened, dead entries auto-pruned).

## [0.2.0] — 2026-04-25

### Added
- **Mix tab** with multitrack waveform lanes, synchronized playhead, transport, per-track A/B bypass, Correction editor.
- `src/player.rs` — cpal output stream, pre-loaded track buffers, atomic playhead, transport state.
- `Track.correction: Option<Profile>`; mixdown at export honours it.

## [0.1.6] — 2026-04-25

### Added
- DSP substrate from TBSS-FR-0001: parametric EQ + de-esser added to `FilterChain` / `FilterChainStereo`; `Suno-Clean` preset shipped.

## [0.1.5] — 2026-04-25

### Added
- In-app manual: 12 chapters embedded via `include_str!` of `docs/manual/*.md`. `Help → Manual…` or `F1` anywhere.

## [0.1.4] — 2026-04-24

### Added
- Suno stem bundle ingestion (folder + zip). `TrackSource::SunoStem { role, original_filename }`. `StemRole` covers the documented 12-stem set plus `Instrumental`/`Master`/`Unknown`.

## [0.1.3] — 2026-04-19

### Added
- Stereo visualisation: dual waveforms, dual peak meters in stereo recording mode.

## [0.1.2] — 2026-04-19

### Added
- Real brand icon (walnut booth + cream mic + teal waveform). Multi-size ICO; window viewport icon embedded in exe; banner README header.

## [0.1.1] — 2026-04-19

### Added
- Stereo recording: `SourceMode { Mixdown, Channel(u16), Stereo }`. `FilterChainStereo` with envelope-linked gate + compressor.

## [0.1.0] — 2026-04-19

Initial release. Skeleton-bootstrapped Rust + egui app:

- Record tab with cpal input, channel/mixdown selection, recording-tone presets (Guitar default), live waveform + spectrum + peak meter.
- Project tab with track table, JSON manifest format (`.tinybooth`).
- Export tab: WAV native via hound; FLAC/MP3/Ogg/Opus/M4A via ffmpeg subprocess.
- Self-update via GitHub Releases.
- WiX MSI installer; tag-driven CI.

[Unreleased]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.3.8...HEAD
[0.3.8]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.3.7...v0.3.8
[0.3.7]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.3.6...v0.3.7
[0.3.6]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.3.5...v0.3.6
[0.3.5]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.3.4...v0.3.5
[0.3.4]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.1.6...v0.2.0
[0.1.6]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/ophiocus/TinyBoothSoundStudio/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/ophiocus/TinyBoothSoundStudio/releases/tag/v0.1.0
