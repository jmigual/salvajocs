//! The Settings tab: repository, compression, password, old backups (ADR-0015), roots (ADR-0009,
//! ADR-0017), appearance and updates (ADR-0013).

use std::path::PathBuf;

use anyhow::anyhow;
use eframe::egui::{self, Ui};
use salvajocs_core::config::{PasswordMode, ThemeChoice};
use salvajocs_core::layout::{Root, RootKind};
use salvajocs_core::storage::Compression;

use crate::first_run::Connect;
use crate::job::Job;
use crate::state::State;
use crate::theme::{self, icons};
use crate::widgets::compression::CompressionField;
use crate::widgets::{form_grid, labelled_edit};

mod launchers;
mod retention;

use launchers::LaunchersSection;
use retention::RetentionSection;

/// The roots offered in the picker, in order.
const ROOT_KINDS: [RootKind; 3] = [RootKind::Steam, RootKind::Other, RootKind::WinePrefix];

fn root_kind_label(kind: RootKind) -> &'static str {
    match kind {
        RootKind::Steam => "Steam library",
        RootKind::Other => "Games folder",
        RootKind::WinePrefix => "Wine prefix",
        _ => "Other root",
    }
}

fn password_mode_label(mode: PasswordMode) -> &'static str {
    match mode {
        PasswordMode::None => "None",
        PasswordMode::Keyring => "Stored in the system keyring",
        PasswordMode::Ask => "Asked once per session",
        _ => "Other",
    }
}

/// The theme choices, in the order the switch shows them.
const THEMES: [(ThemeChoice, &str); 3] = [
    (ThemeChoice::System, "System"),
    (ThemeChoice::Light, "Light"),
    (ThemeChoice::Dark, "Dark"),
];

