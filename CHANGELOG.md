# Changelog

All notable changes to TinyBooth Sound Studio.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); this project tracks [Semantic Versioning](https://semver.org/) loosely (the v0.x series treats minor bumps as feature releases, patch bumps as fixes / polish).

## [Unreleased]

(Nothing yet — known issues all resolved as of v0.4.23.)

## [0.4.32] — 2026-05-13

### Fixed
- **The Mix-tab lane overlap is finally fixed for real this time.** v0.4.31 moved the Mix-tab panels to ctx level but bundled all three (`mix_transport_panel` top, `mix_console_panel` bottom, lanes `CentralPanel`) into a single `mix::ctx_panels(app, ctx)` call placed AFTER the global `bottom_bar` panel declaration. That violated egui's strict panel-order requirement: **all `TopBottomPanel::top` calls must precede all `TopBottomPanel::bottom` calls, which must precede the `CentralPanel`**. With `bottom_bar` declared before `mix_transport_panel`, the bottom-of-screen space was claimed before the Mix-tab top was, scrambling egui's space accounting — visible as the Vocals lane content rendering at the same Y as the transport bar.
- The fix interleaves Mix-tab panel declarations across `app.rs::update()` in the correct order:
  1. `top_bar` (menu) — line ~1321
  2. `mix_transport_panel` (Mix-tab top) — immediately after, when `mix_active`
  3. `bottom_bar` (status) — line ~1591
  4. `mix_console_panel` (Mix-tab bottom) — immediately after, when `mix_active`
  5. `CentralPanel` (tab body) — always last; hosts lane stack for Mix or the tab body for everything else
- `mix.rs` now exposes the helper API the new layout needs: `pre_render(app)`, `compute_console_h(app, ctx)`, `render_transport(app, ui)`, `render_console(app, ui)`, `render_lanes(app, ui)`. The previous `ctx_panels(app, ctx)` wrapper is gone — it was the wrong abstraction (couldn't be placed correctly relative to the global panels).

## [0.4.31] — 2026-05-13

### Fixed
- **Lane content rendering at wrong Y coords — root cause finally found and fixed.** The bug was visible across v0.4.29 / v0.4.30: the first lane's name + chips + M/S/A/B/+Cor row would render at the top of the screen overlapping the global menu bar, while the `Frame::group` border for that lane was either missing or in its proper place below the transport bar. Different parts of the same row drawing in different y-coords pointed at a **painter-layer / Z-order** issue, not just bad rect maths.
- The actual cause: egui's painter uses **multiple layers** (Foreground for widgets, Background for panel fills, Tooltip for popups, plus ScrollArea-internal sublayers). Nesting `TopBottomPanel::show_inside` / `child_ui` + `set_clip_rect` inside the app's global `CentralPanel::show(ctx, ...)` only constrains the *immediate* layer — `ComboBox` popups, `ScrollArea` viewports, and tooltip rendering bypass the child's clip_rect and use the OUTER `ui`'s. So the lane's `Frame::group` (drawn directly in the immediate layer) sat where it should, while the ComboBox / ScrollArea content for that row ended up unbounded.
- **Fix:** the Mix tab now declares its three panels (`mix_transport_panel`, `mix_console_panel`, lanes `CentralPanel`) **at ctx level**, as siblings of the app's global menu bar (`top_bar`) and status bar (`bottom_bar`), rather than nested inside the global `CentralPanel`. This is the egui-blessed pattern for a multi-pane workspace — egui's panel system composites these cleanly because they all draw to the same level, with no painter-layer mismatch. New `mix::ctx_panels(app, ctx)` function called directly from `app.rs` when `Tab::Mix` is active.
- `app.rs` gains a branch: for Mix tab with tracks (and not the Visualizer takeover), it calls `mix::ctx_panels(self, ctx)`; everything else continues to render inside the global `CentralPanel` via `mix::show(self, ui)` (now a thin placeholder for the empty-project case).
- Removed `TRANSPORT_BAR_H` constant and the `render_clipped` helper from v0.4.30 — both were ceremony for the failed child_ui approach.

## [0.4.30] — 2026-05-13

### Fixed
- **Lane content no longer bleeds above the transport bar.** v0.4.29's nested-panel approach (`TopBottomPanel::top` + `TopBottomPanel::bottom` + `CentralPanel`, all via `show_inside`) misbehaved when hosted inside the app's outer `CentralPanel::show(ctx, ...)` — the first lane row would render *above* the lanes region, overlapping the global menu bar. Replaced with explicit `child_ui` regions whose `clip_rect` and `max_size` are both pinned to a pre-computed `Rect` taken from `ui.max_rect()`. The result: each of the three regions (transport, lanes, console) is a hard-clipped rectangular zone, content overflow is physically impossible, and the layout no longer depends on egui's nested-panel internals being well-behaved.
- New helper `render_clipped(parent, rect, id, |ui| …)` centralises the child-ui pattern so all three regions share the same clipping contract — no copy-paste of the `child_ui_with_id_source` + `set_clip_rect` + `set_max_size` triple.
- Added `TRANSPORT_BAR_H = 56.0` so the transport region has a known fixed height, preventing the lanes/console below it from shifting when transport content changes width (e.g. the error banner appearing/disappearing).

## [0.4.29] — 2026-05-12

### Changed — Mix tab GUI architecture
Full architectural rewrite of `mix::show()`. The Mix tab now uses egui's native panel layout instead of stacked `allocate_ui_with_layout` calls, fixing three independent bugs that all traced back to that approach:

```
┌─ TopBottomPanel::top  ────── transport bar + error banner
├─ CentralPanel        ────── lanes (the ONLY surface that takes vertical scroll)
└─ TopBottomPanel::bottom ── spectrum + strip cards (horizontal scroll only)
```

#### Why three panels, in this order
- Each egui panel owns its own **clip rect**, so content can no longer bleed between surfaces. The track headers / waveforms physically cannot reach into the transport bar above or the console deck below — that's been the actual bug under "headers bleeding top and bottom" since v0.4.21.
- Each panel owns its own **scroll-event hit-testing**. Pre-v0.4.29 both the lanes and the console shared the caller's `ui`, so a wheel event in the wrong place could shift either. Manifested as "the cards jitter in place when I scroll".
- The bottom panel claims an `exact_height(console_h)` based on `mix_console_fraction`; the CentralPanel takes whatever's left. The split no longer floats with `ui.available_height()` (which depended on whatever else the parent had drawn — including the top-bar readings changing width when a digit ticked), so the layout doesn't wobble by a px each frame.

#### Sub-surface scroll lock
- **Lanes** — `ScrollArea::vertical().auto_shrink([false; 2])`, fills the CentralPanel.
- **Console deck** — `ScrollArea::new([true, false])` (explicit hscroll-only + vscroll-disabled) + `auto_shrink([false; 2])`. Vertical wheel events inside the deck no longer try to scroll a 0-height extent, which was the actual cause of the "jittering in place" visual.

#### Code factoring
- `show()` extracted into four cohesive helpers — `rebuild_player_if_needed`, `render_player_error_banner_if_present`, `consume_autoplay_request`, plus the three panel renderers. No more 130-line function with player-lifecycle, autoplay, layout maths, and scroll plumbing tangled together.
- Manual drag handle for the lane↔console split removed. The split is now fully driven by `mix_console_fraction` + `CONSOLE_H_MAX`. If interactive resize comes back, it'll be via egui's native `TopBottomPanel::resizable(true)` rather than a hand-rolled `allocate_exact_size` + `drag_delta()`.

#### Functional invariants preserved
- Player lazy-rebuild + error banner + Retry button.
- Auto-play hand-off from Record-tab ▶ clicks.
- Automation arm / capture / commit loop on per-strip and master.
- Spectrum panel position (top of the bottom panel, pinned above the strip cards).
- Telemetry chips, profile dropdown, M/S/A/B/Cor row, hot-load swap, every per-track button — unchanged.
- 75 tests still passing; clippy clean with `-D warnings`.

## [0.4.28] — 2026-05-12

### Changed — release-pipeline speedups
- **`cargo clippy --release` + `cargo test --release` dropped from `release.yml`.** Both gates already run on every PR via `ci.yml` in debug mode (faster, catches the same correctness issues). Re-running them in *release* mode at tag-push time was redundant paranoia — by the time a commit hits `main` and gets tagged, it has already cleared CI. Saves ~3–5 min per release.
- **`cargo-wix` install cached** by `~/.cargo/bin/cargo-wix.exe`, keyed on the toolchain version. v0.4.27 and earlier compiled `cargo-wix` from source on every build. Saves ~1–2 min.
- **FFmpeg LGPL download pinned + cached.** Pre-v0.4.28 the workflow pulled `https://.../releases/download/latest/...` every build — 120 MB over the network for the same binary. Now pinned to `autobuild-2026-05-12-13-59` (a dated BtbN tag, stable) and cached by that key. The asset URL is resolved from the GitHub API rather than guessed, so the file-naming churn between autobuild tags doesn't break the workflow. Bump the pin intentionally when a new FFmpeg version is wanted. Saves ~30–60 s.

Net: ~12 min builds → **~5–7 min builds**, zero binary changes. The release artifact is byte-equivalent to v0.4.27 modulo the version-string bump.

## [0.4.27] — 2026-05-12

### Added — master input/output configuration
- **Admin → Audio devices…** modal. Picks both the master input device (used by the Record tab) and the master output device (used by Mix-tab playback) from cpal-enumerated lists. Each pick shows the device name plus its native channel count and sample rate. Empty pick = "follow the platform default" — useful when the user wants to track Windows' active default rather than pinning a specific device.
- **`Config.input_device` / `Config.output_device`** (`Option<String>`, both `#[serde(default)]`). Both persist to `config.json` so the picks survive app restarts. Older configs auto-migrate to `None` (= follow platform default), which preserves v0.4.26 behaviour exactly for everyone who hasn't touched the new panel.
- **Graceful fallback when a saved device disappears.** When `Config.input_device` or `output_device` references a name that no longer matches any enumerated device (user unplugged the USB mic between sessions, switched ports, etc.), the resolver falls through to the platform default rather than erroring out. New helpers `audio::input_device_by_name(Option<&str>)` and `audio::output_device_by_name(Option<&str>)` centralise this lookup.
- **Rescan button** in the panel — re-enumerates cpal's device list mid-session. Plug in a USB mic, click Rescan, the dropdowns update without an app restart.

### Changed
- **`Player::new` signature gains `output_device_name: Option<&str>`.** Threads through to `build_output_stream`, which now calls `audio::output_device_by_name` instead of the hard-wired `default_host().default_output_device()`. Same fast-fail probe at the top of `Player::new` so a missing chosen device still cheap-errors before the WAV-loading phase.
- **Mix-tab player rebuild** triggered automatically when the output device is changed mid-session — drops `app.player` so the next Mix-tab frame rebuilds with the new device. Playback stops; user hits Play to resume.
- **Record tab's input-device dropdown** now restores its previous selection from `Config.input_device` at startup, falling back to the platform default if the saved device is no longer enumerated.

## [0.4.26] — 2026-05-12

### Changed
- **Clicking the version label always does the round trip**, even when an update is already known to be available. Pre-v0.4.26 the click handler was gated on `state == Idle` — so once an "v0.4.x available — click to install" badge appeared next to the version label, clicking the label itself did nothing. That made the label feel half-broken: it advertised itself as click-to-refresh, but at the very moments you'd want to refresh (e.g. "is there an even newer release than the one shown?") the click was inert. Now the click forces a fresh `check_latest_release()` whenever the updater isn't in the middle of a check or a download (those two states are still guarded so we don't race on the receiver). Hover-text added to the label spelling out the behaviour.

## [0.4.25] — 2026-05-12

### Fixed
- **Lanes no longer overlay the transport bar.** The Mix tab used `TopBottomPanel::top("mix_lanes_panel").show_inside(ui, ...)` for the lanes block and `TopBottomPanel::bottom(...)` for the console deck. `show_inside` positions panels using the parent ui's `max_rect`, IGNORING the current cursor — so the lanes panel landed at the absolute top of the central area, painting over whatever the `transport_bar` had drawn there. Visible in the v0.4.24 screenshot as the Vocals lane bleeding over the `Mix · Pause · Stop · Enable corrections …` row. Replaced both `TopBottomPanel::show_inside` calls with `ui.allocate_ui_with_layout(...)` — that respects the cursor, so the lanes sit cleanly below the transport bar and the console deck cleanly below the resize handle.

## [0.4.24] — 2026-05-12

### Fixed
- **Strip cards no longer staircase diagonally** in the console deck. `ui.horizontal` defaults to `Align::Center` on the cross axis, so cards with even slightly different effective heights drifted down each step (visible as a Vocals → Backing Vocals → Drums → Bass cascade in the v0.4.23 screenshot). Switched to `ui.with_layout(Layout::left_to_right(Align::Min), ...)` — every card's top edge now sits on the same y baseline.
- **Lane header bleed killed**. v0.4.21's 1-px line divider between rows was too subtle to break the eye-fuse between adjacent headers. Each lane is now wrapped in its own `Frame::group` with a dark fill, so every row is a visibly bounded card with a clear border on all sides. Removed the redundant divider.
- **First lane no longer kisses the transport bar above it** — added 4 px of explicit top padding inside the lanes ScrollArea so the Vocals row's name has breathing room from the controls row above.
- **`scripts/ship.ps1` poll fixed.** v0.4.23's first run sat polling for 14 minutes on a release that had been published 6 minutes earlier. Two bugs: (a) `gh release view --json publishedAt, assets` was being parsed by PowerShell as two arguments, and `gh` rejected the second one (`Unknown JSON field: " assets"`) with a non-zero exit code that the script swallowed via `2>$null`. Fixed by quoting the field list as a single token (`'publishedAt,assets'`) and gating on `$LASTEXITCODE`. (b) The published-detection check used a regex `^20\d\d-` against `$pubAt`, but PowerShell 7's `ConvertFrom-Json` auto-parses ISO 8601 strings into `[DateTime]` objects whose `ToString` produces `MM/dd/yyyy …` in US locales — the regex never matched. Replaced with a non-null check.

## [0.4.23] — 2026-05-12

### Fixed
- **In-app updater no longer goes stale for the session.** The long-standing known issue tracked since v0.4.12 — bottom-bar version label showing the install version indefinitely because `check_latest_release` fired only once at app startup — is closed. New `git_update::maybe_spawn_recheck` runs every frame, rate-limited at `RECHECK_INTERVAL = 300 s`, gated on `state == Idle && rx == None`. Two triggers force a non-rate-limited recheck: (a) the 5-minute timer expiring, (b) any tab transition (Record ↔ Project ↔ Mix ↔ Export). The check itself is a single small JSON GET, so the work is bounded; the gate guarantees we don't fire while a previous check is still in flight or while the user is mid-update. ~25 LOC in `src/git_update.rs` plus two new fields on `TinyBoothApp` (`last_update_check_at: Option<Instant>`, `last_tab_seen: Option<Tab>`). No new dependencies.

### Added — ship-flow tooling
- **`scripts/ship.ps1`** — PowerShell script that owns the full "tag pushed → MSI downloadable" arc, not just the push. Pushes main + the tag, then **blocks** polling `gh release view <tag>` every 15 s until `publishedAt` becomes a real ISO timestamp, then prints the asset SHA-256 fingerprints and download URLs. Hard 30-min timeout so a stuck CI run can't hang the script forever. Closes the operator-side half of the same gap the updater fix closes on the app side: before today, "ship" was `git push --tags` plus a vibe — no signal that the release-build pipeline was healthy and the artifact was actually downloadable. Usage: `.\scripts\ship.ps1 -Tag v0.4.24`.

## [0.4.22] — 2026-05-12

### Changed
- **Strip-card name labels rotated 90° (top-to-bottom reading)**, sitting in a narrow gutter on the left edge of every card instead of a horizontal centred label across the top. Saves a full row of vertical space inside each card (the fader rail claims that height now), and matches the classic mixing-console label-runs-along-the-side aesthetic. Implemented via egui's `epaint::TextShape::angle = π/2`. Same treatment on the master strip with its existing yellow accent.
- **Playback readings collapsed into the top bar as a right-hand aside.** Time (pos / dur), sample rate, and momentary / integrated LUFS used to live in the Mix-tab transport bar, taking up a row of horizontal space. They now sit in monospace font on the right side of the top menu bar next to the project name, visible from every tab (was Mix-only). Format: `M ±NN.N  I ±NN.N LUFS   48000 Hz   02:06/03:20`. Fixed-width per-field padding so the digits don't jitter.
- **Transport bar slimmed.** With readings out, the bar is now a tight strip of controls: Play / Pause / Stop · Enable corrections · Disable (saves) · Reset · A/B. No more 1280-px-wide row of mixed-info-and-controls.
- **Strip-card button row tightened**: M / S / R / Ø shrunk 22×22 → 18×20 so they fit alongside the new label gutter inside the fixed STRIP_W = 108. Inner margin tightened 8 → 6 for the same reason.

## [0.4.21] — 2026-05-12

### Fixed
- **Strip cards no longer balloon on tall windows.** v0.4.19's "stretch fader rail to fill available height" change ran unbounded — on a window where the console-deck pane was 500+ px, each strip's fader rail grew to 400+ px and the cards became "gigantic" (user screenshot). Added two hard caps: `FADER_H_MAX = 200` (rail max) and `CONSOLE_H_MAX = 340` (whole deck max). The drag handle between lanes and deck still adjusts `mix_console_fraction`; the caps just prevent the deck from eating the screen. Net effect: tall windows get more vertical space for the lane stack (which is where you actually mix), less for the strips (which don't need 400 px of rail).
- **Lane headers stop bleeding into each other.** v0.4.18's `LANE_H = 52` was a hair too tight for the 2-row header (name + chips above, M / S / A/B / Cor + profile dropdown below), so adjacent rows visually fused with no clear boundary. Bumped `LANE_H` 52 → 62 and `ROW_GAP` 4 → 8, and added a 1-px horizontal divider centred in the row gap. Each lane is now a clearly bounded card with comfortable padding.

## [0.4.20] — 2026-05-12

### Added — advanced stem-project management
- **Hot-load: ↔ Swap audio.** New button on every Project-tab track row. Pick a WAV; the bytes replace the track's audio in-place, preserving every other field on the manifest — track name, role, correction chain, volume automation, polarity flip, telemetry profile. Sample-rate enforcement: the new file has to match the project's existing rate (TBSS still has no resampler; mismatched rates would break the Mix tab silently). On mismatch the swap is refused with a clear status, nothing on disk changes. On success the project is **auto-saved**, the player drops itself so the next Mix-tab frame rebuilds with the new audio, telemetry is invalidated and re-dispatched, and the project-level Krumhansl-Schmuckler key estimate is recomputed because old pitch histograms no longer apply.
- **Transparent TinyBooth metadata injection** — every hot-loaded WAV gets a TBSS JSON blob written into its standard RIFF `LIST/INFO/ICMT` (comment) field before the file goes live. The blob carries: project name, source classification (Suno role / Recorded / TinyDAW take), polarity-inversion flag, active correction-profile name, telemetry profile, and a `tinybooth-sound-studio v0.4.20` produced-by string. Any RIFF-aware reader (exiftool, foobar2000, our own `suno_meta::read_wav_session`) sees a standard comment; TBSS sees a structured record it can round-trip. New module `src/wav_meta.rs` with `inject_tbss_meta` (write side) + `read_tbss_meta` (read side, reserved for the upcoming "drop a WAV onto the Project tab → mint a track preserving TBSS context" feature).
- **Atomic on-disk writes.** Hot-load swap and metadata injection both write to a `.swap-tmp` / `.tbss-tmp` sibling then rename over the live file, so a process crash mid-swap can never leave a half-written WAV in the project folder.

### Added — TinyDAW project template
- **File → New TinyDAW project…** — creates a non-Suno, recording-centric project. The Mix tab, Export, Health panel, telemetry, automation, correction-chain UX are all identical; what changes is the routing rule: a Suno / untitled project sends captured takes to the canonical recordings filespace at `%APPDATA%\TinyBooth Sound Studio\recordings\`, while a TinyDAW project receives its takes directly into its own folder. Switches to the Record tab on creation so the next click ⏺ goes into the new project.
- **New `Project.kind` field** — `ProjectKind { Standard, Recordings, TinyDAW }`. `Standard` (default) preserves v0.4.19 behaviour for every existing manifest. The canonical Recordings filespace gets tagged as `Recordings` on its next open (one-time migration via the existing `open_or_create_recordings` path; the field is `#[serde(default)]` so older manifests don't reset).

### Added — advanced non-stem project management
- Both **hot-load swap** and **transparent TBSS metadata injection** apply identically to TinyDAW projects — they're per-track operations that don't care about Suno context. A TinyDAW user can swap any recorded take with a different WAV (e.g. replace take 3 with the cleaner take 5, keeping its correction chain), and every WAV the project produces carries the project provenance in its RIFF comment.

### Tests
- 3 new unit tests in `src/wav_meta.rs`: inject+read round-trip (with hound reopening the file to prove the WAV is still valid), repeated injection replaces the previous blob (no unbounded file growth), `read_tbss_meta` returns `None` on a plain WAV. Total suite: 72 → **75 passing**.

## [0.4.19] — 2026-05-11

### Changed
- **Spectrum panel relocated from top-of-Mix-tab to top-of-console-deck.** Sits directly above the fader strips now, so the meter ↔ spectrum comparison happens in one glance instead of being a screenful apart. The console deck's vertical budget grew as a result — see strip redesign below. Toggle remains under Admin → Show spectrum panel (Mix tab).
- **Strip cards now stretch the fader rail into their full vertical space.** Pre-v0.4.19 the fader was pinned at `FADER_H = 130` regardless of how tall the console-deck region was, leaving a wide blank zone below the dB readout on tall windows. The strip (and the master strip) now compute `fader_h = available_h − 110` so the rail + peak meter fill whatever's left after the label / button-row / dB-readout claim their fixed share. Floors at the old `FADER_H = 130` so a too-short console deck still shows a usable fader. The peak meter scales with it so a louder signal now sweeps the full height of the card, matching the recalibrated spectrum panel.

### Fixed
- **Spectrum panel was completely saturated all the time.** Root cause in `analysis::spectrum`: the FFT bin magnitude wasn't window-corrected. For a 0 dBFS sine at FFT bin centre, the raw Hann-windowed bin reads as `N/4 ≈ 1024` (at `N = 4096`), and `20·log10(1024) ≈ +60 dB`. The old `((db + 80) / 80)` mapping clamped that to `1.0` immediately, so any real music content pinned every bar at the top. Two-line fix: multiply the magnitude by `4 / N` (Hann amplitude-coherent-gain inverse) so a 0 dBFS sine actually reads as 0 dBFS, then map `((db + 90) / 100)` for the bars — `-90 dB → 0`, `0 dB → 0.9` (10% headroom at the top for transient overshoots). Existing spectrum-floor and peak-bin-position tests still pass.
- **Strip-card bottom space no longer wasted.** Same issue as above — fader was fixed-height inside a taller frame. Fixed by the fader-stretch change.

## [0.4.18] — 2026-05-11

### Added
- **Mix-tab spectrum panel** — pinned at the top of the Mix tab when enabled (default on). Live FFT of the master output bus drawn as bars on a log-frequency X axis (20 Hz → 20 kHz, with 100 Hz / 1 kHz / 10 kHz decade gridlines), normalised log-mag Y axis, plus a slow-release peak-decay trail (0.95×/frame ≈ 1 s release at 30 fps) sitting above the live spectrum. No new audio-thread plumbing — reads the same `PlayerState.output_viz` master-bus tap that v0.4.11 added for the standalone visualizer canvas. New module [src/ui/spectrum_panel.rs](src/ui/spectrum_panel.rs).
- **Admin → Show spectrum panel (Mix tab)** checkbox toggles the panel on / off, persisted via `Config.show_spectrum_panel` (default `true`, `#[serde(default)]` so old `config.json` files don't reset). v0.4.18 adds the field; older installs gain it on first save.

### Changed
- **Mix-tab lane headers compacted from 3 rows → 2.** Pre-v0.4.18 each lane header was three rows tall (name + profile dropdown / chips / M·S·A/B·+Correction), needing `LANE_H = 72`. New layout: row 1 = track name + telemetry chips, row 2 = M / S / A/B / Cor + profile dropdown. `LANE_H` dropped 72 → 52, `ROW_GAP` 6 → 4. Net: ~28% more lanes visible per screen height. The "+ Correction" button label shortened to "Cor" / "+Cor" so the row fits inside the existing 240-px `HEADER_W` without crowding the dropdown. Hover-text on the button still carries the full explanation.

## [0.4.17] — 2026-05-11

### Fixed
- **Drum classifier no longer over-counts events ~3-6×.** The v0.4.13 multi-band onset detector emitted one event per (band, frame) pair — so a single snare hit, which produces real flux peaks in MID + HIGH_MID + HIGH simultaneously, generated separate Snare + Cymbal + HiHat events. Real-world numbers on a 3:20 Suno drum stem: ~5,300 total drum events ≈ 27/sec (physically impossible — sane rate is 3–8/sec). `classify_drum_events` now flattens every per-band onset into a single time-sorted candidate list, clusters candidates whose frames fall within 3 of each other (the same `< 3` window the universal `all_onset_frames` dedup already uses in `analyze_wav`), and per cluster picks the dominant band by **normalised flux strength** (raw flux / band's flux max — the only fair cross-band comparison since absolute flux magnitudes vary wildly between low and high frequencies). The dominant band's frame alone runs through the existing kick/snare/hat/tom/cymbal classification. Total drum-event count on a typical 3-minute stem now lands in the 500–1500 range, distributed across classes — the same order of magnitude as the (correctly deduplicated) universal `tel.onset_count`.
- **`ANALYZER_VERSION` bumped 2 → 3.** Existing v0.4.13–16 manifests are treated as stale and re-analyzed on next project open. Migration is invisible — the dispatcher already skips up-to-date rows. Drum-event chips and Project Health rolls-ups will repopulate with sane counts after the first re-analysis pass.

### Tests
- New unit test `drum_classifier_dedupes_per_hit_no_double_count`: synthesises a 1-second WAV with one kick (60 Hz sine, 50 ms decay, sub-band only) at 0.2 s and one snare (broadband xorshift noise burst, 40 ms decay, fires MID + HIGH_MID + HIGH simultaneously) at 0.6 s. Asserts the drum classifier produces exactly 2 events total — the snare cluster collapses correctly. Pre-v3 this test would have failed with 4–6 events.
- Total suite: 69 → **70 passing**.

## [0.4.16] — 2026-05-08

### Added
- **Per-channel `M` (mute) and `S` (solo) buttons on every Mix-tab lane header.** Previously only available in the console deck strips at the bottom of the tab — invisible while working in the lane view. Now mirrored at the lane level next to `A/B` and `+ Correction`. The atomic flags (`track.mute`, `track.solo`) are shared with the console-deck strip + the audio thread, so flipping in one place reflects everywhere immediately.

### Fixed
- **Lane waveforms now share a common X-start across every row.** v0.4.15's `allocate_ui_with_layout(vec2(HEADER_W, LANE_H), …)` was a *suggested* size — when the inner content's natural width exceeded HEADER_W (chip strip, profile dropdown text), it grew the box and pushed the lane allocation right by a handful of pixels. Every row's waveform / playhead landed at a slightly different X. Replaced with `allocate_exact_size(…)` + a `child_ui` whose clip-rect is set to the header rect, so any inner overflow is hard-clipped and the lane allocation begins at exactly `HEADER_W` past the row's start regardless of telemetry density.
- **`HEADER_W` bumped 220 → 240** and **`LANE_H` bumped 60 → 72** to give the new third row of buttons (M / S / A/B / +Correction) breathing room without crowding the profile dropdown above.

## [0.4.15] — 2026-05-08

### Changed
- **Mix-tab telemetry chips: single-line consolidated strip.** v0.4.13–14 rendered each telemetry feature as its own chip — bright/dark + sustained/percussive + dense + 5 separate drum-class counts (`K744 S1789 h1288 T1280 C232`) + guitar pick chip + bend chip + key chip + mood pip. On drum/percussion stems the chip strip wrapped onto a second line, making the row taller than non-drum rows — every lane started at a different vertical offset, layout looked uneven. Replaced with a fixed three-element strip rendered via `ui.horizontal` (no `_wrapped`, no overflow): one instrument summary chip (`🥁 5333` or `🎸 730 ↗3`) carrying the full per-class breakdown in its tooltip, one key chip when confident, one mood pip whose tooltip carries every spectral / dynamics / rhythm numeric that used to be a separate chip. Every row is now the same height regardless of telemetry density.
- **Tooltips on every header-column control** — telemetry profile dropdown now has a hover explaining all six profile options (Auto / Universal only / Drums / Guitar / Bass / Off) plus the currently-active resolved profile. `+ Correction` / `Correction` button has hover explaining what attaching a chain does (and what happens when the seed comes from the project default vs. the Suno-Clean preset). Mood pip's tooltip got expanded from one line to a structured block with mood / timbre / dynamics / rhythm sections so glance-decoding the colour is feasible.

### Known issue — drum classifier over-firing
Flagged as a follow-up: the multi-band onset detector emits independent events per band, so a single snare hit produces concurrent events in `MID` + `HIGH_MID` + `HIGH` and gets counted as Snare + Cymbal + HiHat. Real numbers on a 3:20 Suno drum stem: ~5,300 total drum events (≈ 27/sec, physically impossible — sane rate is 3–8/sec). The universal `onset_count` is correctly deduplicated; the drum classifier needs the same cross-band time-window dedup with class arbitration via dominant flux peak. Targeted for v0.4.16. Doesn't affect tonality / non-drum analyses.

## [0.4.14] — 2026-05-08

### Added
- **Guitar / bass pick-stroke detection with YIN pitch tracking** (TBSS-FR-0005 phase 2). Each spectral-flux onset in the MID + HIGH_MID bands becomes a candidate event; YIN runs on a 50–150 ms post-onset window for sub-sample-accurate pitch; a polyphony probe counts spectral peaks above –12 dB to flag strums. Each event is classified into one of:
  - `Pluck` — single-string monophonic pick at a new pitch
  - `Repeat` — same pitch as previous (within ±50 cents, configurable) — tremolo / repeat picking
  - `Strum` — polyphonic onset; no pitch reported, single event per strum (per the design discussion's "1 event per strum" decision)
  - `Slide` — smooth pitch trajectory continuing from the previous event, between 50–200 cents in <100 ms
  - `Noise` — onset detected but velocity below the configured pick threshold OR YIN gave up cleanly
- **Pitch persisted as raw Hz** plus YIN confidence (cmnd at the chosen lag). Cents-off-pitch / detune analysis / bend density / key inference / riff fingerprinting are all free post-processing of the persisted data — no re-analysis needed when those features land.
- **Krumhansl-Schmuckler key detection** (per-track + project-level). Per-track: weighted pitch-class histogram (velocity × duration-until-next-pitched-event) → 24-key Pearson correlation against the canonical Krumhansl & Kessler 1982 templates → top key + runner-up. Project-level: union of every guitar/bass track's histogram, recomputed every time a guitar/bass result lands in `drain_telemetry_results`. Surfaced on the Project tab ("Estimated key: G♯ min") and per-track on the Mix-tab lane chips ("♪ E♭ maj").
- **User-selectable analyzer profile per track** (TBSS-FR-0005 §"Phase 2"). New `▾ Auto` / `▾ Guitar` / `▾ Bass` / `▾ Drums` / `▾ Universal only` / `▾ Off` dropdown on every Mix-tab lane header. Default is `Auto` — resolves from the track's `StemRole` (drums → drum kit, electric/acoustic guitar → guitar, bass → bass, everything else → universal-only). Explicit values override — useful when Suno mislabels a stem as `FxOther` when it's actually a percussive synth, or when a recorded take has no role at all and the user wants a guitar pitch read on it. Changing the profile clears `track.telemetry`, persists, and re-dispatches.
- **Admin → Telemetry settings…** modal with sliders for every analyzer threshold:
  - k·MAD onset threshold (default 3.0)
  - Guitar pick velocity threshold (default 0.05)
  - Bass pick velocity threshold (default 0.04)
  - YIN cumulative-mean-difference threshold (default 0.15)
  - Same-pitch tolerance in cents (default 50, controls Pluck / Repeat split)
  - Polyphony cutoff (default 5 peaks above –12 dB → Strum)
  Persisted to `%APPDATA%\TinyBooth Sound Studio\telemetry_settings.json`. Snapshotted into each `TelemetryRequest` at dispatch time so in-flight analyses use the values that were active when they were queued (mid-batch edits don't corrupt running work).
- **Project Health panel** gained two columns: `Profile` (selected → resolved, e.g. `Auto → guitar`) and `Key`. Instrument-layer column now shows `🎸N ↗N (poly NN%)` for guitar tracks alongside the existing drum-kit roll-up.
- **`ANALYZER_VERSION` bumped to 2** — old v0.4.13 telemetry is treated as stale and re-computed on next project open. Migration is invisible: the dispatcher already skips up-to-date rows.

### Why no MIDI ingest
Suno's bundle is purely audio (WAV stems + RIFF `LIST/INFO/ICMT` provenance — already parsed in `src/suno_meta.rs`). No `.mid`, no symbolic notes, no chord chart. Suno is a generative *audio* model; stem separation is post-hoc Demucs-style source separation that by construction can't recover MIDI. All pitch data has to come from our own analysis — YIN is the lever. A future `pitch_source: Analyzed | ImportedMidi` enum is reserved on the schema so user-supplied sidecar `.mid` files (from Basic Pitch / Melodyne / MT3) can plug in without a re-migration.

### Tests
- 7 new unit tests:
  - YIN recovers pure 440 Hz within 5 cents on a synthetic A4 sine
  - Polyphony probe scores chord (313 + 461 + 727 Hz, no clean common period) ≥ 0.1 higher than a pure 440 Hz sine
  - Krumhansl-Schmuckler returns root=C, mode=Major on a hand-built C-major-scale histogram (confidence > 0.7)
  - K-S returns None on an all-zero histogram (no /0)
  - `KeyEstimate::label` produces "C maj" / "A min" / "A♭ maj" for canonical roots
  - `TelemetryProfile::resolve` honours explicit values (Guitar over a Drums-roled stem) and Auto resolves correctly
  - End-to-end: synthetic 3-pitch guitar-like WAV (decaying sines) → Guitar profile → ≥2 picks detected, ≥1 pitched event recovered
- Total suite: 62 → **69 passing**

## [0.4.13] — 2026-05-08

### Added
- **Per-track audio telemetry — pure-DSP analysis baked at first save** (TBSS-FR-0005). Every imported stem and every recorded take is now analyzed in the background by a dedicated worker thread and the result is persisted on `Track.telemetry` inside the `.tinybooth` manifest. No ML, no LLM, no service calls — just rustfft + a single STFT pass per track. The first phase ships these features:
  - **Spectral character**: spectral centroid (brightness), spectral flatness (Wiener entropy — tonal vs. noisy), 85% spectral rolloff. Means and standard deviations across the track.
  - **Dynamics**: RMS dB (mean + stddev), peak dBFS, crest factor (peak / RMS).
  - **Rhythmic articulation**: spectral-flux onset detection with adaptive median + k·MAD threshold. Reports onset count, onset rate (Hz), and a sustain ratio (fraction of frames within 10 dB of the loudest moment).
  - **Mood proxies**: arousal in `[0,1]` (weighted blend of RMS, onset rate, centroid) and a phase-1 valence stub in `[-1,1]` (centroid × tonality). Surfaced as a small coloured pip whose hue tracks valence (cool blue ↔ warm yellow) and whose saturation tracks arousal.
- **Drum-kit class detection** for stems whose role is `Drums` or `Percussion` — gated on role per the design spec, so the kick/snare/hat classifiers never run on vocals or pads. Algorithm is **multi-band parallel onset detection** (Option B from the design discussion): one STFT pass, five frequency-band energy curves (`SUB 40-120Hz`, `LOW_MID 80-300`, `MID 200-800`, `HIGH_MID 1.5k-5k`, `HIGH 5k-12k`), per-band spectral-flux onset detectors fire independently. Each event gets:
  - **class** (Kick / Snare / HiHat / Tom / Cymbal / Other) decided by which bands the onset lands in plus a harmonic-content test (HNR > 15 in the 100ms post-onset window) for kick-vs-tom disambiguation,
  - **velocity** normalised flux peak,
  - **decay_ms** measured peak → 30 % energy.
- **Mix-tab telemetry chips** under each track lane's name, pulled from the manifest. Phase-1 chip vocabulary: `☀` bright, `🌙` dark, `≈` sustained, `⚡` percussive, `▦` dense. Drum stems additionally show counts: `K12 S8 h31 T2 C4`. Hover any chip for the underlying numerics. Mood pip on the right edge of the chip strip.
- **Project Health panel** (Project tab → "📊 Project Health…"). Modal showing per-track analyzer status, mood readout (arousal · valence), drum-event roll-up, and **metadata weight** in bytes (computed via JSON serialisation of each `TrackTelemetry`). Where "Infinity events vs. cap" got resolved: no event cap, but the user can see the cost and decide whether to compact in a future build. Live "Analyzing N/M…" progress while a batch is in flight, also surfaced as a chip on the bottom-bar.
- **Background telemetry worker** (`crate::telemetry::TelemetryService`) — single named OS thread, owns one `mpsc::Receiver<TelemetryRequest>` and one `mpsc::Sender<TelemetryResult>`. UI thread dispatches at every lifecycle event that produces a fresh WAV (Suno import, `stop_take`, project open, project re-open after Trim) and drains results in `update()`, patching the matching tracks and saving the manifest once per drain. Foreign-project results (e.g. Recordings analysis lands while the user has a Suno project active) get written through to the recordings manifest on disk so nothing is lost. Cost target ≈ 1-3 s per 3-minute mono stem on a modern CPU; runs at idle priority through the OS scheduler since the audio callback never sees this thread.
- **Schema version on telemetry** (`analyzer_version: u32`) so future analyzer changes can detect stale rows and re-compute on demand. The dispatcher already gates on this — tracks at the current version are skipped on every "open project" pass. Initial schema is `1`.

### Documentation
- TBSS-FR-0005 was written before this build (full RFC at `docs/feature-requests/TBSS-FR-0005-track-telemetry.md`). The implementation ships phase 1 + drum-kit detection together as decided in the design discussion. Phases 2-4 (pitch tracking, key detection, cross-band coherence, visualizer integration) remain queued.

### Tests
- 6 unit tests in `src/telemetry.rs`: silence handling without panic, pure-tone brightness detection, transient detection (≥3 of 5 synthetic pulses), arousal monotonicity, valence clamps, drum-class glyph non-emptiness. Total suite count up from 56 → 62.

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
