//! TBSS-FR-0013 · E2 — the editable chord-grid panel.
//!
//! The tab that ties the epic together: load a song, analyse it (E1), review
//! the detected progression as editable spans (E3), preview each chord's
//! fretboard diagram (E4), and render the synced video over the original audio
//! (E5).
//!
//! **The editable grid is the point, not a nicety.** Full-mix chord
//! recognition *will* miss borrowed and extended chords, so every span shows
//! its confidence, weak ones are flagged, and any span can be corrected — or
//! set to N.C. — in two clicks before rendering. Correcting a span re-resolves
//! its voicing through the same [`crate::chordvoice`] path the analyser used,
//! so an edited chart and a detected one render identically.
//!
//! Analysis and rendering both run on a background thread (one at a time) and
//! report back through a channel polled at the top of [`show`] — a full-song
//! analysis is seconds of DSP and the render shells out to ffmpeg, neither of
//! which may block the UI thread.

use crate::app::{ChordJobMsg, ChordVideoUiState, TinyBoothApp};
use crate::chordgrid::{ChordLabel, ChordQuality};
use eframe::egui;
use std::path::PathBuf;

pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    poll_job(app, ui.ctx());

    ui.heading("Chord chart video");
    ui.label(
        egui::RichText::new(
            "Detect the chord progression, correct anything the analyser got wrong, \
             then render a fretboard-diagram video over the original audio.",
        )
        .weak(),
    );
    ui.add_space(6.0);

    let busy = app.chordvideo_state.job.is_some();

    // ── source + actions ────────────────────────────────────────────────
    let mut click_load = false;
    let mut click_analyze = false;
    let mut click_render = false;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!busy, |ui| {
            click_load = ui.button("Load audio…").clicked();
            click_analyze = ui
                .add_enabled(
                    app.chordvideo_state.audio_path.is_some(),
                    egui::Button::new("Analyze"),
                )
                .clicked();
            click_render = ui
                .add_enabled(
                    !app.chordvideo_state.spans.is_empty(),
                    egui::Button::new("Render video…"),
                )
                .clicked();
        });
        if let Some((label, _)) = app.chordvideo_state.job.as_ref() {
            ui.spinner();
            ui.label(*label);
        }
    });

    if let Some(p) = app.chordvideo_state.audio_path.clone() {
        ui.label(
            egui::RichText::new(format!("source: {}", p.display()))
                .monospace()
                .weak(),
        );
    }

    // ── options ─────────────────────────────────────────────────────────
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.checkbox(&mut app.chordvideo_state.mirror, "Left-handed")
            .on_hover_text(
                "Mirrors the diagrams at draw time; the stored shapes stay right-handed.",
            );
        ui.add(
            egui::DragValue::new(&mut app.chordvideo_state.fps)
                .range(1..=60)
                .prefix("fps "),
        );
        ui.checkbox(
            &mut app.chordvideo_state.reencode_audio,
            "Re-encode audio (AAC)",
        )
        .on_hover_text(
            "Off keeps your audio bit-exact by stream-copying it. \
             A WAV source copies as PCM, which some players won't open — \
             turn this on for maximum compatibility at the cost of a lossy re-encode.",
        );
    });

    if let Some(g) = app.chordvideo_state.grid.as_ref() {
        ui.label(format!(
            "{:.1} BPM · {} beats · {} spans",
            g.bpm,
            g.beat_times.len(),
            app.chordvideo_state.spans.len()
        ));
    }

    ui.separator();

    if app.chordvideo_state.spans.is_empty() {
        ui.label(egui::RichText::new("Load a song and hit Analyze to see its chord grid.").weak());
    } else {
        ui.columns(2, |cols| {
            grid_editor(&mut app.chordvideo_state, &mut cols[0]);
            diagram_preview(&mut app.chordvideo_state, &mut cols[1]);
        });
    }

    if let Some(msg) = app.chordvideo_state.status.clone() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(msg).monospace());
    }

    // Deferred actions — the closures above hold `app` borrowed.
    if click_load {
        do_load(&mut app.chordvideo_state);
    }
    if click_analyze {
        do_analyze(&mut app.chordvideo_state);
    }
    if click_render {
        do_render(&mut app.chordvideo_state);
    }
}

