//! TBSS-FR-0014 · E3–E5 — the Tracker tab.
//!
//! Vertical pattern editor (MadTracker layout: rows down, tracks across)
//! with FT2-style QWERTY note entry, an instrument rail whose samples the
//! user configures (file or recording take, decoded via `audiodecode`),
//! a looping transport on `CrossfadePreviewSession`, and two sinks: ⤓
//! bake-as-stem (via `add_rendered_track`) and WAV export.
//!
//! Keyboard: the grid is the app's first Editor-scope tenant
//! (TBSS-FR-0016) — while it holds egui focus it claims the keyboard by
//! setting `keyboard_editor_active` each frame and reads keys directly.
//! Chords marked `override_editor` (Ctrl+S, F1) still reach the app.
//!
//! v1 storage: the song persists as a sidecar JSON under the app config
//! dir keyed by project root — no project-format change. The FR's
//! manifest / `config_revs` integration is a follow-up.

use crate::app::TinyBoothApp;
use crate::tracker::{
    note_name, DecodedSample, LoopMode, Nna, TrackerCell, TrackerInstrument, TrackerSong,
};
use eframe::egui;
use std::path::PathBuf;

const GRID_FONT: f32 = 13.0;
const ROW_H: f32 = 16.0;

pub struct TrackerUiState {
    pub song: TrackerSong,
    /// Decoded audio per instrument (parallel to `song.instruments`).
    pub samples: Vec<DecodedSample>,
    pub cursor_track: usize,
    pub cursor_row: u16,
    pub octave: u8,
    pub edit_step: u16,
    pub selected_instrument: usize,
    /// Loop playback session + the frames of one rendered pass.
    pub playing: Option<crate::crossfade_player::CrossfadePreviewSession>,
    /// Song edited since the playing buffer was rendered.
    pub dirty_audio: bool,
    /// In-flight background ops: instrument decode / render.
    #[allow(clippy::type_complexity)]
    pub pending_decode: Option<(
        String,
        PathBuf,
        std::sync::mpsc::Receiver<Result<DecodedSample, String>>,
    )>,
    pub status: Option<String>,
    /// Which project root this song belongs to (sidecar key).
    pub loaded_for: Option<PathBuf>,
    pub song_dirty: bool,
    /// Source path per instrument (parallel to `song.instruments`);
    /// empty path = no source yet.
    pub sources: Vec<PathBuf>,
    /// Instruments awaiting decode: (instrument idx, path).
    pub decode_queue: Vec<(usize, PathBuf)>,
    /// Which instrument the in-flight decode belongs to.
    pub pending_decode_idx: usize,
    /// Piano-selected key (FR-0018): drives the zone wave-editor lane.
    pub piano_key_sel: Option<crate::tracker::Note>,
    /// Drag anchor for the zone lane's trim selection, in frames.
    pub zone_drag_anchor: Option<u64>,
}

impl Default for TrackerUiState {
    fn default() -> Self {
        Self {
            song: TrackerSong::new(8, 64),
            samples: Vec::new(),
            cursor_track: 0,
            cursor_row: 0,
            octave: 4,
            edit_step: 1,
            selected_instrument: 0,
            playing: None,
            dirty_audio: false,
            pending_decode: None,
            status: None,
            loaded_for: None,
            song_dirty: false,
            sources: Vec::new(),
            decode_queue: Vec::new(),
            pending_decode_idx: 0,
            piano_key_sel: None,
            zone_drag_anchor: None,
        }
    }
}

/// Sidecar path for a project's tracker song.
fn sidecar_path(project_root: &std::path::Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    project_root.to_string_lossy().hash(&mut h);
    crate::config::Config::dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("tracker")
        .join(format!("{:016x}.json", h.finish()))
}

