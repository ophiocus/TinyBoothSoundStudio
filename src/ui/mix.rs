//! Mix tab — multitrack lanes (top) + console deck (bottom).
//!
//! The bottom half of the tab is a hardware-style console: vertical
//! fader strips per track plus a master strip on the right. Each strip
//! has its own mute / solo / arm-automation toggles, vertical fader,
//! peak meter, and dB readout.
//!
//! Automation: when a strip's `R` (arm) toggle is on and playback is
//! Playing, the UI thread samples the live `gain_db` once per frame
//! and feeds it to the project-wide [`Recorder`]. On Stop / disarm
//! the scratch lane is committed to the matching `Track.gain_automation`
//! (or `Project.master_gain_automation`) and a fresh `SplineSampler`
//! is shipped to the audio thread, which replays it via Catmull-Rom
//! interpolation on the next playback.

use crate::app::TinyBoothApp;
use crate::player::{PlayState, Player};
use eframe::egui;
use egui::{Color32, Pos2, Rect, Stroke};
use std::sync::atomic::Ordering;

const HEADER_W: f32 = 240.0;
/// Lane height in px. v0.4.20 bumped 52 → 62 — v0.4.18's tight 52
/// was a hair too short to fit the 2-row header cleanly, leaving
/// rows visually bleeding into each other. 62 = 24 (top row, name +
/// chips) + 22 (button row, M·S·A/B·Cor + dropdown) + 16 of vertical
/// padding — comfortable margin so the row divider sits cleanly
/// between lanes.
const LANE_H: f32 = 62.0;
/// Vertical gap between lanes. v0.4.20 bumped 4 → 8 + a row divider
/// to make the boundaries visually unambiguous (was: "headers
/// bleed top and bottom" in v0.4.19's screenshot).
const ROW_GAP: f32 = 8.0;
/// Height of the optional Mix-tab spectrum panel — capped at 80px
/// regardless of `LANE_H` so the spectrum stays a useful size when
/// lanes shrink for compactness.
const SPECTRUM_H: f32 = 80.0;

/// Fixed strip card width. Tight by design — the vertical label
/// gutter on the left (v0.4.22) saves the horizontal space the
/// centred top label used to consume, so the card stays narrow.
const STRIP_W: f32 = 108.0;
/// Width of the rotated-label column at the left of each strip
/// card. v0.4.22 — replaces the horizontal-centred name label that
/// used to sit above the M/S/R/Ø row; freeing a row of vertical
/// space means the fader sees that height instead.
const STRIP_LABEL_COL_W: f32 = 18.0;
const STRIP_GAP: f32 = 4.0;
const FADER_H: f32 = 130.0;
/// Hard cap on fader rail height. v0.4.19 made the rail stretch into
/// the console-deck's full available height — which on tall windows
/// ballooned each strip card to 400+ px and made the deck "gigantic"
/// (per user screenshot). The fix is to stretch up to a sensible
/// maximum and stop. 200 px ≈ enough rail for fine-grained gain
/// control without dominating the screen.
const FADER_H_MAX: f32 = 200.0;
/// Hard cap on console-deck height. Same motivation as
/// `FADER_H_MAX` — on tall windows the `mix_console_fraction`
/// (0.2..0.7) put 400+ px in the deck, leaving the user with one
/// strip occupying half their screen. 340 px ≈ spectrum panel
/// (80) + strip natural height (~230) + a touch of margin.
const CONSOLE_H_MAX: f32 = 340.0;
/// Fixed height for the transport bar region (v0.4.30). The
/// previous nested-panel layout let it claim its natural size, but
/// when content overflowed (long status, error banner) it'd push
/// everything below it. With a fixed height the lanes / console
/// regions don't shift; the transport itself self-clips if its
/// content is taller (rare — only the error banner can grow).
const TRANSPORT_BAR_H: f32 = 56.0;
const METER_W: f32 = 6.0;
/// Cap on track-name characters before we ellipsise. Tuned so Latin-script
/// names like "Backing Vocals" / "Electric Guitar" / "Synth / Lead" fit
/// inside `STRIP_W` without truncation at 1.0× zoom.
const STRIP_NAME_CHARS: usize = 14;
/// Strip-name font size in pt at 1.0× zoom. Egui's `set_zoom_factor`
/// scales these proportionally — bump zoom from the View menu.
const FONT_STRIP_NAME: f32 = 13.0;
const FONT_STRIP_DB: f32 = 12.0;
const FONT_MASTER_NAME: f32 = 14.0;

/// Mix-tab entry point.
///
/// **Architecture (v0.4.30 — explicit clipped child_ui layout):**
///
/// The Mix tab is split into three vertically-stacked regions
/// rendered as `child_ui`s with **explicitly-set `clip_rect`s**. Pre-
/// v0.4.30 we used egui's nested `TopBottomPanel::show_inside` +
/// `CentralPanel::show_inside`, but that combination misbehaves when
/// the outer surface is itself an app-level `CentralPanel::show(ctx,
/// ...)`: lane rows would render *above* their host area, visible as
/// the first lane spilling up into the global menu bar.
///
/// The new layout is explicit and predictable:
///
/// ```text
///   ┌─ transport_rect  (natural height, ~50 px) ──────────────────┐
///   │  transport bar  +  optional error banner                     │
///   ├─ lanes_rect (fills the middle, clip_rect set) ──────────────┤
///   │  lanes_view (vertical ScrollArea, only scrollable surface)   │
///   ├─ console_rect (exact_height = CONSOLE_H, clip_rect set) ────┤
///   │  spectrum panel + strip cards (horizontal scroll only)       │
///   └──────────────────────────────────────────────────────────────┘
/// ```
///
/// Each region:
///   • Has its rect computed up-front from `ui.max_rect()`, not
///     incrementally from the cursor — so a 1-px wobble in any
///     surface's content can't shift the others by a px each frame.
///   • Gets a child `Ui` with its `clip_rect` set to its own rect,
///     hard-bounding all drawing. Content overflow is physically
///     impossible.
///   • Owns its own scroll-event hit-testing because the child Ui's
///     interact_rect is its clip_rect — wheel events outside a
///     scroll area's rect are ignored.
pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    if app.project.tracks.is_empty() {
        ui.heading("Mix");
        ui.separator();
        ui.label("Record at least one track or import a Suno bundle to mix.");
        return;
    }

    rebuild_player_if_needed(app);
    consume_autoplay_request(app);
    capture_automation(app);

    // Total area available to the Mix tab — taken from the host
    // CentralPanel's max_rect, NOT from cursor-based available_height
    // which can wobble with sibling widgets.
    let outer = ui.max_rect();
    let outer_w = outer.width();
    let total_h = outer.height().max(200.0);

    // Heights: transport claims its natural needs (fixed estimate);
    // console claims a fraction (clamped); lanes get whatever's left.
    let transport_h = TRANSPORT_BAR_H;
    let console_h =
        (total_h * app.mix_console_fraction.clamp(0.2, 0.7)).clamp(180.0, CONSOLE_H_MAX);
    let lanes_h = (total_h - transport_h - console_h).max(120.0);

    let top_y = outer.min.y;
    let transport_rect = Rect::from_min_size(
        Pos2::new(outer.min.x, top_y),
        egui::vec2(outer_w, transport_h),
    );
    let lanes_rect = Rect::from_min_size(
        Pos2::new(outer.min.x, top_y + transport_h),
        egui::vec2(outer_w, lanes_h),
    );
    let console_rect = Rect::from_min_size(
        Pos2::new(outer.min.x, top_y + transport_h + lanes_h),
        egui::vec2(outer_w, console_h),
    );

    // ── Region 1: transport ───────────────────────────────────────
    render_clipped(ui, transport_rect, "mix_transport", |ui| {
        transport_bar(app, ui);
        render_player_error_banner_if_present(app, ui);
    });

    // Early-return path: error banner up, no player. Transport
    // already drew the Retry button above.
    if app.player.is_none() {
        return;
    }

    // ── Region 2: lanes (the only vertical-scroll surface) ───────
    render_clipped(ui, lanes_rect, "mix_lanes", |ui| {
        lanes_view(app, ui);
    });

    // ── Region 3: console deck (horizontal-scroll only) ──────────
    render_clipped(ui, console_rect, "mix_console", |ui| {
        console_deck(app, ui);
    });

    // Tell the parent ui we consumed the whole outer rect so it
    // doesn't think the cursor is back at the top — keeps any
    // sibling layout (none today, but defensively) honest.
    ui.allocate_rect(outer, egui::Sense::hover());
}

