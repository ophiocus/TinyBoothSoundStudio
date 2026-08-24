//! TBSS-FR-0018 · E1 UI — the Sample Library window.
//!
//! Lists the curated pack manifest with licenses shown up front,
//! downloads + extracts on a background thread with progress, and turns
//! a downloaded pack into tracker **multisample instruments**: files are
//! grouped by instrument prefix, pitches parsed from filenames, one file
//! chosen per note (loudest-dynamic preference), zones built, and the
//! samples queued through the tracker's existing background decoder.

use crate::app::TinyBoothApp;
use crate::samplelib::{self, SamplePack};
use crate::tracker::{SampleZone, TrackerInstrument};
use eframe::egui;
use std::path::PathBuf;

#[derive(Default)]
pub struct SampleLibUiState {
    pub open: bool,
    /// In-flight download: (pack id, progress, result receiver).
    #[allow(clippy::type_complexity)]
    pub download: Option<(
        String,
        samplelib::Progress,
        std::sync::mpsc::Receiver<Result<PathBuf, String>>,
    )>,
    pub status: Option<String>,
    /// Scanned groups of the selected downloaded pack:
    /// (instrument prefix, per-note files).
    pub pack_groups: Option<(String, Vec<InstrumentGroup>)>,
}

/// One buildable instrument found in a pack: prefix + per-note files.
pub type InstrumentGroup = (String, Vec<(PathBuf, crate::tracker::Note)>);

/// Dynamic-preference rank for picking ONE file per note out of a pack
/// that ships several dynamics/articulations per pitch. Lower = better.
fn dynamic_rank(name: &str) -> u8 {
    let n = name.to_ascii_lowercase();
    if n.contains("fortissimo") || n.contains(".ff.") || n.contains("_ff_") {
        1
    } else if n.contains("forte") {
        0 // forte first: strong but not clipped-hot
    } else if n.contains("mezzo") || n.contains(".mf.") || n.contains("_mf_") {
        2
    } else {
        3
    }
}

/// One file per note: dedupe a group's (path, note) list by note,
/// keeping the best-ranked dynamic; caps at 61 zones (the five-octave
/// piano + root) to bound decode time and RAM.
pub fn pick_zone_files(
    files: &[(PathBuf, crate::tracker::Note)],
) -> Vec<(PathBuf, crate::tracker::Note)> {
    use std::collections::BTreeMap;
    let mut best: BTreeMap<crate::tracker::Note, &(PathBuf, crate::tracker::Note)> =
        BTreeMap::new();
    for entry in files {
        let name = entry
            .0
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let rank = dynamic_rank(name);
        match best.get(&entry.1) {
            Some(cur) => {
                let cur_name = cur
                    .0
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default();
                if rank < dynamic_rank(cur_name) {
                    best.insert(entry.1, entry);
                }
            }
            None => {
                best.insert(entry.1, entry);
            }
        }
    }
    best.into_values().take(61).cloned().collect()
}