fn load_song_for(root: &std::path::Path) -> Option<TrackerSong> {
    let p = sidecar_path(root);
    let text = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_song_for(root: &std::path::Path, song: &TrackerSong) -> anyhow::Result<()> {
    let p = sidecar_path(root);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(song)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

/// Sidecar companion: instrument source paths, parallel to
/// `song.instruments`, so samples re-decode on load.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct SidecarExtra {
    /// Source path per instrument, parallel to `song.instruments`.
    sources: Vec<PathBuf>,
}

fn extra_path(root: &std::path::Path) -> PathBuf {
    sidecar_path(root).with_extension("sources.json")
}

pub fn show(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    // Adopt the current project's song when the project changed.
    if app.tracker_state.loaded_for.as_deref() != Some(app.project.root.as_path()) {
        adopt_project(app);
    }
    poll_jobs(app, ui.ctx());

    ui.heading("Tracker");
    transport_bar(app, ui);
    ui.separator();

    ui.columns(2, |cols| {
        instrument_rail(app, &mut cols[0]);
        pattern_editor(app, &mut cols[1]);
    });
    ui.separator();
    piano_widget(app, ui);
    zone_lane(app, ui);

    if let Some(msg) = app.tracker_state.status.clone() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(msg).monospace());
    }
}