/// Build a `child_ui` clamped to `rect`, with `clip_rect = rect` set
/// so any content drawn beyond the boundary is physically hard-
/// clipped. v0.4.30 — replaces the previous nested-`show_inside`
/// approach which leaked the first lane row above the host area
/// when egui's CentralPanel-inside-CentralPanel interaction
/// misfired.
fn render_clipped<R>(
    parent: &mut egui::Ui,
    rect: Rect,
    id_source: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let mut child = parent.child_ui_with_id_source(
        rect,
        egui::Layout::top_down(egui::Align::Min),
        id_source,
        None,
    );
    child.set_clip_rect(rect);
    // `set_max_size` so any ScrollArea inside knows its viewport.
    child.set_max_size(rect.size());
    add_contents(&mut child)
}

/// Lazy-rebuild the player when needed (project changed shape OR
/// player is None and the last attempt didn't fail for this project).
/// Extracted out of `show()` for readability — same logic as v0.4.27
/// for output-device-aware Player::new.
fn rebuild_player_if_needed(app: &mut TinyBoothApp) {
    let attempt_already_failed = app
        .player_attempt_failed_for
        .as_ref()
        .map(|p| p == &app.project.root)
        .unwrap_or(false);
    let need_rebuild = match app.player.as_ref() {
        None => !attempt_already_failed,
        Some(p) => p.project_track_count != app.project.tracks.len(),
    };
    if !need_rebuild {
        return;
    }
    app.player = None;
    app.player_error = None;
    match Player::new(
        &app.project,
        app.audio_err_tx.clone(),
        app.config.output_device.as_deref(),
    ) {
        Ok(p) => {
            app.player = Some(p);
            app.player_attempt_failed_for = None;
        }
        Err(e) => {
            app.player_error = Some(format!("{e:#}"));
            app.player_attempt_failed_for = Some(app.project.root.clone());
        }
    }
}

/// Player-load error banner — rendered inside the top panel so it
/// shares the transport bar's clip rect. Retry button clears the
/// failed-attempt cache so the next frame retries `Player::new`.
fn render_player_error_banner_if_present(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    let Some(err) = app.player_error.clone() else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(Color32::LIGHT_RED, &err);
        if app.player.is_none()
            && app.player_attempt_failed_for.is_some()
            && ui
                .button("↻ Retry")
                .on_hover_text(
                    "Try to rebuild the player — useful after plugging in audio hardware.",
                )
                .clicked()
        {
            app.player_attempt_failed_for = None;
            app.player_error = None;
        }
    });
}

/// Consume any pending auto-play request from the Record-tab "▶"
/// buttons. Solo'd the chosen take, positioned to 0, started playback.
fn consume_autoplay_request(app: &mut TinyBoothApp) {
    if !app.mix_autoplay_pending {
        return;
    }
    if let Some(player) = app.player.as_ref() {
        if let Some(idx) = app.mix_autoplay_solo_idx.take() {
            for (i, t) in player.state.tracks.iter().enumerate() {
                t.solo.store(i == idx, std::sync::atomic::Ordering::Relaxed);
            }
        }
        player
            .state
            .position_frames
            .store(0, std::sync::atomic::Ordering::Release);
        player.state.set_play_state(PlayState::Playing);
    }
    app.mix_autoplay_pending = false;
}

// ───────────────────── transport ─────────────────────

