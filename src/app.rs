use crate::audio::{self, DeviceInfo, RecordingSession, SourceMode, VizState};
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
pub enum Tab { Record, Project, Export }

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

        // Load recording-tone profiles, seed defaults on first run, and
        // pick the last-used one (Guitar if nothing is saved).
        let profiles = dsp::load_or_seed();
        let active_profile_idx = profiles
            .iter()
            .position(|p| p.name == config.active_profile)
            .unwrap_or(0);

        Self {
            config,
            project: Project::new("Untitled session", default_root),
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
            status: None,
            show_manual: false,
            manual_slug: crate::manual::DEFAULT_SLUG.to_string(),
            md_cache: egui_commonmark::CommonMarkCache::default(),
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
                self.status = Some(format!("saved {}", self.project.manifest_path().display()));
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
                self.status = Some(format!("saved {}", self.project.manifest_path().display()));
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
        match crate::suno_import::import_folder(&src, &project_root, &name) {
            Ok(proj) => {
                let n_tracks = proj.tracks.len();
                self.status = Some(format!(
                    "Imported {n_tracks} stem(s) from {} → {}",
                    src.display(), proj.manifest_path().display()
                ));
                self.project = proj;
                self.project_dirty = false;
                self.tab = Tab::Project;
            }
            Err(e) => self.status = Some(format!("Suno import failed: {e}")),
        }
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
        match crate::suno_import::import_zip(&src, &project_root, &name) {
            Ok(proj) => {
                let n_tracks = proj.tracks.len();
                self.status = Some(format!(
                    "Imported {n_tracks} stem(s) from {} → {}",
                    src.display(), proj.manifest_path().display()
                ));
                self.project = proj;
                self.project_dirty = false;
                self.tab = Tab::Project;
            }
            Err(e) => self.status = Some(format!("Suno import failed: {e}")),
        }
    }

    pub fn open_project_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("TinyBooth project", &["tinybooth"])
            .pick_file()
        {
            match Project::load(&p) {
                Ok(proj) => {
                    self.project = proj;
                    self.project_dirty = false;
                    self.status = Some(format!("opened {}", p.display()));
                }
                Err(e) => self.status = Some(format!("open error: {e}")),
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
            Tab::Export => ui::export::show(self, ui),
        });

        // Admin window for editing recording-tone profiles.
        if self.show_admin {
            ui::admin::show(self, ctx);
        }

        // Floating manual window — non-modal, doesn't block anything else.
        if self.show_manual {
            ui::manual::show(self, ctx);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.session.is_some() {
            self.stop_take();
        }
    }
}