fn adopt_project(app: &mut TinyBoothApp) {
    let root = app.project.root.clone();
    let st = &mut app.tracker_state;
    st.playing = None;
    st.song = load_song_for(&root).unwrap_or_else(|| TrackerSong::new(8, 64));
    st.samples = vec![DecodedSample::default(); st.song.instruments.len()];
    st.loaded_for = Some(root.clone());
    st.song_dirty = false;
    st.dirty_audio = true;
    // Kick decodes for stored sources.
    let extra: SidecarExtra = std::fs::read_to_string(extra_path(&root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    // Decode sequentially via the pending mechanism: queue the first;
    // poll_jobs advances the queue.
    st.decode_queue = extra
        .sources
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.as_os_str().is_empty())
        .map(|(i, p)| (i, p.clone()))
        .collect();
    st.sources = extra.sources;
}

fn persist_song(app: &mut TinyBoothApp) {
    if let Some(root) = app.tracker_state.loaded_for.clone() {
        let extra = SidecarExtra {
            sources: app.tracker_state.sources.clone(),
        };
        let _ = std::fs::write(
            extra_path(&root),
            serde_json::to_string(&extra).unwrap_or_default(),
        );
        match save_song_for(&root, &app.tracker_state.song) {
            Ok(()) => app.tracker_state.song_dirty = false,
            Err(e) => app.tracker_state.status = Some(format!("save failed: {e:#}")),
        }
    }
}

// ───────────────────────── transport ─────────────────────────

fn transport_bar(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    let mut click_play = false;
    let mut click_stop = false;
    let mut click_bake = false;
    let mut click_export = false;
    let mut click_library = false;
    let mut click_demo: Option<usize> = None;
    ui.horizontal(|ui| {
        let st = &mut app.tracker_state;
        let playing = st.playing.is_some();
        if playing {
            click_stop = ui.button("⏹").clicked();
        } else {
            click_play = ui
                .add_enabled(!st.song.instruments.is_empty(), egui::Button::new("▶ Loop"))
                .clicked();
        }
        ui.add(
            egui::DragValue::new(&mut st.song.bpm)
                .range(32.0..=300.0)
                .prefix("BPM "),
        );
        ui.add(
            egui::DragValue::new(&mut st.song.speed)
                .range(1..=31)
                .prefix("speed "),
        );
        ui.label(format!(
            "rows {}  octave {}  step {}",
            st.song.patterns[0].rows, st.octave, st.edit_step
        ));
        if ui.button("💾").on_hover_text("Save tracker song").clicked() {
            st.song_dirty = true; // persist below
        }
        click_bake = ui
            .add_enabled(
                !st.song.instruments.is_empty(),
                egui::Button::new("⤓ Bake as stem"),
            )
            .on_hover_text("Render the loop and add it to this project as a track")
            .clicked();
        click_export = ui
            .add_enabled(
                !st.song.instruments.is_empty(),
                egui::Button::new("Export WAV…"),
            )
            .clicked();
        click_library = ui
            .button("🌐 Library…")
            .on_hover_text("Download free instrument packs (TBSS-FR-0018)")
            .clicked();
        egui::ComboBox::from_id_source("tracker_demos")
            .selected_text("Demos ▾")
            .width(90.0)
            .show_ui(ui, |ui| {
                for (i, (name, _)) in crate::tracker_demos::demo_songs().iter().enumerate() {
                    if ui.selectable_label(false, *name).clicked() {
                        click_demo = Some(i);
                    }
                }
            });
        if st.song_dirty {
            ui.label(egui::RichText::new("●").color(egui::Color32::YELLOW))
                .on_hover_text("Unsaved tracker edits");
        }
    });
    if app.tracker_state.song_dirty {
        persist_song(app);
    }
    if click_library {
        app.samplelib_state.open = true;
    }
    if let Some(i) = click_demo {
        load_demo(app, i);
    }
    if click_stop {
        app.tracker_state.playing = None;
    }
    if click_play {
        app.stop_all_playback();
        start_loop(app);
    }
    if click_bake || click_export {
        let song = app.tracker_state.song.clone();
        let samples = app.tracker_state.samples.clone();
        let rate = 48_000;
        let out = crate::tracker::render_song(&song, &samples, rate);
        match crate::export::encode_wav_16_bytes(&out, rate, 2) {
            Ok(bytes) => {
                if click_bake {
                    app.add_rendered_track(bytes, rate, 2, "tracker");
                } else {
                    let Some(p) = rfd::FileDialog::new()
                        .add_filter("WAV", &["wav"])
                        .set_file_name("tracker-loop.wav")
                        .save_file()
                    else {
                        return;
                    };
                    let p = if p.extension().is_none() {
                        p.with_extension("wav")
                    } else {
                        p
                    };
                    match std::fs::write(&p, &bytes) {
                        Ok(()) => {
                            app.tracker_state.status = Some(format!("exported {}", p.display()));
                        }
                        Err(e) => {
                            app.tracker_state.status = Some(format!("export failed: {e:#}"));
                        }
                    }
                }
            }
            Err(e) => app.tracker_state.status = Some(format!("render failed: {e:#}")),
        }
    }
}

fn start_loop(app: &mut TinyBoothApp) {
    let st = &mut app.tracker_state;
    let out = crate::tracker::render_song(&st.song, &st.samples, 48_000);
    if out.iter().all(|s| *s == 0.0) {
        st.status = Some("pattern renders silence — add notes / samples first.".into());
    }
    match crate::crossfade_player::CrossfadePreviewSession::play(out, 48_000, 2, 0) {
        Ok(s) => {
            st.playing = Some(s);
            st.dirty_audio = false;
        }
        Err(e) => st.status = Some(format!("playback failed: {e:#}")),
    }
}

fn poll_jobs(app: &mut TinyBoothApp, ctx: &egui::Context) {
    // Loop the transport: wrap at the end; re-render when edits landed.
    let restart = match app.tracker_state.playing.as_ref() {
        Some(s) if s.is_finished() => true,
        Some(_) => {
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
            false
        }
        None => false,
    };
    if restart {
        if app.tracker_state.dirty_audio {
            start_loop(app); // re-render with the edits at the boundary
        } else if let Some(s) = app.tracker_state.playing.as_ref() {
            s.seek_frac(0.0);
        }
    }

    // Instrument decode queue.
    let st = &mut app.tracker_state;
    if st.pending_decode.is_none() {
        if let Some((idx, path)) = st.decode_queue.pop() {
            let (tx, rx) = std::sync::mpsc::channel();
            let p = path.clone();
            std::thread::spawn(move || {
                let r = crate::audiodecode::decode_audio_mono(&p)
                    .map(|(data, sample_rate)| DecodedSample { data, sample_rate })
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(r);
            });
            st.pending_decode = Some((format!("instr {idx}"), path, rx));
            st.pending_decode_idx = idx;
        }
    }
    if let Some((_, _, rx)) = st.pending_decode.as_ref() {
        match rx.try_recv() {
            Ok(Ok(sample)) => {
                let idx = st.pending_decode_idx;
                if idx < st.samples.len() {
                    st.samples[idx] = sample;
                }
                st.pending_decode = None;
                st.dirty_audio = true;
            }
            Ok(Err(e)) => {
                st.pending_decode = None;
                st.status = Some(format!("decode failed: {e}"));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                st.pending_decode = None;
            }
        }
    }
}

// ───────────────────────── instruments ─────────────────────────

fn instrument_rail(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("Instruments").strong());
    let add_from_file;
    let mut remove_idx: Option<usize> = None;
    {
        let st = &mut app.tracker_state;
        egui::ScrollArea::vertical()
            .id_source("tracker_instruments")
            .max_height(320.0)
            .show(ui, |ui| {
                for i in 0..st.song.instruments.len() {
                    let selected = st.selected_instrument == i;
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(
                                selected,
                                format!("{i:02} {}", st.song.instruments[i].name),
                            )
                            .clicked()
                        {
                            st.selected_instrument = i;
                        }
                        let loaded = st
                            .samples
                            .get(i)
                            .map(|s| !s.data.is_empty())
                            .unwrap_or(false);
                        ui.label(if loaded { "●" } else { "…" });
                        if ui.button("🗑").clicked() {
                            remove_idx = Some(i);
                        }
                    });
                }
            });
        add_from_file = ui.button("+ sample…").clicked();

        // Selected-instrument editor.
        if let Some(inst) = st.song.instruments.get_mut(st.selected_instrument) {
            ui.separator();
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("base");
                let mut n = inst.base_note as i32;
                changed |= ui
                    .add(egui::DragValue::new(&mut n).range(0..=119))
                    .changed();
                inst.base_note = n as u8;
                ui.label(note_name(inst.base_note));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut inst.gain_db)
                            .range(-24.0..=12.0)
                            .suffix(" dB"),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                for (m, label) in [
                    (LoopMode::Off, "off"),
                    (LoopMode::Forward, "loop"),
                    (LoopMode::PingPong, "pingpong"),
                ] {
                    if ui.selectable_label(inst.loop_mode == m, label).clicked() {
                        inst.loop_mode = m;
                        changed = true;
                    }
                }
                for (m, label) in [(Nna::Cut, "cut"), (Nna::Continue, "ring")] {
                    if ui.selectable_label(inst.nna == m, label).clicked() {
                        inst.nna = m;
                        changed = true;
                    }
                }
            });
            ui.horizontal(|ui| {
                let mut on = inst.filter.is_some();
                if ui.checkbox(&mut on, "filter").changed() {
                    inst.filter = if on {
                        Some(crate::tracker::FilterCfg {
                            cutoff_hz: 4000.0,
                            q: 0.9,
                        })
                    } else {
                        None
                    };
                    changed = true;
                }
                if let Some(f) = inst.filter.as_mut() {
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut f.cutoff_hz)
                                .range(60.0..=16_000.0)
                                .suffix(" Hz"),
                        )
                        .changed();
                    changed |= ui
                        .add(egui::DragValue::new(&mut f.q).range(0.2..=8.0).prefix("Q "))
                        .changed();
                }
            });
            if changed {
                st.song_dirty = true;
                st.dirty_audio = true;
            }
        }
    }
    if let Some(i) = remove_idx {
        let st = &mut app.tracker_state;
        st.song.instruments.remove(i);
        st.samples.remove(i);
        if i < st.sources.len() {
            st.sources.remove(i);
        }
        st.selected_instrument = st
            .selected_instrument
            .min(st.song.instruments.len().saturating_sub(1));
        st.song_dirty = true;
        st.dirty_audio = true;
    }
    if add_from_file {
        let Some(p) = rfd::FileDialog::new()
            .add_filter("Audio", &crate::audiodecode::SUPPORTED_EXTS)
            .pick_file()
        else {
            return;
        };
        let st = &mut app.tracker_state;
        let name = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "sample".into());
        st.song.instruments.push(TrackerInstrument::simple(&name));
        st.samples.push(DecodedSample::default());
        st.sources.push(p.clone());
        st.selected_instrument = st.song.instruments.len() - 1;
        st.decode_queue.push((st.song.instruments.len() - 1, p));
        st.song_dirty = true;
    }
}