fn transport_bar(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    // Snapshot the booleans the transport bar still cares about.
    // v0.4.22 — the readings (pos / sample-rate / LUFS) moved to
    // the top bar as a right-hand aside next to the project name,
    // so this bar is now a pure controls strip: Play / Stop +
    // Enable / Disable / Reset / A/B.
    let (have_player, playing) = if let Some(p) = app.player.as_ref() {
        (true, p.state.play_state() == PlayState::Playing)
    } else {
        (false, false)
    };

    // How many tracks already carry a correction chain — drives the
    // bulk-action buttons' enabled state and labels.
    let n_tracks = app.project.tracks.len();
    let n_with_corr = app
        .project
        .tracks
        .iter()
        .filter(|t| t.correction.is_some())
        .count();
    let n_without_corr = n_tracks.saturating_sub(n_with_corr);

    // Ephemeral global bypass — atomic on PlayerState, set by either
    // the A/B button (transient) or the persisted Disable toggle.
    let global_bypass_on = match app.player.as_ref() {
        Some(p) => p
            .state
            .global_bypass
            .load(std::sync::atomic::Ordering::Relaxed),
        None => false,
    };
    // Persisted project flag — separate from the atomic so the user can
    // toggle live A/B without dirtying the project, and toggle the
    // persisted flag without leaving the project in a "live A/B" state
    // on reload.
    let corrections_disabled = app.project.corrections_disabled;

    let mut click_play = false;
    let mut click_pause = false;
    let mut click_stop = false;
    let mut click_enable_all = false;
    let mut click_disable_persisted = false;
    let mut click_reset_all = false;
    let mut click_toggle_bypass = false;

    ui.horizontal(|ui| {
        ui.heading("Mix");
        ui.separator();
        ui.add_enabled_ui(have_player, |ui| {
            if !playing {
                if ui.add(egui::Button::new("▶  Play").min_size(egui::vec2(80.0, 30.0))).clicked() {
                    click_play = true;
                }
            } else if ui.add(egui::Button::new("⏸  Pause").min_size(egui::vec2(80.0, 30.0))).clicked() {
                click_pause = true;
            }
            if ui.add(egui::Button::new("⏹  Stop").min_size(egui::vec2(80.0, 30.0))).clicked() {
                click_stop = true;
            }
        });
        ui.separator();

        // Bulk correction toggles. "Enable all" seeds Suno-Clean on
        // every track currently at correction = None; doesn't overwrite
        // tracks the user has already tweaked.
        ui.add_enabled_ui(n_tracks > 0 && n_without_corr > 0, |ui| {
            let label = if n_without_corr == n_tracks {
                "✓ Enable all corrections".to_string()
            } else {
                format!("✓ Enable corrections on {n_without_corr}/{n_tracks}")
            };
            if ui.add(egui::Button::new(label).min_size(egui::vec2(160.0, 28.0)))
                .on_hover_text("Apply Suno-Clean to every track without an existing correction chain. Doesn't overwrite tracks you've already edited.")
                .clicked()
            {
                click_enable_all = true;
            }
        });
        // Persisted Disable — flips Project.corrections_disabled and
        // syncs the player's global_bypass. Survives reload.
        ui.add_enabled_ui(n_tracks > 0, |ui| {
            let label = if corrections_disabled {
                "⊘ Disabled (saved)"
            } else {
                "⊘ Disable (saves)"
            };
            if ui.add(egui::SelectableLabel::new(corrections_disabled, label))
                .on_hover_text("Persisted project-wide bypass. Saves to the manifest — corrections stay off across reloads. Toggle again to re-enable. Non-destructive: chain config is preserved.")
                .clicked()
            {
                click_disable_persisted = true;
            }
        });
        ui.add_enabled_ui(n_with_corr > 0, |ui| {
            let label = if n_with_corr == n_tracks {
                "⟲ Reset all".to_string()
            } else {
                format!("⟲ Reset {n_with_corr}/{n_tracks}")
            };
            if ui.add(egui::Button::new(label).min_size(egui::vec2(110.0, 28.0)))
                .on_hover_text("Destructive — strips every correction chain. Tweaks lost. Re-enable to re-seed from the cascade (project default → feature default).")
                .clicked()
            {
                click_reset_all = true;
            }
        });
        ui.separator();
        // Ephemeral A/B — flips global_bypass without touching the
        // project flag. Useful for live listening; reload restores the
        // persisted state.
        ui.add_enabled_ui(n_with_corr > 0 || corrections_disabled, |ui| {
            let label = if global_bypass_on { "A/B  ▣  bypassed" } else { "A/B  ☐  live" };
            if ui.add(egui::SelectableLabel::new(global_bypass_on, label))
                .on_hover_text("Ephemeral global A/B — flips the audio thread's bypass without touching the project flag. Doesn't dirty the project; reload restores whatever the persisted Disable was.")
                .clicked()
            {
                click_toggle_bypass = true;
            }
        });
    });

    if click_play {
        if let Some(p) = app.player.as_ref() {
            p.play();
        }
    }
    if click_pause {
        if let Some(p) = app.player.as_ref() {
            p.pause();
        }
    }
    if click_stop {
        stop_and_commit_automation(app);
    }
    if click_enable_all {
        app.enable_all_corrections();
    }
    if click_disable_persisted {
        app.toggle_corrections_disabled();
    }
    if click_reset_all {
        app.reset_all_corrections();
    }
    if click_toggle_bypass {
        app.toggle_global_bypass();
    }
}

// ───────────────────── multitrack lane view ─────────────────────

