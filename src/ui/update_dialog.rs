//! Modal "we're downloading the new release" overlay (v0.4.10).
//!
//! Bundling ffmpeg statically (also v0.4.10) bumped the MSI from
//! ~10 MB to ~130 MB. On a slower connection that's a multi-minute
//! wait — long enough that staring at a tiny "downloading…" label
//! in the bottom bar gets old. This dialog takes over the screen
//! during the download with:
//!
//!   • A spinner so the user knows the app hasn't frozen.
//!   • A short note explaining the bigger download (so it doesn't
//!     feel like a regression).
//!   • A rotating fortune-cookie tip — small workflow facts the
//!     user might not yet know about TinyBooth. Tip changes every
//!     `TIP_ROTATION_SECS` based on `ctx.input(|i| i.time)`, so
//!     even a 30 s download surfaces 5 different tips.
//!
//! Renders only while `git_update::UpdateState::Downloading(_)` is
//! active. The caller (app::update) checks the variant and calls
//! [`show`]; outside the download phase the dialog is a no-op.

use eframe::egui;

const TIP_ROTATION_SECS: f64 = 6.0;

/// Pithy workflow facts. Mix of v0.4.x feature pointers, mixing
/// rules-of-thumb, and the bedroom-studio-mystic vibe per
/// `docs/design-vibes.md`. Add to this list freely — every tip is
/// a small chance for the user to discover something they didn't
/// know.
const TIPS: &[&str] = &[
    "Recordings always land in %APPDATA%\\TinyBooth Sound Studio\\recordings\\ — never in your active stem-mixing project.",
    "The Ø button on every channel strip is a polarity flip. Useful when a stem reads anti-phase against the mixdown.",
    "Streaming targets — Spotify −14 LUFS, Apple Music −16, broadcast −23. The I readout on the transport bar tracks integrated loudness in real time.",
    "Drag the splitter between the Mix tab's lanes and console deck to give the strips more breathing room.",
    "\"Reset all\" clears every track's correction back to its default. Use it when you've explored too far down the wrong path.",
    "View → UI scale grows fonts AND widget metrics together. 125% reads comfortably on most screens.",
    "F1 toggles the in-app manual from anywhere — same content as the GitHub docs.",
    "Each Suno role auto-seeds its own correction preset at import: Suno-Vocal, Suno-Drums, Suno-Bass, Suno-Synth, etc. Eleven chains tuned for typical artefacts.",
    "The cleanse runs every frame. If a stray recording ends up in a Suno project's filespace, it migrates back to Recordings on the next render.",
    "✂ Trim project (Project tab) crops every WAV in the project to a shared time range. Re-import the bundle to undo.",
    "Arm a fader (R button) during playback to record the gesture. Catmull-Rom replays it on the next playback pass.",
    "Per-track A/B bypasses just that stem's correction. Disable (saves) is project-wide and persists across reloads.",
    "Import-time coherence sums every stem against the bundled mixdown. A residual below −30 dB means stems compose cleanly.",
    "A stem with Pearson r < −0.3 against the mixdown gets flagged as anti-phase at import. Try the Ø button on those.",
    "Nyquist clean is on by default for every Suno-X preset — top-octave LPF that suppresses Suno's characteristic AI shimmer. Turn it off only when you specifically want the shimmer.",
    "Click ▶ on any recording entry (Record tab) to send that take to the main mixer in one click — solos and plays.",
    "File → Open Recordings swaps the active project to the persistent recordings filespace.",
    "DC remove (sub-audible 5 Hz HPF) reclaims headroom on percussive stems by stripping any DC drift the AI may have introduced.",
    "Per-track Correction window: + Correction on each strip. Same chain editor as Admin → Recording-tone profiles.",
    "ffmpeg ships bundled (LGPL build) — FLAC, MP3, Ogg Vorbis, Ogg Opus, and M4A-AAC export are always available, offline.",
    "If a stem disappears on summation, polarity is your first suspect. Click Ø; if it pops back, you found it.",
    "Suno stems are co-rendered, so they share a single rate AND a single length. Mix-tab refuses any track that doesn't match — no resampling surprises mid-mix.",
];

/// Render the modal overlay. Caller is expected to gate this on
/// `matches!(state, UpdateState::Downloading(_))`.
pub fn show(ctx: &egui::Context) {
    let elapsed = ctx.input(|i| i.time);
    let tip_idx = ((elapsed / TIP_ROTATION_SECS) as usize) % TIPS.len();
    let tip = TIPS[tip_idx];

    // Dim the background so the dialog reads as modal. egui::Modal
    // would do this for us in newer versions, but we're on 0.28
    // where the recipe is "Area + foreground order + manual paint".
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("update_dialog_dim"))
        .fixed_pos(egui::pos2(0.0, 0.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.painter().rect_filled(
                screen,
                0.0,
                egui::Color32::from_rgba_premultiplied(0, 0, 0, 200),
            );
        });

    egui::Window::new("⬇  Updating TinyBooth Sound Studio")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_min_width(440.0);
            ui.set_max_width(560.0);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(24.0));
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Downloading the new release…").strong());
            });
            ui.add_space(6.0);
            ui.weak(
                "TinyBooth ships ffmpeg statically bundled, so the download is a bit \
                 heftier (~130 MB) but FLAC / MP3 / Ogg / M4A export Just Works \
                 offline on the next launch — no scavenging binaries off the internet.",
            );
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(10.0);
            ui.strong("While you wait:");
            ui.add_space(4.0);
            ui.label(egui::RichText::new(tip).italics());
            ui.add_space(8.0);
        });

    // Keep ticking so the spinner animates and tips rotate even if
    // there are no other repaint triggers active.
    ctx.request_repaint_after(std::time::Duration::from_millis(80));
}