// ───────────────────────── pattern editor ─────────────────────────

/// FT2 QWERTY piano: two rows, lower octave on ZXCV…, upper on QWER….
fn key_to_note(key: egui::Key, octave: u8) -> Option<u8> {
    use egui::Key::*;
    let semis = match key {
        Z => 0,
        S => 1,
        X => 2,
        D => 3,
        C => 4,
        V => 5,
        G => 6,
        B => 7,
        H => 8,
        N => 9,
        J => 10,
        M => 11,
        Q => 12,
        Num2 => 13,
        W => 14,
        Num3 => 15,
        E => 16,
        R => 17,
        Num5 => 18,
        T => 19,
        Num6 => 20,
        Y => 21,
        Num7 => 22,
        U => 23,
        I => 24,
        _ => return None,
    };
    let n = octave as i32 * 12 + semis;
    (0..=119).contains(&n).then_some(n as u8)
}

fn pattern_editor(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    let st = &mut app.tracker_state;
    let rows = st.song.patterns[0].rows;
    let n_tracks = st.song.n_tracks();
    st.cursor_track = st.cursor_track.min(n_tracks.saturating_sub(1));
    st.cursor_row = st.cursor_row.min(rows.saturating_sub(1));

    // The grid claims the keyboard while focused (FR-0016 editor scope).
    let grid_id = ui.make_persistent_id("tracker_grid");
    let has_focus = ui.memory(|m| m.has_focus(grid_id));

    // ── keyboard (before drawing so the cursor moves this frame) ────
    if has_focus {
        app.keyboard_editor_active = true;
        let events = ui.input(|i| i.events.clone());
        for ev in events {
            if let egui::Event::Key {
                key, pressed: true, ..
            } = ev
            {
                handle_grid_key(st, key);
            }
        }
    }
    let st = &mut app.tracker_state; // reborrow after flag write

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Pattern 00").strong());
        ui.weak("click the grid, then play notes on the keyboard (FT2 layout)");
    });

    let visible_rows = 24usize;
    let first = (st.cursor_row as i32 - visible_rows as i32 / 2)
        .clamp(0, (rows as i32 - visible_rows as i32).max(0)) as u16;
    let width = 40.0 + n_tracks as f32 * 92.0;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(width.min(ui.available_width()), visible_rows as f32 * ROW_H),
        egui::Sense::click(),
    );
    if resp.clicked() {
        ui.memory_mut(|m| m.request_focus(grid_id));
        if let Some(p) = resp.interact_pointer_pos() {
            let row = first + ((p.y - rect.min.y) / ROW_H) as u16;
            let track = (((p.x - rect.min.x - 40.0) / 92.0) as usize).min(n_tracks - 1);
            st.cursor_row = row.min(rows - 1);
            st.cursor_track = track;
        }
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(12, 12, 16));
    let font = egui::FontId::monospace(GRID_FONT);
    for vis in 0..visible_rows {
        let row = first + vis as u16;
        if row >= rows {
            break;
        }
        let y = rect.min.y + vis as f32 * ROW_H;
        let row_rect =
            egui::Rect::from_min_size(egui::pos2(rect.min.x, y), egui::vec2(rect.width(), ROW_H));
        if row.is_multiple_of(4) {
            painter.rect_filled(row_rect, 0.0, egui::Color32::from_rgb(18, 18, 24));
        }
        if row == st.cursor_row {
            painter.rect_filled(
                row_rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(230, 200, 80, 26),
            );
        }
        painter.text(
            egui::pos2(rect.min.x + 4.0, y + 1.0),
            egui::Align2::LEFT_TOP,
            format!("{row:02X}"),
            font.clone(),
            egui::Color32::from_gray(110),
        );
        for t in 0..n_tracks {
            let x = rect.min.x + 40.0 + t as f32 * 92.0;
            let cell = st.song.patterns[0].cell(t, row);
            let text = cell_text(&cell);
            let color = if cell.note.is_some() {
                egui::Color32::from_rgb(160, 230, 180)
            } else {
                egui::Color32::from_gray(70)
            };
            if row == st.cursor_row && t == st.cursor_track {
                painter.rect_filled(
                    egui::Rect::from_min_size(egui::pos2(x - 2.0, y), egui::vec2(90.0, ROW_H)),
                    2.0,
                    egui::Color32::from_rgba_unmultiplied(230, 200, 80, 60),
                );
            }
            painter.text(
                egui::pos2(x, y + 1.0),
                egui::Align2::LEFT_TOP,
                text,
                font.clone(),
                color,
            );
        }
    }
    if !has_focus {
        painter.text(
            rect.center_bottom() - egui::vec2(0.0, 2.0),
            egui::Align2::CENTER_BOTTOM,
            "· click to focus ·",
            egui::FontId::proportional(10.0),
            egui::Color32::from_gray(90),
        );
    }
}