fn lanes_view(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    let Some(player) = app.player.as_ref() else {
        return;
    };
    let dur = player.state.duration_secs().max(0.001);
    let pos = player.state.position_secs();

    let mut requested_correction: Option<usize> = None;
    let mut requested_profile_change: Option<(usize, crate::telemetry::TelemetryProfile)> = None;

    // v0.4.29 — explicit single-axis scroll. `auto_shrink([false; 2])`
    // makes the area fill the CentralPanel's full extent so the lane
    // rows always render against a stable rect, never the natural
    // size of the content. Combined with the panel's clip rect, this
    // guarantees the rows never bleed past the lane area.
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            // v0.4.24 — small top padding so the first row's name label
            // doesn't kiss the bottom edge of the transport bar above.
            ui.add_space(4.0);
            for (idx, track) in player.state.tracks.iter().enumerate() {
                // v0.4.24 — wrap each lane in `Frame::group` so each row
                // gets a visibly bounded card with its own border. Pre-
                // v0.4.24 had only a 1-px divider line that wasn't strong
                // enough to break the eye-fuse between adjacent rows
                // (the "header overlap" issue in the user screenshot).
                egui::Frame::group(ui.style())
                    .fill(egui::Color32::from_rgb(14, 14, 18))
                    .inner_margin(egui::Margin::symmetric(4.0, 2.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // ── Header column ─────────────────────────────────
                            // `allocate_exact_size` (not `allocate_ui_with_layout`)
                            // is critical: the latter is a *suggested* size — if
                            // the inner content's natural width exceeds HEADER_W
                            // (e.g. a wide chip strip), it grows the box and
                            // pushes the next allocation (the lane) right. That
                            // made every row's waveform start at a slightly
                            // different X — the "tracks aren't trimmed to the
                            // same start" bug visible in v0.4.15. `allocate_exact_size`
                            // reserves precisely HEADER_W × LANE_H and any inner
                            // overflow gets clipped, so every lane shares the
                            // same X. v0.4.16.
                            let (header_rect, _) = ui.allocate_exact_size(
                                egui::vec2(HEADER_W, LANE_H),
                                egui::Sense::hover(),
                            );
                            let mut hui = ui.child_ui(
                                header_rect,
                                egui::Layout::top_down(egui::Align::Min),
                                None,
                            );
                            hui.set_clip_rect(header_rect);
                            {
                                let ui = &mut hui;
                                // Two-row compact layout (v0.4.18):
                                //   Row 1 — track name + telemetry chips
                                //   Row 2 — M / S / A/B / Cor + profile dropdown
                                // Drops a row vs v0.4.13–17 (was name / chips / buttons),
                                // so LANE_H fits in 52 px instead of 72 — ~28% more
                                // lanes visible per screen height.
                                ui.add_space(1.0);

                                // Row 1: name (strong, leftmost) + telemetry chips.
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(&track.name).strong());
                                    if let Some(t) = app.project.tracks.get(idx) {
                                        telemetry_chips(ui, t);
                                    }
                                });

                                // Row 2: control cluster (M / S / A/B / Cor) +
                                // profile dropdown to the right of the buttons.
                                ui.horizontal(|ui| {
                                    // Per-channel mute (v0.4.16) — mirrors the
                                    // console-deck strip's mute toggle. The atomic
                                    // is shared, so flipping here is reflected on
                                    // the strip + audio thread immediately.
                                    let mute = track.mute.load(Ordering::Relaxed);
                                    if ui
                                        .add_sized(
                                            [20.0, 18.0],
                                            egui::SelectableLabel::new(mute, "M"),
                                        )
                                        .on_hover_text(if mute {
                                            "Muted — click to unmute"
                                        } else {
                                            "Mute this track"
                                        })
                                        .clicked()
                                    {
                                        track.mute.store(!mute, Ordering::Relaxed);
                                    }
                                    // Per-channel solo (v0.4.16).
                                    let solo = track.solo.load(Ordering::Relaxed);
                                    if ui
                                        .add_sized(
                                            [20.0, 18.0],
                                            egui::SelectableLabel::new(solo, "S"),
                                        )
                                        .on_hover_text(if solo {
                                            "Solo'd — click to clear"
                                        } else {
                                            "Solo this track (silences others)"
                                        })
                                        .clicked()
                                    {
                                        track.solo.store(!solo, Ordering::Relaxed);
                                    }
                                    // v0.4.7 perf: atomic-bool mirror avoids a
                                    // Mutex+clone of the whole Profile every frame.
                                    let mut bypass =
                                        track.bypass_correction.load(Ordering::Relaxed);
                                    let has_corr = track.has_correction();
                                    ui.add_enabled_ui(has_corr, |ui| {
                                        if ui
                                            .add_sized(
                                                [26.0, 18.0],
                                                egui::SelectableLabel::new(bypass, "A/B"),
                                            )
                                            .on_hover_text(if bypass {
                                                "Bypassed (original)"
                                            } else {
                                                "Correction active"
                                            })
                                            .clicked()
                                        {
                                            bypass = !bypass;
                                            track
                                                .bypass_correction
                                                .store(bypass, Ordering::Relaxed);
                                        }
                                    });
                                    // Compact label "Cor" / "+Cor" fits in the
                                    // 240-px header alongside M/S/A/B + dropdown.
                                    // Full hover-text preserves the explanation.
                                    let label = if has_corr { "Cor" } else { "+Cor" };
                                    let corr_tip = if has_corr {
                                        "Open the per-track correction chain editor \
                             (HPF / EQ / de-esser / gate / compressor / makeup) \
                             — applied at playback and export."
                                    } else {
                                        "Attach a correction chain to this track. Seeded from \
                             the project default if set, else from the Suno-Clean \
                             preset. Edit at any time; takes effect on next playback."
                                    };
                                    if ui
                                        .add_sized([34.0, 18.0], egui::Button::new(label))
                                        .on_hover_text(corr_tip)
                                        .clicked()
                                    {
                                        requested_correction = Some(idx);
                                    }

                                    // Profile dropdown — pushed to the right of
                                    // the button cluster so the row reads as
                                    // "controls (left) → analyzer (right)".
                                    if let Some(t) = app.project.tracks.get(idx) {
                                        let cur = t.telemetry_profile;
                                        let resolved = cur.resolve(&t.source);
                                        let label = format!("▾ {}", cur.label());
                                        let tip = format!(
                                            "Telemetry analyzer profile.\n\
                                 Currently: {} (running as: {})\n\
                                 \n\
                                 • Auto — infer from track role (drums → drum kit, \
                                 guitar/bass → pitch tracker, else universal-only).\n\
                                 • Universal only — basic features only.\n\
                                 • Drums — kick / snare / hat / tom / cymbal.\n\
                                 • Guitar — pick detection + YIN pitch + key.\n\
                                 • Bass — same, biased toward low strings.\n\
                                 • Off — skip analysis for this track.\n\
                                 \n\
                                 Changing this re-runs the analyzer.",
                                            cur.label(),
                                            resolved_short(resolved),
                                        );
                                        egui::ComboBox::from_id_source(("tel_prof", idx))
                                            .selected_text(
                                                egui::RichText::new(label)
                                                    .size(11.0)
                                                    .color(egui::Color32::from_gray(160)),
                                            )
                                            .width(88.0)
                                            .show_ui(ui, |ui| {
                                                let mut sel = cur;
                                                for &p in crate::telemetry::TelemetryProfile::all()
                                                {
                                                    ui.selectable_value(&mut sel, p, p.label());
                                                }
                                                if sel != cur {
                                                    // Defer — `player` borrow blocks
                                                    // mutation of app.project here.
                                                    requested_profile_change = Some((idx, sel));
                                                }
                                            })
                                            .response
                                            .on_hover_text(tip);
                                    }
                                });
                            }

                            // ── Lane (waveform) ───────────────────────────────
                            let avail = ui.available_size().x.max(200.0);
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(avail, LANE_H),
                                egui::Sense::hover(),
                            );
                            // v0.4.7 perf: was `track.automation().as_ref()` — that
                            // cloned the Vec<AutomationPoint> only to take a reference
                            // to the clone for the draw call. Borrow via callback so
                            // the lock is held briefly during draw_lane (microseconds)
                            // with no allocation.
                            track.with_automation(|auto| {
                                draw_lane(
                                    ui,
                                    rect,
                                    &track.peaks,
                                    dur,
                                    pos,
                                    track.frame_count,
                                    track.sample_rate,
                                    auto,
                                );
                            });
                        });
                    }); // close Frame::group inner closure
                        // v0.4.24 — Frame::group above gives each row its own
                        // bordered card, so the previous 1-px line divider is
                        // gone. A small gap below keeps cards from touching.
                ui.add_space(ROW_GAP);
            }
        });

    if let Some(i) = requested_correction {
        if app.project.tracks[i].correction.is_none() {
            let seed = app
                .profiles
                .iter()
                .find(|p| p.name == "Suno-Clean")
                .or_else(|| app.profiles.first())
                .cloned();
            app.project.tracks[i].correction = seed.clone();
            app.project_dirty = true;
            if let Some(player) = app.player.as_ref() {
                if let Some(track) = player.state.tracks.get(i) {
                    track.set_correction(seed);
                }
            }
        }
        app.editing_correction_for = Some(i);
    }

    // Telemetry profile change — apply the staged choice (deferred
    // out of the lane closure so the `player` borrow drops first),
    // then re-dispatch the analyzer for that track.
    if let Some((idx, new_profile)) = requested_profile_change {
        if let Some(track) = app.project.tracks.get_mut(idx) {
            track.telemetry_profile = new_profile;
            app.project_dirty = true;
        }
        app.invalidate_telemetry_for_track(idx);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_lane(
    ui: &mut egui::Ui,
    rect: Rect,
    peaks: &[f32],
    total_secs: f32,
    pos_secs: f32,
    track_frames: u64,
    sample_rate: u32,
    automation: Option<&crate::automation::AutomationLane>,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(10, 10, 14));
    if peaks.is_empty() {
        return;
    }

    let track_secs = track_frames as f32 / sample_rate.max(1) as f32;
    let track_w = rect.width() * (track_secs / total_secs).min(1.0);
    let track_rect = Rect::from_min_size(rect.min, egui::vec2(track_w, rect.height()));

    let mid_y = rect.center().y;
    let gain = rect.height() * 0.45;
    let stroke = Stroke::new(1.0, Color32::from_rgb(100, 220, 150));
    let cols = track_rect.width() as usize;
    if cols > 0 {
        for x_px in 0..cols {
            let bin_idx = (x_px as f32 / cols.max(1) as f32 * peaks.len() as f32) as usize;
            let bin_idx = bin_idx.min(peaks.len() - 1);
            let p = peaks[bin_idx];
            let h = p * gain;
            let x = track_rect.min.x + x_px as f32;
            painter.line_segment([Pos2::new(x, mid_y - h), Pos2::new(x, mid_y + h)], stroke);
        }
    }
    painter.line_segment(
        [Pos2::new(rect.min.x, mid_y), Pos2::new(rect.max.x, mid_y)],
        Stroke::new(0.5, Color32::from_gray(40)),
    );

    // Automation curve (drawn semi-transparent under the playhead).
    if let Some(lane) = automation {
        if !lane.points.is_empty() {
            let auto_color = Color32::from_rgba_unmultiplied(230, 200, 80, 180);
            let cols = rect.width() as usize;
            let sampler = crate::automation::SplineSampler::build(lane);
            // Map dB → y: 0 dB at midline, +6 at top, -60 at bottom.
            let db_to_y = |db: f32| -> f32 {
                let n = ((db + 60.0) / 66.0).clamp(0.0, 1.0); // 0..1 from -60 to +6
                rect.max.y - n * rect.height()
            };
            let mut prev: Option<Pos2> = None;
            for x_px in 0..cols {
                let t = x_px as f32 / cols.max(1) as f32 * total_secs;
                if let Some(db) = sampler.sample(t) {
                    let p = Pos2::new(rect.min.x + x_px as f32, db_to_y(db));
                    if let Some(pv) = prev {
                        painter.line_segment([pv, p], Stroke::new(1.5, auto_color));
                    }
                    prev = Some(p);
                } else {
                    prev = None;
                }
            }
        }
    }

    // Synchronized playhead.
    let head_x = rect.min.x + rect.width() * (pos_secs / total_secs).clamp(0.0, 1.0);
    painter.line_segment(
        [Pos2::new(head_x, rect.min.y), Pos2::new(head_x, rect.max.y)],
        Stroke::new(1.5, Color32::from_rgb(230, 200, 80)),
    );
}

