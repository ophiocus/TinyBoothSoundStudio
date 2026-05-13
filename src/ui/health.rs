//! Project Health panel (TBSS-FR-0005 §"Health").
//!
//! Modal summary of every track's telemetry status: which ones have
//! up-to-date analysis, which are pending, and the **metadata weight**
//! — total bytes the telemetry consumes inside the manifest. The
//! event list on a drum stem can grow into the hundreds; this panel
//! is where the user sees the cost and decides whether to compact.
//!
//! Read-only for now. Future work (TBSS-FR-0005 §"Phase 4"):
//! per-track "Re-analyze" button, "Drop drum events past N" compaction,
//! "Re-run analyzer with version V" force-update.

use crate::app::TinyBoothApp;
use crate::project::{StemRole, TrackSource};
use crate::telemetry::ResolvedProfile;
use eframe::egui;

pub fn show(app: &mut TinyBoothApp, ctx: &egui::Context) {
    if !app.show_health {
        return;
    }
    let mut open = true;

    // Compute aggregates up-front so the closure body stays readable.
    let total_tracks = app.project.tracks.len();
    let mut analyzed = 0usize;
    let mut pending = 0usize;
    let mut total_events = 0usize;
    let mut total_bytes = 0usize;
    let mut drum_tracks = 0usize;

    for t in &app.project.tracks {
        let is_drum = matches!(
            &t.source,
            TrackSource::SunoStem {
                role: StemRole::Drums | StemRole::Percussion,
                ..
            }
        );
        if is_drum {
            drum_tracks += 1;
        }
        match &t.telemetry {
            None => pending += 1,
            Some(tel) => {
                analyzed += 1;
                if let Some(kit) = &tel.drum_kit {
                    total_events += kit.events.len();
                }
                // Approximate byte cost via JSON serialisation. Run
                // every render — projects are tiny, this is cheap.
                if let Ok(bytes) = serde_json::to_vec(tel) {
                    total_bytes += bytes.len();
                }
            }
        }
    }

    egui::Window::new("📊  Project Health")
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(680.0)
        .min_height(360.0)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(&app.project.name)
                    .heading()
                    .color(egui::Color32::from_rgb(180, 220, 240)),
            );
            ui.add_space(4.0);

            // ── Summary line ─────────────────────────────────────
            ui.horizontal(|ui| {
                ui.label(format!("{total_tracks} tracks"));
                ui.separator();
                ui.label(format!("{analyzed} analyzed"));
                ui.separator();
                if pending > 0 {
                    ui.label(
                        egui::RichText::new(format!("{pending} pending"))
                            .color(egui::Color32::from_rgb(240, 200, 100)),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("0 pending")
                            .color(egui::Color32::from_rgb(140, 200, 140)),
                    );
                }
                ui.separator();
                ui.label(format!("{drum_tracks} drum-classified"));
            });

            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Metadata weight:").strong());
                ui.label(format!(
                    "{} ({} drum events)",
                    fmt_bytes(total_bytes),
                    total_events
                ));
            });

            // Status-bar live progress, if a batch is still running.
            if let Some((done, total)) = app.telemetry.progress() {
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(format!("Analyzing {done}/{total}…"))
                        .italics()
                        .color(egui::Color32::from_rgb(180, 220, 240)),
                );
            }

            ui.separator();

            // ── Per-track table ──────────────────────────────────
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    egui::Grid::new("tbss_health_grid")
                        .num_columns(10)
                        .striped(true)
                        .min_col_width(50.0)
                        .show(ui, |ui| {
                            // Header row.
                            ui.label(egui::RichText::new("Track").strong());
                            ui.label(egui::RichText::new("Role").strong());
                            ui.label(egui::RichText::new("Profile").strong());
                            ui.label(egui::RichText::new("Status").strong());
                            ui.label(egui::RichText::new("Onsets").strong());
                            ui.label(egui::RichText::new("Sustain").strong());
                            ui.label(egui::RichText::new("Mood").strong());
                            ui.label(egui::RichText::new("Inst. layer").strong());
                            ui.label(egui::RichText::new("Key").strong());
                            ui.label(egui::RichText::new("Band Coh.").strong())
                                .on_hover_text(
                                    "Cross-band coherence (v0.4.35) — mean pairwise \
                                 Pearson correlation of octave-band energy \
                                 envelopes. Natural recordings 0.6–0.9; AI-audio \
                                 fingerprint 0.2–0.5. See sound-vision-philosophy.md §V.",
                                );
                            ui.end_row();

                            for t in &app.project.tracks {
                                ui.label(&t.name);
                                ui.label(role_label(&t.source));
                                let resolved = t.telemetry_profile.resolve(&t.source);
                                ui.label(format!(
                                    "{} → {}",
                                    t.telemetry_profile.label(),
                                    resolved_label(resolved)
                                ));
                                match &t.telemetry {
                                    None => {
                                        ui.label(
                                            egui::RichText::new("pending")
                                                .color(egui::Color32::from_rgb(240, 200, 100)),
                                        );
                                        ui.label("—");
                                        ui.label("—");
                                        ui.label("—");
                                        ui.label("—");
                                        ui.label("—");
                                        ui.label("—");
                                    }
                                    Some(tel) => {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "v{}",
                                                tel.analyzer_version
                                            ))
                                            .color(egui::Color32::from_rgb(140, 200, 140)),
                                        );
                                        ui.label(format!(
                                            "{} ({:.1}/s)",
                                            tel.onset_count, tel.onset_rate_hz
                                        ));
                                        ui.label(format!("{:.0}%", tel.sustain_ratio * 100.0));
                                        ui.label(format!(
                                            "a {:.2} · v {:+.2}",
                                            tel.arousal, tel.valence
                                        ));
                                        // Instrument layer roll-up:
                                        // drums → counts, guitar → pick
                                        // count + bend count, else dash.
                                        ui.label(if let Some(kit) = &tel.drum_kit {
                                            format!(
                                                "K{} S{} h{} T{} C{}",
                                                kit.kick_count,
                                                kit.snare_count,
                                                kit.hihat_count,
                                                kit.tom_count,
                                                kit.cymbal_count
                                            )
                                        } else if let Some(g) = &tel.guitar {
                                            format!(
                                                "🎸{} ↗{} (poly {:.0}%)",
                                                g.pick_count,
                                                g.bend_or_slide_count,
                                                g.estimated_polyphony * 100.0
                                            )
                                        } else {
                                            "—".to_string()
                                        });
                                        ui.label(match tel.key_estimate.as_ref() {
                                            None => "—".to_string(),
                                            Some(k) => {
                                                format!("{} ({:.2})", k.label(), k.confidence)
                                            }
                                        });
                                        // Cross-band coherence column.
                                        let c = tel.cross_band_coherence;
                                        let coh_color = if c < 0.45 {
                                            egui::Color32::from_rgb(220, 140, 220)
                                        } else if c >= 0.65 {
                                            egui::Color32::from_rgb(140, 220, 180)
                                        } else {
                                            egui::Color32::from_gray(180)
                                        };
                                        let coh_label = if c <= 0.05 {
                                            "—".to_string()
                                        } else if c < 0.45 {
                                            // v0.4.36 — 🤖 emoji renders as
                                            // tofu in egui's default font;
                                            // use plain "AI" suffix instead.
                                            format!("{c:.2}  AI")
                                        } else if c >= 0.65 {
                                            format!("{c:.2}  ≈")
                                        } else {
                                            format!("{c:.2}")
                                        };
                                        ui.label(egui::RichText::new(coh_label).color(coh_color));
                                    }
                                }
                                ui.end_row();
                            }
                        });
                });

            ui.separator();
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(
                        "Telemetry is computed once per track at first save, \
                         persisted in the manifest, refreshed after Trim. \
                         The Profile column shows what was selected → what \
                         was actually run (Auto resolves from the track role).",
                    )
                    .small()
                    .color(egui::Color32::from_gray(150)),
                );
            });
        });

    if !open {
        app.show_health = false;
    }
}

fn role_label(src: &TrackSource) -> String {
    match src {
        TrackSource::Recorded => "recorded".into(),
        TrackSource::SunoStem { role, .. } => role.label().to_string(),
    }
}

fn resolved_label(p: ResolvedProfile) -> &'static str {
    match p {
        ResolvedProfile::None => "off",
        ResolvedProfile::UniversalOnly => "universal",
        ResolvedProfile::Drums => "drums",
        ResolvedProfile::Guitar => "guitar",
        ResolvedProfile::Bass => "bass",
    }
}

fn fmt_bytes(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KiB", n as f32 / 1024.0)
    } else {
        format!("{:.2} MiB", n as f32 / (1024.0 * 1024.0))
    }
}