fn cell_text(cell: &TrackerCell) -> String {
    let note = cell
        .note
        .map(note_name)
        .unwrap_or_else(|| "···".to_string());
    let instr = cell
        .instr
        .map(|i| format!("{i:02}"))
        .unwrap_or_else(|| "··".into());
    let vol = cell
        .vol
        .map(|v| format!("{v:02}"))
        .unwrap_or_else(|| "··".into());
    format!("{note} {instr} {vol}")
}

fn handle_grid_key(st: &mut TrackerUiState, key: egui::Key) {
    use egui::Key::*;
    let rows = st.song.patterns[0].rows;
    match key {
        ArrowUp => st.cursor_row = st.cursor_row.saturating_sub(1),
        ArrowDown => st.cursor_row = (st.cursor_row + 1).min(rows - 1),
        ArrowLeft => st.cursor_track = st.cursor_track.saturating_sub(1),
        ArrowRight => st.cursor_track = (st.cursor_track + 1).min(st.song.n_tracks() - 1),
        PageUp => st.cursor_row = st.cursor_row.saturating_sub(16),
        PageDown => st.cursor_row = (st.cursor_row + 16).min(rows - 1),
        Home => st.cursor_row = 0,
        End => st.cursor_row = rows - 1,
        Delete => {
            let (t, r) = (st.cursor_track, st.cursor_row as usize);
            st.song.patterns[0].tracks[t][r] = TrackerCell::default();
            st.song_dirty = true;
            st.dirty_audio = true;
        }
        F9 => st.octave = st.octave.saturating_sub(1).max(1),
        F10 => st.octave = (st.octave + 1).min(8),
        k => {
            if let Some(note) = key_to_note(k, st.octave) {
                let (t, r) = (st.cursor_track, st.cursor_row as usize);
                st.song.patterns[0].tracks[t][r] = TrackerCell {
                    note: Some(note),
                    instr: Some(st.selected_instrument as u8),
                    ..st.song.patterns[0].tracks[t][r]
                };
                st.cursor_row = (st.cursor_row + st.edit_step).min(rows - 1);
                st.song_dirty = true;
                st.dirty_audio = true;
            }
        }
    }
}