// ───────────────────── console deck ─────────────────────

fn console_deck(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    let n_tracks = match app.player.as_ref() {
        Some(p) => p.state.tracks.len(),
        None => return,
    };
    let mut commit_track: Option<usize> = None;
    let mut commit_master = false;

    // v0.4.19: spectrum panel relocated to the top of the console
    // deck (was: top of the Mix tab). Pinned at the bottom of the
    // screen so the meter ↔ spectrum comparison happens in one
    // glance. Compute it now so the strip area below can use the
    // remaining height — that gives the fader rail room to stretch
    // into what used to be wasted bottom space inside each card.
    let mut strip_h = ui.available_height().max(160.0);
    if app.config.show_spectrum_panel {
        crate::ui::spectrum_panel::show(app, ui, SPECTRUM_H);
        ui.add_space(2.0);
        strip_h = (strip_h - SPECTRUM_H - 2.0).max(140.0);
    }

    // v0.4.29 — explicit single-axis scroll. `hscroll(true)` +
    // `vscroll(false)` makes the wheel passthrough — vertical wheel
    // events inside the console deck don't shift the strips. That's
    // why pre-v0.4.29 "the cards jittered in place when I scroll":
    // egui was scrolling a 0-height vertical extent because
    // ScrollArea::horizontal silently still accepted wheel input on
    // the cross axis. `auto_shrink` set so the bar always claims the
    // panel's full extent — strip rows don't drift around as the
    // user adds / removes tracks.
    egui::ScrollArea::new([true, false])
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            // v0.4.24 — explicit `Align::Min` on the cross axis (top
            // alignment for left-to-right). Plain `ui.horizontal` uses
            // `Align::Center`, which staircase-shifted cards down as soon
            // as any two cards had even slightly different effective
            // heights. With Align::Min every card's top edge sits on the
            // same y baseline.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                for idx in 0..n_tracks {
                    if strip(app, ui, idx, strip_h) {
                        commit_track = Some(idx);
                    }
                    ui.add_space(STRIP_GAP);
                }
                ui.add_space(STRIP_GAP * 2.0);
                if master_strip(app, ui, strip_h) {
                    commit_master = true;
                }
            });
        });

    if let Some(i) = commit_track {
        commit_track_automation(app, i);
    }
    if commit_master {
        commit_master_automation(app);
    }
}

