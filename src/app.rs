use crate::audio::{self, DeviceInfo, RecordingSession, SourceMode, VizState};
use crate::suno_import::{ImportKind, PendingImport};
use crate::config::Config;
use crate::dsp::{self, Profile};
use crate::export::{self, ExportFormat};
use crate::git_update::{UpdateAvailable, UpdateState};
use crate::project::{Project, Track};
use crate::ui;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab { Record, Project, Mix, Export }

pub struct TinyBoothApp {
    pub config: Config,

    // Project state.
    pub project: Project,
    pub project_dirty: bool,

    // Recording state (Record tab).
    pub devices: Vec<DeviceInfo>,
    pub selected_device: Option<String>,
    pub selected_mode: SourceMode,
    pub viz: Arc<VizState>,
    pub session: Option<RecordingSession>,
    pub pending_track_name: String,

    // Recording-tone profiles.
    pub profiles: Vec<Profile>,
    pub active_profile_idx: usize,
    pub show_admin: bool,
    pub admin_edit_idx: Option<usize>,
    pub admin_status: Option<String>,

    // Export state (Export tab).
    pub export_format: ExportFormat,
    pub export_bitrate: u32,
    pub export_busy: bool,
    pub export_msg: Option<String>,
    pub ffmpeg_available: bool,

    // UI.
    pub tab: Tab,
    pub status: Option<String>,
    pub show_manual: bool,
    pub manual_slug: String,
    pub md_cache: egui_commonmark::CommonMarkCache,

    // Multitrack player (None until the first time the Mix tab is opened
    // for a project, or when tracks change shape and we need to rebuild).
    pub player: Option<crate::player::Player>,
    pub player_error: Option<String>,
    /// Index of the track whose Correction editor is open, if any.
    pub editing_correction_for: Option<usize>,

    /// Modal dialog shown after every import attempt — success or fail.
    pub import_dialog: Option<crate::suno_import::ImportOutcome>,

    /// Pending Suno import waiting for user resolution because the
    /// target project root already contains a project with a matching
    /// session epoch. The conflict modal shows while this is `Some`.
    pub import_conflict: Option<PendingImport>,

    /// Mixer/automation recorder. Captures fader gestures while a strip's
    /// arm toggle is on and the player is in Playing state. Flushed into
    /// the project on Stop / disarm.
    pub recorder: crate::automation::Recorder,
    /// Resizable split — what fraction of the Mix tab's height is the
    /// console deck (vs. the multitrack lane area).
    pub mix_console_fraction: f32,

    // Self-update plumbing.
    pub update_state: UpdateState,
    pub update_error: Option<String>,
    pub update_rx: Option<mpsc::Receiver<Option<UpdateAvailable>>>,
}