/// The editable span list. Each row: timing, confidence flag, root + quality
/// pickers, and an N.C. toggle.
fn grid_editor(st: &mut ChordVideoUiState, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("Progression").strong());
    ui.label(
        egui::RichText::new("⚠ marks a low-confidence detection worth checking.")
            .weak()
            .small(),
    );
    ui.add_space(2.0);

    let mut edited: Option<usize> = None;
    egui::ScrollArea::vertical()
        .max_height(360.0)
        .show(ui, |ui| {
            for i in 0..st.spans.len() {
                let selected = st.selected == i;
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(
                            selected,
                            egui::RichText::new(format!(
                                "{:>6.2}s–{:>6.2}s",
                                st.spans[i].start_secs, st.spans[i].end_secs
                            ))
                            .monospace(),
                        )
                        .clicked()
                    {
                        st.selected = i;
                    }

                    if st.spans[i].low_confidence {
                        ui.label(egui::RichText::new("⚠").color(egui::Color32::YELLOW))
                            .on_hover_text("Low-confidence detection — worth a listen.");
                    }

                    let mut is_nc = st.spans[i].chord.is_none();
                    if ui.checkbox(&mut is_nc, "N.C.").changed() {
                        st.spans[i].chord = if is_nc {
                            None
                        } else {
                            Some(ChordLabel {
                                root: 0,
                                quality: ChordQuality::Major,
                            })
                        };
                        edited = Some(i);
                    }

                    if let Some(label) = st.spans[i].chord {
                        let mut root = label.root;
                        let mut quality = label.quality;
                        egui::ComboBox::from_id_source(("cv-root", i))
                            .selected_text(crate::chordgrid::note_name(root))
                            .width(52.0)
                            .show_ui(ui, |ui| {
                                for pc in 0..12u8 {
                                    ui.selectable_value(
                                        &mut root,
                                        pc,
                                        crate::chordgrid::note_name(pc),
                                    );
                                }
                            });
                        egui::ComboBox::from_id_source(("cv-qual", i))
                            .selected_text(quality_label(quality))
                            .width(74.0)
                            .show_ui(ui, |ui| {
                                for q in ChordQuality::all() {
                                    ui.selectable_value(&mut quality, q, quality_label(q));
                                }
                            });
                        if root != label.root || quality != label.quality {
                            st.spans[i].chord = Some(ChordLabel { root, quality });
                            edited = Some(i);
                        }
                    }
                });
            }
        });

    if let Some(i) = edited {
        reresolve(st, i);
    }
}

/// Re-pick the voicing for one edited span, keeping the neck-position
/// continuity the resolver gives detected chords.
fn reresolve(st: &mut ChordVideoUiState, i: usize) {
    let db = crate::chorddb::ChordDb::build();
    let prev = i
        .checked_sub(1)
        .and_then(|p| st.spans[p].voicing.as_ref())
        .map(|v| v.base_fret);
    let span = &mut st.spans[i];
    match span.chord {
        Some(label) => {
            span.voicing = crate::chordvoice::voice_label(&db, &label, prev);
            span.name = label.name();
        }
        None => {
            span.voicing = None;
            span.name = "N.C.".to_string();
        }
    }
    // An operator edit is a decision, not a guess.
    span.low_confidence = false;
    st.selected = i;
    st.preview_key = None;
}

fn quality_label(q: ChordQuality) -> &'static str {
    match q.suffix() {
        "" => "maj",
        s => s,
    }
}