/// Returns true if the strip's R toggle was just turned OFF (caller
/// should commit the recorder's scratch lane for this track).
///
/// `available_h` is the height of the console-deck region we should
/// fill (v0.4.19). The fader rail stretches into whatever's left
/// after the labels / button rows / dB readout claim their share —
/// no more wasted vertical space inside the card.
fn strip(app: &mut TinyBoothApp, ui: &mut egui::Ui, idx: usize, available_h: f32) -> bool {
    // Clone the Arc so we can drop the immutable borrow on app before
    // any mutation. Cheap — Arc clone is two atomic ops.
    let track = match app.player.as_ref() {
        Some(p) => match p.state.tracks.get(idx) {
            Some(t) => t.clone(),
            None => return false,
        },
        None => return false,
    };

    let mut frame_color = Color32::from_rgb(22, 22, 26);
    if track.recording_armed.load(Ordering::Relaxed) {
        frame_color = Color32::from_rgb(70, 30, 30);
    } else if track.solo.load(Ordering::Relaxed) {
        frame_color = Color32::from_rgb(60, 50, 20);
    }

    let mut just_disarmed = false;
    egui::Frame::group(ui.style())
        .fill(frame_color)
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            ui.set_width(STRIP_W);
            // Hard cap the rail (was: stretch unbounded into
            // available_h, which on tall windows produced 400+ px
            // rails). Floors at FADER_H so a short deck still gets
            // a usable fader; ceilings at FADER_H_MAX so the card
            // stays compact. v0.4.20–v0.4.22.
            let fader_h = (available_h - 60.0).clamp(FADER_H, FADER_H_MAX);
            ui.style_mut().spacing.slider_width = fader_h;

            ui.horizontal(|ui| {
                // ── Rotated label gutter (v0.4.22) ──
                // Replaces the horizontal centred name label at the
                // top of the card. Uses a Painter + TextShape with
                // angle = π/2 (top-to-bottom reading, classic console
                // style) so we save a row of vertical space and
                // claim a narrow horizontal gutter instead.
                let inner_h = fader_h + 60.0;
                let (label_rect, _) = ui.allocate_exact_size(
                    egui::vec2(STRIP_LABEL_COL_W, inner_h),
                    egui::Sense::hover(),
                );
                draw_rotated_label(
                    ui,
                    label_rect,
                    &ellipsize(&track.name, STRIP_NAME_CHARS),
                    FONT_STRIP_NAME,
                    Color32::from_rgb(220, 230, 220),
                );

                // ── Right column: buttons + fader + meter + dB ──
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let mute = track.mute.load(Ordering::Relaxed);
                        if ui
                            .add_sized([18.0, 20.0], egui::SelectableLabel::new(mute, "M"))
                            .on_hover_text("Mute")
                            .clicked()
                        {
                            track.mute.store(!mute, Ordering::Relaxed);
                        }
                        let solo = track.solo.load(Ordering::Relaxed);
                        if ui
                            .add_sized([18.0, 20.0], egui::SelectableLabel::new(solo, "S"))
                            .on_hover_text("Solo")
                            .clicked()
                        {
                            track.solo.store(!solo, Ordering::Relaxed);
                        }
                        let armed = track.recording_armed.load(Ordering::Relaxed);
                        if ui
                            .add_sized([18.0, 20.0], egui::SelectableLabel::new(armed, "R"))
                            .on_hover_text("Arm — record fader gestures during playback")
                            .clicked()
                        {
                            let new_armed = !armed;
                            track.recording_armed.store(new_armed, Ordering::Relaxed);
                            if !new_armed {
                                just_disarmed = true;
                            }
                        }
                        let polarity = track.polarity_inverted.load(Ordering::Relaxed);
                        if ui
                            .add_sized([18.0, 20.0], egui::SelectableLabel::new(polarity, "Ø"))
                            .on_hover_text(
                                "Polarity flip (phase invert) — multiplies \
                                 samples by −1. Use when this stem appears \
                                 anti-phase relative to others.",
                            )
                            .clicked()
                        {
                            let new_polarity = !polarity;
                            track
                                .polarity_inverted
                                .store(new_polarity, Ordering::Relaxed);
                            if let Some(t) = app.project.tracks.get_mut(idx) {
                                t.polarity_inverted = new_polarity;
                            }
                            app.project_dirty = true;
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let mut gain = track.gain_db();
                        let resp = ui.add_sized(
                            [STRIP_W - STRIP_LABEL_COL_W - 30.0, fader_h],
                            egui::Slider::new(&mut gain, -60.0..=6.0)
                                .vertical()
                                .show_value(false),
                        );
                        if resp.changed() {
                            track.set_gain_db(gain);
                        }
                        draw_meter(ui, track.peak(), fader_h);
                    });
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(format!("{:+.1} dB", track.gain_db()))
                            .size(FONT_STRIP_DB)
                            .monospace(),
                    );
                });
            });
        });
    let _ = app; // keep argument used for future expansion
    just_disarmed
}

/// Render a label rotated 90° CW (top-to-bottom reading) centred
/// inside `rect`. Used by the v0.4.22 strip-card layout — replaces
/// the previous horizontal-centred name label that ate a full row
/// of vertical space at the top of every card.
fn draw_rotated_label(ui: &mut egui::Ui, rect: Rect, text: &str, size_pt: f32, color: Color32) {
    let painter = ui.painter_at(rect);
    let galley =
        painter.layout_no_wrap(text.to_string(), egui::FontId::proportional(size_pt), color);
    let text_size = galley.size();
    // For TextShape::angle = +π/2 (CW rotation around `pos`):
    //   the unrotated rect [0,w]×[0,h] anchored at pos becomes
    //   the rotated rect [pos.x − h, pos.x] × [pos.y, pos.y + w].
    // To centre the rotated rect inside `rect`:
    //   pos.x = rect.center().x + h/2
    //   pos.y = rect.center().y − w/2
    let pos = Pos2::new(
        rect.center().x + text_size.y * 0.5,
        rect.center().y - text_size.x * 0.5,
    );
    let mut shape = egui::epaint::TextShape::new(pos, galley, color);
    shape.angle = std::f32::consts::FRAC_PI_2;
    painter.add(shape);
}