impl TinyBoothApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let config = Config::load();
        cc.egui_ctx.set_visuals(if config.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        cc.egui_ctx.set_zoom_factor(config.zoom);

        // Background update check.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::git_update::check_latest_release());
        });

        // Enumerate input devices once at startup; user can refresh later.
        let devices = audio::list_input_devices();
        let selected_device = devices.first().map(|d| d.name.clone());

        // Default scratch project in %APPDATA%\TinyBooth Sound Studio\sessions\unnamed.
        let default_root = Config::dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("sessions")
            .join(format!("session-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S")));

        // Try to auto-restore the last project. Fall back to a fresh
        // scratch session if the path is missing, the file's gone, or
        // the manifest fails to parse — and clear the stale breadcrumb.
        let mut config = config; // shadow to allow mut for recovery
        let mut startup_status: Option<String> = None;
        let project = match config.last_project_path.clone() {
            Some(p) if p.is_file() => match Project::load(&p) {
                Ok(proj) => {
                    startup_status = Some(format!("Restored: {}", proj.name));
                    proj
                }
                Err(e) => {
                    config.last_project_path = None;
                    config.save();
                    startup_status = Some(format!("Could not restore last project: {e}"));
                    Project::new("Untitled session", default_root.clone())
                }
            },
            Some(_) => {
                // Path was recorded but file's gone — clear it.
                config.last_project_path = None;
                config.save();
                Project::new("Untitled session", default_root.clone())
            }
            None => Project::new("Untitled session", default_root.clone()),
        };

        // Load recording-tone profiles, seed defaults on first run, and
        // pick the last-used one (Guitar if nothing is saved).
        let profiles = dsp::load_or_seed();
        let active_profile_idx = profiles
            .iter()
            .position(|p| p.name == config.active_profile)
            .unwrap_or(0);

        Self {
            config,
            project,
            project_dirty: false,
            devices,
            selected_device,
            selected_mode: SourceMode::Mixdown,
            viz: VizState::new(),
            session: None,
            pending_track_name: String::new(),
            profiles,
            active_profile_idx,
            show_admin: false,
            admin_edit_idx: None,
            admin_status: None,
            export_format: ExportFormat::Wav,
            export_bitrate: 192,
            export_busy: false,
            export_msg: None,
            ffmpeg_available: export::ffmpeg_available(),
            tab: Tab::Record,
            status: startup_status,
            show_manual: false,
            manual_slug: crate::manual::DEFAULT_SLUG.to_string(),
            md_cache: egui_commonmark::CommonMarkCache::default(),
            player: None,
            player_error: None,
            editing_correction_for: None,
            import_dialog: None,
            import_conflict: None,
            recorder: crate::automation::Recorder::default(),
            mix_console_fraction: 0.42,
            update_state: UpdateState::Checking,
            update_error: None,
            update_rx: Some(rx),
        }
    }

    pub fn active_profile(&self) -> &Profile {
        &self.profiles[self.active_profile_idx.min(self.profiles.len() - 1)]
    }

    pub fn set_active_profile(&mut self, idx: usize) {
        if idx >= self.profiles.len() { return; }
        self.active_profile_idx = idx;
        self.config.active_profile = self.profiles[idx].name.clone();
        self.config.save();
    }

    pub fn save_profiles(&mut self) {
        match dsp::save_profiles(&self.profiles) {
            Ok(()) => self.admin_status = Some("Profiles saved.".into()),
            Err(e) => self.admin_status = Some(format!("Save failed: {e}")),
        }
    }

    pub fn reset_profiles_to_defaults(&mut self) {
        self.profiles = dsp::builtin_profiles();
        // Keep the active selection pointing at a valid index.
        self.active_profile_idx = self
            .profiles
            .iter()
            .position(|p| p.name == self.config.active_profile)
            .unwrap_or(0);
        self.save_profiles();
    }

    pub fn start_new_take(&mut self) -> anyhow::Result<()> {
        let Some(dev) = self.selected_device.clone() else {
            anyhow::bail!("select an input device first");
        };
        let (id, abs) = self.project.new_track_slot();
        let name = if self.pending_track_name.trim().is_empty() {
            id.clone()
        } else {
            self.pending_track_name.trim().to_string()
        };
        std::fs::create_dir_all(&self.project.root)?;
        let profile = self.active_profile().clone();
        let mode = self.selected_mode;
        let session = audio::start_recording(
            &dev,
            mode,
            &abs,
            self.viz.clone(),
            profile.clone(),
        )?;
        let sample_rate = session.sample_rate;
        self.session = Some(session);
        let file_rel = abs
            .strip_prefix(&self.project.root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| format!("tracks/{id}.wav"));
        let channel_source = match mode {
            SourceMode::Channel(c) => Some(c),
            _ => None,
        };
        self.project.tracks.push(Track {
            id: id.clone(),
            name,
            file: file_rel,
            mute: false,
            gain_db: 0.0,
            sample_rate,
            channel_source,
            duration_secs: 0.0,
            profile: Some(profile),
            stereo: mode.is_stereo(),
            source: crate::project::TrackSource::Recorded,
            correction: None,
            gain_automation: None,
        });
        self.project_dirty = true;
        self.pending_track_name.clear();
        Ok(())
    }

    pub fn stop_take(&mut self) {
        if let Some(sess) = self.session.take() {
            let dur = sess.duration_secs();
            // Update the matching track row (last one we pushed).
            if let Some(last) = self.project.tracks.last_mut() {
                last.duration_secs = dur;
            }
            drop(sess);
            if let Err(e) = self.project.save() {
                self.status = Some(format!("save error: {e}"));
            } else {
                let manifest = self.project.manifest_path();
                self.config.record_project(&manifest);
                self.status = Some(format!("saved {}", manifest.display()));
                self.project_dirty = false;
            }
        }
    }

    pub fn set_project_root(&mut self, root: PathBuf, name: String) {
        self.project = Project::new(name, root);
        self.project_dirty = true;
    }

    pub fn save_project(&mut self) {
        match self.project.save() {
            Ok(()) => {
                let manifest = self.project.manifest_path();
                self.config.record_project(&manifest);
                self.status = Some(format!("saved {}", manifest.display()));
                self.project_dirty = false;
            }
            Err(e) => self.status = Some(format!("save error: {e}")),
        }
    }

    /// Open a folder of Suno stems and turn it into a fresh `.tinybooth`
    /// project. The new project is saved as a sibling of the source folder
    /// and immediately becomes the active project.
    pub fn import_suno_folder(&mut self) {
        let Some(src) = rfd::FileDialog::new()
            .set_title("Pick a folder of Suno stems")
            .pick_folder()
        else {
            return;
        };
        let name = src.file_name().map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Suno session".into());
        let parent = src.parent().unwrap_or_else(|| Path::new("."));
        let project_root = parent.join(format!("{name} (TinyBooth)"));
        let probe = crate::suno_import::probe_folder(&src, &project_root);
        if probe.is_duplicate() {
            self.import_conflict = Some(PendingImport {
                kind: ImportKind::Folder,
                source: src,
                project_root,
                project_name: name,
                probe,
            });
            return;
        }
        let outcome = crate::suno_import::import_folder(&src, &project_root, &name);
        self.apply_import_outcome(outcome);
    }

    /// Same as [`import_suno_folder`] but for a "Download All" zip archive.
    pub fn import_suno_zip(&mut self) {
        let Some(src) = rfd::FileDialog::new()
            .set_title("Pick a Suno stems zip archive")
            .add_filter("Zip archive", &["zip"])
            .pick_file()
        else {
            return;
        };
        let name = src.file_stem().map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Suno session".into());
        let parent = src.parent().unwrap_or_else(|| Path::new("."));
        let project_root = parent.join(format!("{name} (TinyBooth)"));
        let probe = crate::suno_import::probe_zip(&src, &project_root);
        if probe.is_duplicate() {
            self.import_conflict = Some(PendingImport {
                kind: ImportKind::Zip,
                source: src,
                project_root,
                project_name: name,
                probe,
            });
            return;
        }
        let outcome = crate::suno_import::import_zip(&src, &project_root, &name);
        self.apply_import_outcome(outcome);
    }

    /// Apply the Suno-Clean preset to every track that doesn't already
    /// carry a correction chain. Bulk action — saves the user from
    /// clicking `+ Correction` on every stem of a 9-stem project.
    pub fn enable_all_corrections(&mut self) {
        let seed = self.profiles.iter().find(|p| p.name == "Suno-Clean")
            .or_else(|| self.profiles.first())
            .cloned();
        let Some(seed) = seed else {
            self.status = Some("No profiles available to seed corrections.".into());
            return;
        };
        let mut changed = 0;
        let mut already = 0;
        for (i, track) in self.project.tracks.iter_mut().enumerate() {
            if track.correction.is_some() {
                already += 1;
                continue;
            }
            track.correction = Some(seed.clone());
            changed += 1;
            if let Some(player) = self.player.as_ref() {
                if let Some(t) = player.state.tracks.get(i) {
                    t.set_correction(Some(seed.clone()));
                }
            }
        }
        if changed > 0 {
            self.project_dirty = true;
            let preset = &seed.name;
            self.status = Some(if already > 0 {
                format!("Enabled '{preset}' on {changed} track(s) — {already} already had corrections.")
            } else {
                format!("Enabled '{preset}' on all {changed} track(s).")
            });
        } else {
            self.status = Some(format!("All {already} track(s) already have corrections."));
        }
    }

    /// Set every track's `bypass_correction` flag — non-destructive
    /// project-level A/B. Picks up mid-playback (the audio callback
    /// reads `bypass_correction` per-sample). Returns the new state
    /// (true = all bypassed = "raw source"; false = corrections live).
    /// Logic: if every track is currently bypassed, the toggle flips
    /// to "live"; otherwise it flips to "all bypassed" — same shape as
    /// a single A/B toggle but at project scope.
    pub fn toggle_global_bypass(&mut self) -> bool {
        let Some(player) = self.player.as_ref() else { return false };
        if player.state.tracks.is_empty() { return false; }
        let all_bypassed = player.state.tracks.iter()
            .all(|t| t.bypass_correction.load(std::sync::atomic::Ordering::Relaxed));
        let new_state = !all_bypassed;
        for t in &player.state.tracks {
            t.bypass_correction.store(new_state, std::sync::atomic::Ordering::Relaxed);
        }
        self.status = Some(if new_state {
            "Global bypass ON — playback is now the raw source for every track.".into()
        } else {
            "Global bypass OFF — all correction chains live again.".into()
        });
        new_state
    }

    /// Strip every track's correction chain. Counterpart to
    /// `enable_all_corrections` for full project-level A/B comparison
    /// or starting over from scratch.
    pub fn disable_all_corrections(&mut self) {
        let mut changed = 0;
        for (i, track) in self.project.tracks.iter_mut().enumerate() {
            if track.correction.is_none() { continue; }
            track.correction = None;
            changed += 1;
            if let Some(player) = self.player.as_ref() {
                if let Some(t) = player.state.tracks.get(i) {
                    t.set_correction(None);
                }
            }
        }
        if changed > 0 {
            self.project_dirty = true;
            self.status = Some(format!("Disabled corrections on {changed} track(s)."));
        } else {
            self.status = Some("No tracks had corrections to disable.".into());
        }
    }

    /// Resolve a pending import (called by the conflict modal).
    /// `replace = true` wipes the existing project and re-imports.
    pub fn resolve_import_conflict(&mut self, replace: bool) {
        let Some(pending) = self.import_conflict.take() else { return };
        if !replace { return; } // Cancel — do nothing
        if let Err(e) = crate::suno_import::wipe_project_root(&pending.project_root) {
            self.status = Some(format!("Could not wipe existing project: {e}"));
            return;
        }
        let outcome = match pending.kind {
            ImportKind::Folder => crate::suno_import::import_folder(
                &pending.source, &pending.project_root, &pending.project_name,
            ),
            ImportKind::Zip => crate::suno_import::import_zip(
                &pending.source, &pending.project_root, &pending.project_name,
            ),
        };
        self.apply_import_outcome(outcome);
    }

    /// Common post-import handling. Updates state on success and always
    /// pops the modal regardless of outcome — silence-on-failure is what
    /// made this whole flow feel broken.
    fn apply_import_outcome(&mut self, outcome: crate::suno_import::ImportOutcome) {
        if let Some(proj) = outcome.project.as_ref() {
            let manifest = proj.manifest_path();
            self.config.record_project(&manifest);
        }
        if outcome.success {
            if let Some(proj) = outcome.project.clone() {
                self.project = proj;
                self.project_dirty = false;
                self.player = None;
                self.tab = Tab::Project;
            }
        }
        self.status = Some(if outcome.success {
            format!("Imported into {}", self.project.manifest_path().display())
        } else {
            "Suno import did not produce any tracks — see dialog".into()
        });
        self.import_dialog = Some(outcome);
    }

    pub fn open_project_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("TinyBooth project", &["tinybooth"])
            .pick_file()
        {
            self.open_project_path(&p);
        }
    }

    /// Load a project manifest from a known path. Used by the Open
    /// dialog and by the File → Open Recent submenu.
    pub fn open_project_path(&mut self, path: &Path) {
        match Project::load(path) {
            Ok(proj) => {
                self.config.record_project(path);
                self.project = proj;
                self.project_dirty = false;
                self.player = None; // force player rebuild for new project
                self.status = Some(format!("opened {}", path.display()));
            }
            Err(e) => {
                // Stale recent — drop it so the menu cleans up over time.
                self.config.recent_projects.retain(|p| p != path);
                self.config.save();
                self.status = Some(format!("open error: {e}"));
            }
        }
    }
}

