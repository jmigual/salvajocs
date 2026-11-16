//! The Settings tab: repository, compression, password and roots (ADR-0009).

use std::path::PathBuf;

use anyhow::anyhow;
use eframe::egui::{self, Color32, Ui};
use salvajocs_core::config::PasswordMode;
use salvajocs_core::layout::{Root, RootKind};
use salvajocs_core::storage::Compression;

use crate::first_run::Connect;
use crate::job::Job;
use crate::state::State;
use crate::widgets::{compression, labelled_edit};

/// The roots offered in the picker, in order.
const ROOT_KINDS: [RootKind; 3] = [RootKind::Steam, RootKind::Other, RootKind::WinePrefix];

fn root_kind_label(kind: RootKind) -> &'static str {
    match kind {
        RootKind::Steam => "Steam library",
        RootKind::Other => "Games folder",
        RootKind::WinePrefix => "Wine prefix",
    }
}

fn password_mode_label(mode: PasswordMode) -> &'static str {
    match mode {
        PasswordMode::None => "No password",
        PasswordMode::Keyring => "Stored in the system keyring",
        PasswordMode::Ask => "Asked once per session",
    }
}

/// The state of the Settings tab.
pub struct SettingsTab {
    connect: Option<Connect>,
    compression: Option<String>,
    compression_job: Option<Job<Compression>>,
    compression_error: Option<String>,
    root_kind: RootKind,
    root_path: String,
    root_picker: Option<Job<Option<PathBuf>>>,
    cleanup: Option<Job<()>>,
}

impl Default for SettingsTab {
    fn default() -> Self {
        Self {
            connect: None,
            compression: None,
            compression_job: None,
            compression_error: None,
            root_kind: RootKind::Other,
            root_path: String::new(),
            root_picker: None,
            cleanup: None,
        }
    }
}

impl SettingsTab {
    /// Drops the edited values, so the tab shows the current config and repository again.
    pub fn reset(&mut self) {
        self.connect = None;
        self.compression = None;
        self.compression_job = None;
        self.compression_error = None;
    }

    /// Draws the tab.
    pub fn ui(&mut self, ui: &mut Ui, state: &mut State) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            self.repository_ui(ui, state);
            ui.separator();
            self.compression_ui(ui, state);
            ui.separator();
            self.roots_ui(ui, state);
            self.cleanup_ui(ui, state);
        });
    }

    fn repository_ui(&mut self, ui: &mut Ui, state: &mut State) {
        ui.heading("Repository");
        if let Some(config) = &state.config {
            ui.label(format!("Current: {}", config.repository));
            ui.label(format!(
                "Password: {}",
                password_mode_label(config.password)
            ));
        }
        let connect = self.connect.get_or_insert_with(|| match &state.config {
            Some(config) => Connect::editing(config, &state.credentials),
            None => Connect::first_run(),
        });
        if let Some(connected) = connect.ui(ui, &state.env) {
            state.adopt(connected);
            self.reset();
        }
    }

    fn compression_ui(&mut self, ui: &mut Ui, state: &mut State) {
        ui.heading("Compression");
        let Some(session) = &mut state.session else {
            ui.label("The repository is not open.");
            return;
        };
        if let Some(job) = &mut self.compression_job
            && let Some(result) = job.take_result()
        {
            self.compression_job = None;
            match result {
                Ok(stored) => {
                    session.compression = stored;
                    self.compression = None;
                    self.compression_error = None;
                }
                Err(err) => self.compression_error = Some(format!("{err:#}")),
            }
        }
        let text = self
            .compression
            .get_or_insert_with(|| compression::format(session.compression));
        let parsed = egui::Grid::new("repository_compression")
            .num_columns(2)
            .show(ui, |ui| compression::ui(ui, "Compression", text))
            .inner;
        if let Some(error) = &self.compression_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        let new = parsed.ok().filter(|new| *new != session.compression);
        let apply = ui.add_enabled(
            new.is_some() && self.compression_job.is_none(),
            egui::Button::new("Apply compression"),
        );
        if let (true, Some(new)) = (apply.clicked(), new) {
            let storage = session.storage.clone();
            self.compression_job = Some(Job::spawn(ui.ctx(), move || {
                let mut storage = storage
                    .lock()
                    .map_err(|_| anyhow!("the repository lock is poisoned"))?;
                storage.set_compression(new)?;
                Ok(storage.compression())
            }));
        }
    }

    fn roots_ui(&mut self, ui: &mut Ui, state: &mut State) {
        ui.heading("Roots");
        let Some(config) = &mut state.config else {
            ui.label("Choose a repository first.");
            return;
        };
        let mut changed = false;
        let mut remove = None;
        for (index, root) in config.roots.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} ({})",
                    root.path.display(),
                    root_kind_label(root.kind)
                ));
                if ui.button("Remove").clicked() {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            config.roots.remove(index);
            changed = true;
        }
        if let Some(job) = &mut self.root_picker
            && let Some(result) = job.take_result()
        {
            self.root_picker = None;
            if let Ok(Some(folder)) = result {
                self.root_path = folder.to_string_lossy().into_owned();
            }
        }
        egui::Grid::new("add_root").num_columns(2).show(ui, |ui| {
            let label = ui.label("Kind");
            egui::ComboBox::from_id_salt("root_kind")
                .selected_text(root_kind_label(self.root_kind))
                .show_ui(ui, |ui| {
                    for kind in ROOT_KINDS {
                        ui.selectable_value(&mut self.root_kind, kind, root_kind_label(kind));
                    }
                })
                .response
                .labelled_by(label.id);
            ui.end_row();
            labelled_edit(ui, "Root folder", &mut self.root_path, false);
            if ui.button("Browse").clicked() && self.root_picker.is_none() {
                self.root_picker = Some(Job::spawn(ui.ctx(), || {
                    Ok(rfd::FileDialog::new().pick_folder())
                }));
            }
            ui.end_row();
        });
        let path = self.root_path.trim();
        if ui
            .add_enabled(!path.is_empty(), egui::Button::new("Add root"))
            .clicked()
        {
            config.roots.push(Root {
                path: PathBuf::from(path),
                kind: self.root_kind,
            });
            self.root_path.clear();
            changed = true;
        }
        if changed {
            state.save_config(ui.ctx());
        }
    }

    /// Offers to delete the keyring entries of the repository that was just replaced.
    fn cleanup_ui(&mut self, ui: &mut Ui, state: &mut State) {
        if let Some(job) = &mut self.cleanup
            && let Some(result) = job.take_result()
        {
            self.cleanup = None;
            if let Err(err) = result {
                state
                    .notices
                    .push(format!("Could not delete the old keyring entries: {err:#}"));
            }
        }
        if state.stale.is_none() {
            return;
        }
        ui.separator();
        ui.label("Delete the keyring entries of the previous repository?");
        ui.horizontal(|ui| {
            if ui.button("Delete entries").clicked()
                && let Some(stale) = state.stale.take()
            {
                let store = state.env.secrets.clone();
                self.cleanup = Some(Job::spawn(ui.ctx(), move || {
                    for key in &stale.keys {
                        store.delete(&stale.scope, key)?;
                    }
                    Ok(())
                }));
            }
            if ui.button("Keep entries").clicked() {
                state.stale = None;
            }
        });
    }
}