/// Returns true if the master strip's R toggle was just turned OFF.
///
/// `available_h` follows the same shape as `strip()` — fader rail
/// stretches into the console-deck height instead of staying fixed.
fn master_strip(app: &mut TinyBoothApp, ui: &mut egui::Ui, available_h: f32) -> bool {
    // Clone the Arc<PlayerState> so we drop the immutable borrow on app
    // before any project-level mutation.
    let state = match app.player.as_ref() {
        Some(p) => p.state.clone(),
        None => return false,
    };

    let mut frame_color = Color32::from_rgb(28, 28, 36);
    if state.master_recording_armed.load(Ordering::Relaxed) {
        frame_color = Color32::from_rgb(80, 30, 30);
    }

    let mut just_disarmed = false;
    let mut new_master_db: Option<f32> = None;
    egui::Frame::group(ui.style())
        .fill(frame_color)
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            ui.set_width(STRIP_W + 16.0);
            let fader_h = (available_h - 60.0).clamp(FADER_H, FADER_H_MAX);
            ui.style_mut().spacing.slider_width = fader_h;

            ui.horizontal(|ui| {
                // Rotated MASTER label gutter — same shape as
                // `strip()` (v0.4.22) but coloured yellow to mark it
                // as the master bus.
                let inner_h = fader_h + 60.0;
                let (label_rect, _) = ui.allocate_exact_size(
                    egui::vec2(STRIP_LABEL_COL_W + 2.0, inner_h),
                    egui::Sense::hover(),
                );
                draw_rotated_label(
                    ui,
                    label_rect,
                    "MASTER",
                    FONT_MASTER_NAME,
                    Color32::from_rgb(230, 200, 80),
                );

                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.add_sized([18.0, 20.0], egui::SelectableLabel::new(false, "M"))
                            .on_hover_text("Mute (no-op on bus)");
                        ui.add_sized([18.0, 20.0], egui::SelectableLabel::new(false, "S"))
                            .on_hover_text("Solo (no-op on bus)");
                        let armed = state.master_recording_armed.load(Ordering::Relaxed);
                        if ui
                            .add_sized([18.0, 20.0], egui::SelectableLabel::new(armed, "R"))
                            .on_hover_text("Arm — record master fader gestures")
                            .clicked()
                        {
                            let new_armed = !armed;
                            state
                                .master_recording_armed
                                .store(new_armed, Ordering::Relaxed);
                            if !new_armed {
                                just_disarmed = true;
                            }
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let mut gain = state.master_gain_db();
                        let resp = ui.add_sized(
                            [STRIP_W - STRIP_LABEL_COL_W - 30.0, fader_h],
                            egui::Slider::new(&mut gain, -60.0..=6.0)
                                .vertical()
                                .show_value(false),
                        );
                        if resp.changed() {
                            state.set_master_gain_db(gain);
                            new_master_db = Some(gain);
                        }
                        draw_meter(ui, state.master_peak_left(), fader_h);
                        draw_meter(ui, state.master_peak_right(), fader_h);
                    });
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(format!("{:+.1} dB", state.master_gain_db()))
                            .size(FONT_STRIP_DB)
                            .monospace(),
                    );
                });
            });
        });
    if let Some(db) = new_master_db {
        app.project.master_gain_db = db;
        app.project_dirty = true;
    }
    just_disarmed
}

fn draw_meter(ui: &mut egui::Ui, peak: f32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(METER_W, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 1.0, Color32::from_rgb(15, 15, 18));
    let h = peak.clamp(0.0, 1.0) * rect.height();
    let filled = Rect::from_min_size(
        Pos2::new(rect.min.x, rect.max.y - h),
        egui::vec2(rect.width(), h),
    );
    let color = if peak > 0.9 {
        Color32::from_rgb(230, 80, 80)
    } else if peak > 0.7 {
        Color32::from_rgb(230, 200, 80)
    } else {
        Color32::from_rgb(100, 220, 150)
    };
    painter.rect_filled(filled, 1.0, color);
}

/// Truncate `name` to at most `cap` chars, appending `…` if any chars
/// were dropped. Operates on `chars()` so multi-byte UTF-8 names (accents,
/// emoji) won't panic the way `&name[..n]` byte-slicing would.
fn ellipsize(name: &str, cap: usize) -> String {
    let count = name.chars().count();
    if count <= cap {
        name.to_owned()
    } else {
        // cap counts the visible glyphs including the ellipsis itself.
        let keep = cap.saturating_sub(1);
        let head: String = name.chars().take(keep).collect();
        format!("{head}…")
    }
}

// ───────────────────── automation recorder hooks ─────────────────────

fn capture_automation(app: &mut TinyBoothApp) {
    let Some(player) = app.player.as_ref() else {
        return;
    };
    if player.state.play_state() != PlayState::Playing {
        return;
    }
    let t = player.state.position_secs();
    for (i, track) in player.state.tracks.iter().enumerate() {
        if track.recording_armed.load(Ordering::Relaxed) {
            app.recorder.record_track(i, t, track.gain_db());
        }
    }
    if player.state.master_recording_armed.load(Ordering::Relaxed) {
        app.recorder.record_master(t, player.state.master_gain_db());
    }
}

fn stop_and_commit_automation(app: &mut TinyBoothApp) {
    if let Some(p) = app.player.as_ref() {
        p.stop();
    }
    // Commit any in-flight scratch lanes from armed strips.
    let arm_idxs: Vec<usize> = if let Some(p) = app.player.as_ref() {
        p.state
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.recording_armed.load(Ordering::Relaxed))
            .map(|(i, _)| i)
            .collect()
    } else {
        Vec::new()
    };
    for i in arm_idxs {
        commit_track_automation(app, i);
    }
    let master_armed = app
        .player
        .as_ref()
        .map(|p| p.state.master_recording_armed.load(Ordering::Relaxed))
        .unwrap_or(false);
    if master_armed {
        commit_master_automation(app);
    }
}

fn commit_track_automation(app: &mut TinyBoothApp, idx: usize) {
    let lane = app.recorder.track_scratch.remove(&idx);
    if let Some(lane) = lane {
        if !lane.is_empty() {
            app.project.tracks[idx].gain_automation = Some(lane.clone());
            if let Some(p) = app.player.as_ref() {
                if let Some(t) = p.state.tracks.get(idx) {
                    t.set_automation(Some(lane));
                }
            }
            app.project_dirty = true;
        }
    }
}

fn commit_master_automation(app: &mut TinyBoothApp) {
    let lane = std::mem::take(&mut app.recorder.master_scratch);
    if !lane.is_empty() {
        app.project.master_gain_automation = Some(lane.clone());
        if let Some(p) = app.player.as_ref() {
            p.state.set_master_automation(Some(lane));
        }
        app.project_dirty = true;
    }
}

/// `MM:SS` formatter — `pub` so the top-bar readings in app.rs
/// (v0.4.22) can re-use the same shape.
pub fn fmt_time(secs: f32) -> String {
    let total = secs.max(0.0) as u32;
    format!("{:02}:{:02}", total / 60, total % 60)
}