pub fn show(app: &mut TinyBoothApp, ctx: &egui::Context) {
    if !app.samplelib_state.open {
        return;
    }
    poll_download(app, ctx);

    let mut open = app.samplelib_state.open;
    let mut click_download: Option<SamplePack> = None;
    let mut click_scan: Option<SamplePack> = None;
    let mut click_remove: Option<String> = None;
    let mut click_build: Option<(String, Vec<(PathBuf, crate::tracker::Note)>)> = None;

    egui::Window::new("Sample Library")
        .open(&mut open)
        .default_width(560.0)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(
                    "Free instrument packs, downloaded from their sources on demand. \
                     Licenses shown per pack — nothing is bundled with TinyBooth.",
                )
                .weak(),
            );
            ui.separator();
            let busy = app.samplelib_state.download.is_some();
            for pack in samplelib::builtin_packs() {
                let downloaded = samplelib::pack_is_downloaded(&pack.id);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&pack.name).strong());
                    ui.label(format!("~{} MB", pack.approx_mb));
                    ui.hyperlink_to(&pack.license.id, &pack.license.url)
                        .on_hover_text(&pack.license.summary);
                    if downloaded {
                        if ui.button("Instruments…").clicked() {
                            click_scan = Some(pack.clone());
                        }
                        if ui.button("🗑").on_hover_text("Delete this pack").clicked() {
                            click_remove = Some(pack.id.clone());
                        }
                    } else if app
                        .samplelib_state
                        .download
                        .as_ref()
                        .is_some_and(|(id, _, _)| *id == pack.id)
                    {
                        let (_, prog, _) = app.samplelib_state.download.as_ref().unwrap();
                        let done = prog.0.load(std::sync::atomic::Ordering::Relaxed);
                        let total = prog.1.load(std::sync::atomic::Ordering::Relaxed);
                        if total > 0 {
                            ui.add(
                                egui::ProgressBar::new(done as f32 / total as f32)
                                    .desired_width(140.0)
                                    .text(format!("{} MB", done / 1024 / 1024)),
                            );
                        } else {
                            ui.spinner();
                        }
                    } else {
                        ui.add_enabled_ui(!busy, |ui| {
                            if ui
                                .button("⬇ Download")
                                .on_hover_text(&pack.license.summary)
                                .clicked()
                            {
                                click_download = Some(pack.clone());
                            }
                        });
                    }
                });
            }

            // Scanned pack → instrument groups.
            if let Some((pack_id, groups)) = app.samplelib_state.pack_groups.clone() {
                ui.separator();
                ui.label(egui::RichText::new(format!("Instruments in {pack_id}")).strong());
                egui::ScrollArea::vertical()
                    .id_source("samplelib_groups")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for (name, files) in &groups {
                            let picked = pick_zone_files(files);
                            ui.horizontal(|ui| {
                                ui.label(format!(
                                    "{name} — {} notes ({} files)",
                                    picked.len(),
                                    files.len()
                                ));
                                if ui.button("＋ Add as instrument").clicked() {
                                    click_build = Some((name.clone(), picked));
                                }
                            });
                        }
                    });
            }

            if let Some(msg) = app.samplelib_state.status.clone() {
                ui.separator();
                ui.label(egui::RichText::new(msg).monospace());
            }
        });
    app.samplelib_state.open = open;

    if let Some(pack) = click_download {
        start_download(app, pack);
    }
    if let Some(id) = click_remove {
        match samplelib::remove_pack(&id) {
            Ok(()) => app.samplelib_state.status = Some(format!("removed {id}")),
            Err(e) => app.samplelib_state.status = Some(format!("{e:#}")),
        }
        app.samplelib_state.pack_groups = None;
    }
    if let Some(pack) = click_scan {
        let (matched, unparsed) = samplelib::scan_pack(&samplelib::pack_dir(&pack.id));
        let groups = samplelib::group_by_instrument(&matched);
        app.samplelib_state.status = Some(format!(
            "{}: {} pitched files in {} instruments ({} unparseable skipped)",
            pack.id,
            matched.len(),
            groups.len(),
            unparsed
        ));
        app.samplelib_state.pack_groups = Some((pack.id, groups));
    }
    if let Some((name, picked)) = click_build {
        build_instrument(app, &name, &picked);
    }
}

fn start_download(app: &mut TinyBoothApp, pack: SamplePack) {
    let progress: samplelib::Progress = std::sync::Arc::new((
        std::sync::atomic::AtomicU64::new(0),
        std::sync::atomic::AtomicU64::new(0),
    ));
    let (tx, rx) = std::sync::mpsc::channel();
    let prog = progress.clone();
    let p = pack.clone();
    std::thread::spawn(move || {
        let r = samplelib::download_and_extract(&p, &prog).map_err(|e| format!("{e:#}"));
        let _ = tx.send(r);
    });
    app.samplelib_state.download = Some((pack.id, progress, rx));
    app.samplelib_state.status = None;
}

fn poll_download(app: &mut TinyBoothApp, ctx: &egui::Context) {
    let Some((id, _, rx)) = app.samplelib_state.download.as_ref() else {
        return;
    };
    match rx.try_recv() {
        Ok(Ok(dir)) => {
            app.samplelib_state.status = Some(format!("downloaded {id} → {}", dir.display()));
            app.samplelib_state.download = None;
        }
        Ok(Err(e)) => {
            app.samplelib_state.status = Some(format!("download failed: {e}"));
            app.samplelib_state.download = None;
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            app.samplelib_state.status = Some("download thread died".into());
            app.samplelib_state.download = None;
        }
    }
}

/// Create a multisample tracker instrument from picked per-note files:
/// zones point into the flat sample pool; decodes stream through the
/// tracker's existing background queue.
fn build_instrument(
    app: &mut TinyBoothApp,
    name: &str,
    picked: &[(PathBuf, crate::tracker::Note)],
) {
    if picked.is_empty() {
        return;
    }
    let st = &mut app.tracker_state;
    let mut inst = TrackerInstrument::simple(name);
    for (path, note) in picked {
        let sample_idx = st.samples.len();
        st.samples.push(crate::tracker::DecodedSample::default());
        st.sources.push(path.clone());
        st.decode_queue.push((sample_idx, path.clone()));
        inst.zones.push(SampleZone {
            root: *note,
            sample: sample_idx,
            start: 0,
            end: 0,
            loop_start: 0,
            loop_end: 0,
        });
    }
    // Root note for FT2 entry ergonomics: the median zone.
    inst.base_note = inst.zones[inst.zones.len() / 2].root;
    st.song.instruments.push(inst);
    st.selected_instrument = st.song.instruments.len() - 1;
    st.song_dirty = true;
    st.dirty_audio = true;
    st.status = Some(format!(
        "added '{name}' with {} zones — decoding in the background…",
        picked.len()
    ));
}