// ───────────────── five-octave piano + zone lane (FR-0018) ─────────────────

/// C-2..B-6 — five octaves, 60 keys. Keys whose pitch is an exact zone
/// root render accented (a real recording lives there); other keys play
/// pitch-stretched from the nearest zone. Click = audition through the
/// real engine; the selected key drives the wave-editor lane below.
fn piano_widget(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    let st = &app.tracker_state;
    let Some(inst) = st.song.instruments.get(st.selected_instrument) else {
        return;
    };
    let zone_roots: std::collections::HashSet<u8> = if inst.zones.is_empty() {
        [inst.base_note].into_iter().collect()
    } else {
        inst.zones.iter().map(|z| z.root).collect()
    };
    let first: u8 = 24; // C-2
    let n_keys = 60usize;
    let white_w = (ui.available_width() / 35.0).clamp(10.0, 22.0);
    let h = 46.0;

    // Layout: x position per key (white index scan).
    let mut white_i = 0usize;
    let mut keys: Vec<(u8, bool, f32)> = Vec::with_capacity(n_keys); // (note, is_black, x)
    for k in 0..n_keys {
        let note = first + k as u8;
        let pc = note % 12;
        let black = matches!(pc, 1 | 3 | 6 | 8 | 10);
        let x = if black {
            white_i as f32 * white_w - white_w * 0.3
        } else {
            let x = white_i as f32 * white_w;
            white_i += 1;
            x
        };
        keys.push((note, black, x));
    }
    let total_w = white_i as f32 * white_w;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(total_w, h), egui::Sense::click());
    let painter = ui.painter_at(rect);

    // Whites first, blacks on top.
    for pass in 0..2 {
        for (note, black, x) in &keys {
            if (*black as usize) != pass {
                continue;
            }
            let (w, kh) = if *black {
                (white_w * 0.6, h * 0.6)
            } else {
                (white_w - 1.0, h)
            };
            let r = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + x, rect.min.y),
                egui::vec2(w, kh),
            );
            let covered = zone_roots.contains(note);
            let selected = st.piano_key_sel == Some(*note);
            let fill = match (black, covered) {
                (false, true) => egui::Color32::from_rgb(120, 190, 140),
                (false, false) => egui::Color32::from_gray(210),
                (true, true) => egui::Color32::from_rgb(40, 120, 70),
                (true, false) => egui::Color32::from_gray(25),
            };
            painter.rect_filled(r, 1.0, fill);
            if selected {
                painter.rect_stroke(
                    r,
                    1.0,
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(230, 200, 80)),
                );
            }
        }
    }
    painter.text(
        rect.left_bottom() + egui::vec2(2.0, 2.0),
        egui::Align2::LEFT_TOP,
        format!(
            "{} — green keys have a real recording; others pitch-stretch the nearest zone",
            inst.name
        ),
        egui::FontId::proportional(10.0),
        egui::Color32::from_gray(120),
    );

    if resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            // Hit-test blacks first (they overlay).
            let mut hit: Option<u8> = None;
            for (note, black, x) in keys.iter().rev() {
                let (w, kh) = if *black {
                    (white_w * 0.6, h * 0.6)
                } else {
                    (white_w - 1.0, h)
                };
                let r = egui::Rect::from_min_size(
                    egui::pos2(rect.min.x + x, rect.min.y),
                    egui::vec2(w, kh),
                );
                if *black && r.contains(pos) {
                    hit = Some(*note);
                    break;
                }
                if !*black && r.contains(pos) && hit.is_none() {
                    hit = Some(*note);
                }
            }
            if let Some(note) = hit {
                app.tracker_state.piano_key_sel = Some(note);
                audition_note(app, note);
            }
        }
    }
}

