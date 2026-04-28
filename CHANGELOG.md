# Changelog

All notable changes to TinyBooth Sound Studio.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); this project tracks [Semantic Versioning](https://semver.org/) loosely (the v0.x series treats minor bumps as feature releases, patch bumps as fixes / polish).

## [Unreleased]

### Suno-aware mixer — phase 2 of v0.4.0

- **Import-time coherence analysis**. Every Suno bundle whose extracted contents include a mixdown WAV (filename containing `master`, `mix`, or `final` — the existing `StemRole::Master` heuristic) now triggers a coherence pass: sum all stems at unity gain, subtract the mixdown, compute residual RMS relative to mixdown RMS. Below ~−30 dB ⇒ stems compose cleanly; above ~−10 dB ⇒ a stem is missing, mislabelled, length-mismatched, or polarity-flipped.
- **Per-stem polarity-vs-mixdown check**. Pearson correlation between each stem and the mixdown over its active region. Stems with `r < −0.3` get flagged with an `⚠ ANTI-PHASE` badge in the import log and a "try the Ø button" pointer in the import-result modal. Doesn't auto-flip — that's a user decision — but surfaces the suggestion at exactly the moment the user is reviewing what just imported.
- **Mixdown stored as project reference, not summed track**. The bundled Suno mixdown WAV no longer becomes a regular `Track` (which would double the audio when the user hits Play). It's kept on disk in the project's `tracks/` folder but referenced via a new `Project.suno_mixdown_path: Option<String>`. Phase 3 will surface this as the auto-loaded reference for loudness-matched A/B from the Mix tab.
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