/// Rasterise the selected span's diagram and show it, caching the texture.
fn diagram_preview(st: &mut ChordVideoUiState, ui: &mut egui::Ui) {
    let Some(span) = st.spans.get(st.selected) else {
        return;
    };
    ui.label(egui::RichText::new(format!("Diagram · {}", span.name)).strong());

    let key = (st.selected, st.mirror);
    if st.preview_key != Some(key) {
        let silent = crate::chorddb::Voicing {
            frets: [-1; 6],
            fingers: [0; 6],
            base_fret: 0,
            verified: false,
        };
        let v = span.voicing.as_ref().unwrap_or(&silent);
        let img = crate::fretboard::render(
            v,
            &crate::fretboard::RenderOpts {
                width: 480,
                height: 270,
                mirror: st.mirror,
                ..Default::default()
            },
        );
        let color = crate::fretboard::to_color_image(&img);
        match st.preview_tex.as_mut() {
            Some(h) => h.set(color, egui::TextureOptions::LINEAR),
            None => {
                st.preview_tex = Some(ui.ctx().load_texture(
                    "chord-diagram",
                    color,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        st.preview_key = Some(key);
    }

    if let Some(tex) = st.preview_tex.as_ref() {
        ui.add(egui::Image::new(tex).max_width(480.0));
    }
}

// ── actions ─────────────────────────────────────────────────────────────

fn do_load(st: &mut ChordVideoUiState) {
    let Some(p) = rfd::FileDialog::new()
        .add_filter("Audio", &crate::audiodecode::SUPPORTED_EXTS)
        .add_filter("All files", &["*"])
        .pick_file()
    else {
        return;
    };
    st.audio_path = Some(p);
    st.grid = None;
    st.spans.clear();
    st.selected = 0;
    st.preview_key = None;
    st.status = Some("loaded — hit Analyze".into());
}

fn do_analyze(st: &mut ChordVideoUiState) {
    let Some(path) = st.audio_path.clone() else {
        return;
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let msg = match crate::audiodecode::decode_audio_mono(&path) {
            Ok((mono, sr)) => {
                let grid = crate::chordgrid::analyze(&mono, sr);
                ChordJobMsg::Analyzed(Box::new(grid))
            }
            Err(e) => ChordJobMsg::Failed(format!("analyze failed: {e:#}")),
        };
        let _ = tx.send(msg);
    });
    st.job = Some(("analysing…", rx));
    st.status = None;
}

fn do_render(st: &mut ChordVideoUiState) {
    let Some(audio) = st.audio_path.clone() else {
        return;
    };
    let default_name = audio
        .file_stem()
        .map(|s| format!("{}-chords.mp4", s.to_string_lossy()))
        .unwrap_or_else(|| "chords.mp4".to_string());
    let Some(p) = rfd::FileDialog::new()
        .add_filter("MP4 video", &["mp4"])
        .set_file_name(&default_name)
        .save_file()
    else {
        return;
    };
    let out: PathBuf = if p.extension().is_none() {
        p.with_extension("mp4")
    } else {
        p
    };

    let spans = st.spans.clone();
    let opts = crate::chordvideo::VideoOpts {
        fps: st.fps.max(1),
        mirror: st.mirror,
        reencode_audio: st.reencode_audio,
        ..Default::default()
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let db = crate::chorddb::ChordDb::build();
        let msg = match crate::chordvideo::render_chord_video(&spans, &db, &audio, &out, &opts) {
            Ok(p) => ChordJobMsg::Rendered(p),
            Err(e) => ChordJobMsg::Failed(format!("render failed: {e:#}")),
        };
        let _ = tx.send(msg);
    });
    st.job = Some(("rendering…", rx));
    st.status = None;
}

/// Drain the background job's channel. Keeps repainting while a job is live so
/// the spinner animates and the result lands without needing mouse movement.
fn poll_job(app: &mut TinyBoothApp, ctx: &egui::Context) {
    let Some(msg) = app
        .chordvideo_state
        .job
        .as_ref()
        .map(|(_, rx)| rx.try_recv())
    else {
        return;
    };
    match msg {
        Ok(ChordJobMsg::Analyzed(grid)) => {
            let db = crate::chorddb::ChordDb::build();
            let spans = crate::chordvoice::resolve_spans(&grid, &db);
            app.chordvideo_state.status = Some(format!(
                "detected {:.1} BPM, {} chord spans",
                grid.bpm,
                spans.len()
            ));
            app.chordvideo_state.spans = spans;
            app.chordvideo_state.grid = Some(*grid);
            app.chordvideo_state.selected = 0;
            app.chordvideo_state.preview_key = None;
            app.chordvideo_state.job = None;
        }
        Ok(ChordJobMsg::Rendered(p)) => {
            app.chordvideo_state.status = Some(format!("wrote {}", p.display()));
            app.chordvideo_state.job = None;
        }
        Ok(ChordJobMsg::Failed(e)) => {
            app.chordvideo_state.status = Some(e);
            app.chordvideo_state.job = None;
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            app.chordvideo_state.status = Some("background job ended unexpectedly".into());
            app.chordvideo_state.job = None;
        }
    }
}