/// Play one note of the selected instrument through the real engine.
fn audition_note(app: &mut TinyBoothApp, note: crate::tracker::Note) {
    app.stop_all_playback();
    let st = &app.tracker_state;
    let out = crate::tracker::render_one_note(
        &st.song,
        &st.samples,
        st.selected_instrument,
        note,
        48_000,
        2.0,
    );
    if out.iter().all(|s| *s == 0.0) {
        app.tracker_state.status =
            Some("that key renders silence — sample still decoding, or no zones?".into());
        return;
    }
    match crate::crossfade_player::CrossfadePreviewSession::play(out, 48_000, 2, 0) {
        Ok(s) => app.tracker_state.playing = Some(s),
        Err(e) => app.tracker_state.status = Some(format!("audition failed: {e:#}")),
    }
}

/// The wave-editor lane for the piano-selected key's zone: peaks +
/// scrub-style playhead + drag-selection that WRITES the zone trim,
/// mirroring the Record/Mix wave language (FR-0015/0017).
fn zone_lane(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    let st = &mut app.tracker_state;
    let Some(key) = st.piano_key_sel else {
        return;
    };
    let Some(inst) = st.song.instruments.get(st.selected_instrument) else {
        return;
    };
    if inst.zones.is_empty() {
        ui.weak("single-sample instrument — add zones via 🌐 Library packs to edit per-key");
        return;
    }
    // Nearest zone for the selected key (same rule as the engine).
    let (zone_idx, zone) = inst
        .zones
        .iter()
        .enumerate()
        .min_by_key(|(_, z)| ((z.root as i32 - key as i32).abs(), z.root))
        .map(|(i, z)| (i, *z))
        .expect("non-empty zones");
    let Some(sample) = st.samples.get(zone.sample) else {
        return;
    };
    if sample.data.is_empty() {
        ui.weak("zone sample still decoding…");
        return;
    }
    let n = sample.data.len() as u64;
    let z_end = if zone.end == 0 { n } else { zone.end.min(n) };

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "zone {} · root {} · {} frames",
                zone_idx,
                note_name(zone.root),
                n
            ))
            .strong(),
        );
        ui.weak("drag = trim · both handles drawn · selection persists to the song");
    });
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().min(760.0), 44.0),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(14, 14, 18));
    // Peaks straight from the decoded data (pure, no file IO).
    let cols = rect.width() as usize;
    let mid = rect.center().y;
    let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 200, 130));
    if cols > 0 {
        let per = (sample.data.len() / cols.max(1)).max(1);
        for x in 0..cols {
            let s0 = x * per;
            let s1 = ((x + 1) * per).min(sample.data.len());
            let mut peak = 0.0f32;
            for v in &sample.data[s0..s1] {
                peak = peak.max(v.abs());
            }
            let hh = peak * 18.0;
            let xp = rect.min.x + x as f32;
            painter.line_segment([egui::pos2(xp, mid - hh), egui::pos2(xp, mid + hh)], stroke);
        }
    }
    // Trim window band.
    let fx = |frame: u64| rect.min.x + rect.width() * (frame as f32 / n.max(1) as f32);
    let band = egui::Rect::from_min_max(
        egui::pos2(fx(zone.start), rect.min.y),
        egui::pos2(fx(z_end), rect.max.y),
    );
    painter.rect_filled(
        band,
        0.0,
        egui::Color32::from_rgba_unmultiplied(230, 200, 80, 30),
    );
    for edge in [zone.start, z_end] {
        painter.line_segment(
            [
                egui::pos2(fx(edge), rect.min.y),
                egui::pos2(fx(edge), rect.max.y),
            ],
            egui::Stroke::new(1.5, egui::Color32::from_rgb(230, 200, 80)),
        );
    }

    // Drag = set trim (write-through to the song, FR-0018 E5).
    let to_frame = |x: f32| -> u64 {
        (((x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0) * n as f32) as u64
    };
    if resp.drag_started() {
        if let Some(p) = resp.interact_pointer_pos() {
            st.zone_drag_anchor = Some(to_frame(p.x));
        }
    }
    if resp.dragged() || resp.drag_stopped() {
        if let (Some(anchor), Some(p)) = (st.zone_drag_anchor, resp.interact_pointer_pos()) {
            let cur = to_frame(p.x);
            let (a, b) = if cur >= anchor {
                (anchor, cur)
            } else {
                (cur, anchor)
            };
            if b - a > 32 {
                let inst = &mut st.song.instruments[st.selected_instrument];
                inst.zones[zone_idx].start = a;
                inst.zones[zone_idx].end = b;
                st.song_dirty = true;
                st.dirty_audio = true;
            }
        }
        if resp.drag_stopped() {
            st.zone_drag_anchor = None;
        }
    }
}

/// Load a bundled public-domain demo song (traditional tunes — see
/// `tracker_demos`). Persists the current song first, then replaces it.
fn load_demo(app: &mut TinyBoothApp, idx: usize) {
    persist_song(app);
    let demos = crate::tracker_demos::demo_songs();
    let Some((name, song)) = demos.into_iter().nth(idx) else {
        return;
    };
    let st = &mut app.tracker_state;
    st.playing = None;
    // Keep the user's instruments + samples; the demo brings patterns.
    let instruments = std::mem::take(&mut st.song.instruments);
    st.song = song;
    st.song.instruments = instruments;
    if st.song.instruments.is_empty() {
        st.status = Some(format!(
            "loaded '{name}' — add an instrument (🌐 Library) to hear it; notes use instrument 00"
        ));
    } else {
        st.status = Some(format!("loaded '{name}' — playing through instrument 00"));
    }
    st.song_dirty = true;
    st.dirty_audio = true;
    st.cursor_row = 0;
}