// ───────────────────── telemetry chips ─────────────────────
//
// Compact, single-line chip strip rendered under each lane's track
// name. Pulls from `Track.telemetry`; renders nothing if absent
// (analyzer hasn't run yet). v0.4.15 redesign — every glance-level
// numeric got consolidated into at most three visible elements:
//
//   ┌──────────┐  ┌──────────┐  ┌─┐
//   │ 🥁  543  │  │ ♪ G maj  │  │■│
//   └──────────┘  └──────────┘  └─┘
//      instrument      key       mood pip
//      summary
//
// — the instrument chip shows the headline number (drum hits OR
//   guitar picks) and its tooltip carries the full per-class
//   breakdown (kick/snare/hat/tom/cymbal or pluck/repeat/strum/slide).
// — the key chip shows the most-likely tonic + mode (only when
//   K-S confidence ≥ 0.5).
// — the mood pip's color encodes arousal × valence; its tooltip
//   carries the spectral / dynamics numerics that used to be five
//   separate chips (brightness, sustain, density, RMS, crest, peak).
//
// Single line via `ui.horizontal` (not `_wrapped`) → every row has
// the same height regardless of telemetry density. Headers are no
// longer uneven between drum stems and vocal stems.
fn telemetry_chips(ui: &mut egui::Ui, track: &crate::project::Track) {
    let Some(tel) = track.telemetry.as_ref() else {
        return;
    };
    ui.horizontal(|ui| {
        // ── Instrument summary chip ───────────────────────────
        // Drum stem → "🥁 N" with full per-class tooltip.
        // Guitar/bass stem → "🎸 N (↗M)" with kind-breakdown tooltip.
        if let Some(kit) = tel.drum_kit.as_ref() {
            let total = kit.kick_count
                + kit.snare_count
                + kit.hihat_count
                + kit.tom_count
                + kit.cymbal_count
                + kit.other_count;
            ui.label(
                egui::RichText::new(format!("🥁 {total}"))
                    .size(11.0)
                    .monospace()
                    .color(Color32::from_rgb(220, 200, 130)),
            )
            .on_hover_text(format!(
                "Drum hits: {total} total\n\
                 • {} kicks\n\
                 • {} snares\n\
                 • {} hats\n\
                 • {} toms\n\
                 • {} cymbals\n\
                 • {} other\n\
                 (full event list in Project → 📊 Project Health…)",
                kit.kick_count,
                kit.snare_count,
                kit.hihat_count,
                kit.tom_count,
                kit.cymbal_count,
                kit.other_count,
            ));
        } else if let Some(g) = tel.guitar.as_ref() {
            let suffix = if g.bend_or_slide_count > 0 {
                format!(" ↗{}", g.bend_or_slide_count)
            } else {
                String::new()
            };
            ui.label(
                egui::RichText::new(format!("🎸 {}{}", g.pick_count, suffix))
                    .size(11.0)
                    .monospace()
                    .color(Color32::from_rgb(200, 220, 180)),
            )
            .on_hover_text(format!(
                "Picks: {} total\n\
                 • {} plucks (new pitch)\n\
                 • {} repeats (same pitch)\n\
                 • {} bends / slides\n\
                 • {} strums (polyphonic)\n\
                 estimated polyphony {:.0}%",
                g.pick_count,
                g.pitch_change_count,
                g.repeated_pick_count,
                g.bend_or_slide_count,
                g.strum_count,
                g.estimated_polyphony * 100.0,
            ));
        }

        // ── Key estimate chip (when confident enough) ─────────
        if let Some(k) = tel.key_estimate.as_ref() {
            if k.confidence >= 0.50 {
                ui.label(
                    egui::RichText::new(format!("♪ {}", k.label()))
                        .size(11.0)
                        .color(Color32::from_rgb(200, 220, 240)),
                )
                .on_hover_text(format!(
                    "Estimated key: {} ({:.0}% confidence)\n\
                     Runner-up: {} {} ({:.0}%)\n\
                     (Krumhansl-Schmuckler over pitched events)",
                    k.label(),
                    k.confidence * 100.0,
                    note_name(k.second_choice_root),
                    k.second_choice_mode.label(),
                    k.second_choice_confidence * 100.0,
                ));
            }
        }

        // ── Mood pip ──────────────────────────────────────────
        // 10×10 px coloured square. Hue = valence (cool ↔ warm),
        // saturation = arousal. Tooltip carries every numeric
        // that used to be its own chip.
        let (r, g, b) = mood_color(tel.arousal, tel.valence);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 2.0, Color32::from_rgb(r, g, b));
        ui.painter()
            .rect_stroke(rect, 2.0, egui::Stroke::new(0.5, Color32::from_gray(80)));
        resp.on_hover_text(format!(
            "Mood proxy\n\
             • arousal {:.2}  (RMS + onsets + brightness)\n\
             • valence {:+.2}  (brightness × tonality)\n\
             \n\
             Timbre\n\
             • centroid {:.2}  ({})\n\
             • flatness {:.2}  ({})\n\
             • rolloff  {:.2}\n\
             \n\
             Dynamics\n\
             • RMS  {:.1} dB ± {:.1}\n\
             • peak {:.1} dB\n\
             • crest {:.1}\n\
             \n\
             Rhythm\n\
             • {} onsets ({:.1}/s)\n\
             • sustain {:.0}%",
            tel.arousal,
            tel.valence,
            tel.spectral_centroid_avg,
            if tel.spectral_centroid_avg >= 0.45 {
                "bright"
            } else if tel.spectral_centroid_avg <= 0.15 {
                "dark"
            } else {
                "neutral"
            },
            tel.spectral_flatness_avg,
            if tel.spectral_flatness_avg >= 0.5 {
                "noisy"
            } else {
                "tonal"
            },
            tel.spectral_rolloff_avg,
            tel.rms_avg_db,
            tel.rms_std_db,
            tel.peak_db,
            tel.crest_factor_avg,
            tel.onset_count,
            tel.onset_rate_hz,
            tel.sustain_ratio * 100.0,
        ));
    });
}

/// Short label for `ResolvedProfile`, used in the profile-dropdown
/// tooltip's "running as: X" line.
fn resolved_short(p: crate::telemetry::ResolvedProfile) -> &'static str {
    use crate::telemetry::ResolvedProfile;
    match p {
        ResolvedProfile::None => "off",
        ResolvedProfile::UniversalOnly => "universal",
        ResolvedProfile::Drums => "drums",
        ResolvedProfile::Guitar => "guitar",
        ResolvedProfile::Bass => "bass",
    }
}

/// 12-tone note name with sharps used for both the per-track and
/// project-level key chip / readout. Matches `KeyEstimate::label`.
fn note_name(root: u8) -> &'static str {
    const NOTES: [&str; 12] = [
        "C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B",
    ];
    NOTES[(root as usize) % 12]
}

/// Map an (arousal, valence) point in [0,1] × [-1,1] to an RGB triple.
/// Hue: valence (cool blue → warm yellow). Saturation: arousal.
fn mood_color(arousal: f32, valence: f32) -> (u8, u8, u8) {
    // Blue ≈ 220° at v=-1, yellow ≈ 50° at v=+1. Interpolate.
    let h = 220.0 - (valence.clamp(-1.0, 1.0) + 1.0) * 0.5 * (220.0 - 50.0);
    let s = arousal.clamp(0.0, 1.0) * 0.85 + 0.15;
    let v = 0.85;
    hsv_to_rgb_u8(h, s, v)
}

fn hsv_to_rgb_u8(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let h6 = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (h6 % 2.0 - 1.0).abs());
    let (r, g, b) = match h6 as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (
        ((r + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((g + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((b + m) * 255.0).clamp(0.0, 255.0) as u8,
    )
}