impl eframe::App for TinyBoothApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Repaint continuously while recording so the visualizer animates.
        if self.session.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }

        // F1 toggles the manual. Skipped when a text field has focus so it
        // doesn't fight typing in the Admin window or track-name input.
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.show_manual = !self.show_manual;
        }

        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New project…").clicked() {
                        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                            let name = dir.file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| "Session".into());
                            self.set_project_root(dir, name);
                            ui.close_menu();
                        }
                    }
                    if ui.button("Open project…").clicked() {
                        self.open_project_dialog();
                        ui.close_menu();
                    }
                    let mut recent_clicked: Option<PathBuf> = None;
                    let mut clear_recent = false;
                    ui.menu_button("Open Recent", |ui| {
                        if self.config.recent_projects.is_empty() {
                            ui.label(egui::RichText::new("(none yet)").weak());
                        } else {
                            for path in &self.config.recent_projects {
                                let label = path
                                    .parent()
                                    .and_then(|p| p.file_name())
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| path.display().to_string());
                                if ui
                                    .button(label)
                                    .on_hover_text(path.display().to_string())
                                    .clicked()
                                {
                                    recent_clicked = Some(path.clone());
                                    ui.close_menu();
                                }
                            }
                            ui.separator();
                            if ui.button("Clear list").clicked() {
                                clear_recent = true;
                                ui.close_menu();
                            }
                        }
                    });
                    if let Some(p) = recent_clicked {
                        self.open_project_path(&p);
                    }
                    if clear_recent {
                        self.config.clear_recent();
                    }
                    if ui.button("Save").clicked() {
                        self.save_project();
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.label(egui::RichText::new("Import Suno stems").weak());
                    if ui.button("…from folder").clicked() {
                        self.import_suno_folder();
                        ui.close_menu();
                    }
                    if ui.button("…from zip").clicked() {
                        self.import_suno_zip();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        std::process::exit(0);
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.checkbox(&mut self.config.dark_mode, "Dark mode").changed() {
                        ctx.set_visuals(if self.config.dark_mode {
                            egui::Visuals::dark()
                        } else {
                            egui::Visuals::light()
                        });
                        self.config.save();
                    }
                });
                ui.menu_button("Admin", |ui| {
                    if ui.button("Recording-tone profiles…").clicked() {
                        self.show_admin = true;
                        if self.admin_edit_idx.is_none() {
                            self.admin_edit_idx = Some(self.active_profile_idx);
                        }
                        ui.close_menu();
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("Manual…  (F1)").clicked() {
                        self.show_manual = true;
                        ui.close_menu();
                    }
                });

                ui.separator();
                ui.selectable_value(&mut self.tab, Tab::Record, "Record");
                ui.selectable_value(&mut self.tab, Tab::Project, "Project");
                ui.selectable_value(&mut self.tab, Tab::Mix, "Mix");
                ui.selectable_value(&mut self.tab, Tab::Export, "Export");

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if self.project_dirty {
                        format!("● {}", self.project.name)
                    } else {
                        self.project.name.clone()
                    };
                    ui.label(label);
                });
            });
        });

        egui::TopBottomPanel::bottom("bottom_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                crate::git_update::render(
                    ui,
                    &mut self.update_state,
                    &mut self.update_error,
                    &mut self.update_rx,
                );
                ui.separator();
                if let Some(s) = self.status.as_ref() {
                    ui.label(s);
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Record => ui::record::show(self, ui),
            Tab::Project => ui::project::show(self, ui),
            Tab::Mix => ui::mix::show(self, ui),
            Tab::Export => ui::export::show(self, ui),
        });

        // Mix-tab transport runs continuously while playing — repaint so
        // the playhead animates.
        if let Some(p) = self.player.as_ref() {
            if p.state.play_state() == crate::player::PlayState::Playing {
                ctx.request_repaint_after(std::time::Duration::from_millis(33));
            }
        }

        // Admin window for editing recording-tone profiles.
        if self.show_admin {
            ui::admin::show(self, ctx);
        }

        // Floating manual window — non-modal, doesn't block anything else.
        if self.show_manual {
            ui::manual::show(self, ctx);
        }

        // Per-track Correction editor — also a floating window.
        if self.editing_correction_for.is_some() {
            ui::correction::show(self, ctx);
        }

        // Import-result modal — always shown after an import completes,
        // success or fail. Can't be missed.
        if self.import_dialog.is_some() {
            ui::import_dialog::show(self, ctx);
        }

        // Duplicate-import conflict modal.
        if self.import_conflict.is_some() {
            ui::import_conflict::show(self, ctx);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.session.is_some() {
            self.stop_take();
        }
    }
}