/// The state of the Settings tab.
pub struct SettingsTab {
    connect: Option<Connect>,
    compression: Option<CompressionField>,
    compression_job: Option<Job<Compression>>,
    compression_error: Option<String>,
    retention: RetentionSection,
    launchers: LaunchersSection,
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
            retention: RetentionSection::default(),
            launchers: LaunchersSection::default(),
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
        self.retention.reset();
    }

    /// Whether a repository change, compression change, retention save, keyring cleanup or
    /// folder picker runs.
    pub(crate) fn busy(&self) -> bool {
        self.connect.as_ref().is_some_and(Connect::busy)
            || self.compression_job.is_some()
            || self.retention.busy()
            || self.cleanup.is_some()
            || self.root_picker.is_some()
    }

    /// Whether a repository creation, compression change or retention save runs, which closing
    /// the app would cut short.
    pub(crate) fn writing(&mut self) -> bool {
        self.connect.as_mut().is_some_and(Connect::finishing)
            || self.compression_job.is_some()
            || self.retention.busy()
    }

    /// Collects the finished compression change, folder picker and keyring cleanup. The app
    /// calls it every frame, whichever tab is in front, so a finished job does not stay running.
    pub(crate) fn poll(&mut self, state: &mut State) {
        if let Some(job) = &mut self.compression_job
            && let Some(session) = &mut state.session
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
        if let Some(job) = &mut self.root_picker
            && let Some(result) = job.take_result()
        {
            self.root_picker = None;
            if let Ok(Some(folder)) = result {
                self.root_path = folder.to_string_lossy().into_owned();
            }
        }
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
    }

    /// Draws the tab. Returns whether the user connected a different repository.
    pub fn ui(&mut self, ui: &mut Ui, state: &mut State) -> bool {
        self.poll(state);
        theme::page(ui, |ui| {
            let adopted = self.repository_ui(ui, state);
            ui.add_space(16.0);
            self.compression_ui(ui, state);
            ui.add_space(16.0);
            self.retention.ui(ui, state);
            ui.add_space(16.0);
            self.roots_ui(ui, state);
            appearance_ui(ui, state);
            updates_ui(ui, state);
            self.cleanup_ui(ui, state);
            adopted
        })
    }

    fn repository_ui(&mut self, ui: &mut Ui, state: &mut State) -> bool {
        ui.heading("Repository");
        theme::group(ui, |ui| {
            if let Some(config) = &state.config {
                ui.label(format!("Current: {}", config.repository));
                ui.label(format!(
                    "Encryption password: {}",
                    password_mode_label(config.password)
                ));
                theme::row_separator(ui);
            }
            let connect = self.connect.get_or_insert_with(|| match &state.config {
                Some(config) => Connect::editing(config, &state.credentials),
                None => Connect::first_run(),
            });
            match connect.ui(ui, &state.env) {
                Some(connected) => {
                    state.adopt(connected, ui.ctx());
                    self.reset();
                    true
                }
                None => false,
            }
        })
    }

    fn compression_ui(&mut self, ui: &mut Ui, state: &mut State) {
        ui.heading("Compression");
        theme::group(ui, |ui| self.compression_group_ui(ui, state));
    }

    fn compression_group_ui(&mut self, ui: &mut Ui, state: &mut State) {
        let Some(session) = &mut state.session else {
            ui.label("The repository is not open.");
            return;
        };
        let field = self
            .compression
            .get_or_insert_with(|| CompressionField::new(session.compression));
        form_grid("repository_compression").show(ui, |ui| {
            field.ui(
                ui,
                "Compression",
                "Applies to data written from now on. Higher levels are smaller and slower.",
            );
        });
        if let Some(error) = &self.compression_error {
            let color = ui.visuals().error_fg_color;
            theme::status(ui, icons::X_CIRCLE, color, error);
        }
        let new = field.change_from(session.compression);
        let apply = ui.add_enabled(
            new.is_some() && self.compression_job.is_none(),
            egui::Button::new("Apply compression"),
        );
        if let (true, Some(new)) = (apply.clicked(), new) {
            let storage = session.storage.clone();
            self.compression_job = Some(Job::spawn(ui.ctx(), move || {
                let mut storage = storage
                    .lock()
                    .map_err(|_| anyhow!("the repository lock is poisoned; restart the app"))?;
                storage.set_compression(new)?;
                Ok(storage.compression())
            }));
        }
    }

    fn roots_ui(&mut self, ui: &mut Ui, state: &mut State) {
        ui.heading("Roots");
        theme::group(ui, |ui| {
            self.roots_group_ui(ui, state);
            if cfg!(target_os = "linux") {
                self.launchers.ui(ui, state);
            }
        });
    }

    fn roots_group_ui(&mut self, ui: &mut Ui, state: &mut State) {
        let Some(config) = &mut state.config else {
            ui.label("Choose a repository first.");
            return;
        };
        let mut changed = false;
        let mut remove = None;
        if config.roots.is_empty() {
            ui.weak(
                "No roots yet. salvajocs finds the default Steam folders by itself. Add other \
                 Steam libraries, games folders or Wine prefixes here.",
            );
        }
        for (index, root) in config.roots.iter().enumerate() {
            if index > 0 {
                theme::row_separator(ui);
            }
            theme::row(
                ui,
                |ui| {
                    let kind = root_kind_label(root.kind);
                    ui.label(format!("{} ({kind})", root.path.display()));
                },
                |ui| {
                    if ui.button("Remove").clicked() {
                        remove = Some(index);
                    }
                },
            );
        }
        theme::row_separator(ui);
        if let Some(index) = remove {
            config.roots.remove(index);
            changed = true;
        }
        form_grid("add_root").show(ui, |ui| {
            let label = ui.label("Kind");
            egui::ComboBox::from_id_salt("root_kind")
                .selected_text(root_kind_label(self.root_kind))
                .show_ui(ui, |ui| {
                    theme::pill_list(ui);
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
            config
                .roots
                .push(Root::new(PathBuf::from(path), self.root_kind));
            self.root_path.clear();
            changed = true;
        }
        if changed {
            state.save_config(ui.ctx());
        }
    }

    /// Offers to delete the keyring entries of the repository that was just replaced.
    fn cleanup_ui(&mut self, ui: &mut Ui, state: &mut State) {
        if state.stale.is_none() {
            return;
        }
        ui.add_space(16.0);
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

/// The theme switch. It applies at once and is saved with the config.
fn appearance_ui(ui: &mut Ui, state: &mut State) {
    let Some(config) = &mut state.config else {
        return;
    };
    ui.add_space(16.0);
    ui.heading("Appearance");
    let changed = theme::group(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Theme");
            THEMES
                .into_iter()
                .map(|(choice, label)| ui.selectable_value(&mut config.theme, choice, label))
                .fold(false, |changed, response| changed | response.changed())
        })
        .inner
    });
    if changed {
        ui.ctx().set_theme(theme::preference(config.theme));
        state.save_config(ui.ctx());
    }
}

/// The update check toggle, on builds that check at all.
fn updates_ui(ui: &mut Ui, state: &mut State) {
    let Some(config) = &mut state.config else {
        return;
    };
    if state.env.check_update.is_none() {
        return;
    }
    ui.add_space(16.0);
    ui.heading("Updates");
    let checkbox = theme::group(ui, |ui| {
        ui.checkbox(&mut config.check_updates, "Check for updates at start")
    });
    if checkbox.changed() {
        state.save_config(ui.ctx());
    }
}

/// Gives the tab jobs that run until the returned sender is dropped.
#[cfg(test)]
impl SettingsTab {
    pub(crate) fn hold_compression(&mut self, ctx: &egui::Context) -> std::sync::mpsc::Sender<()> {
        let (release, job) = crate::job::held(ctx);
        self.compression_job = Some(job);
        release
    }

    /// Gives the tab a repository creation that runs until the returned sender is dropped.
    pub(crate) fn hold_finishing(&mut self, ctx: &egui::Context) -> std::sync::mpsc::Sender<()> {
        let mut connect = Connect::first_run();
        let release = connect.hold_finishing(ctx);
        self.connect = Some(connect);
        release
    }
}
