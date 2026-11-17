//! The window: startup stages, the tab strip and the dialogs in front of the tabs.

use std::mem;
use std::sync::Arc;

use eframe::egui::{self, Color32, Ui};
use salvajocs_core::config::{AppConfig, PasswordMode};
use salvajocs_core::storage::{Storage, StorageError};

use crate::env::Env;
use crate::first_run::Connect;
use crate::job::Job;
use crate::prompt::SecretPrompt;
use crate::startup::{self, Credentials, ManifestLoad, Resolved};
use crate::state::{Session, State};
use crate::tabs::{
    Tab, backup::BackupTab, custom_games::CustomGamesTab, restore::RestoreTab,
    settings::SettingsTab,
};
use crate::update::Updater;

/// Where startup stands. Each stage ends in `Ready` or `OpenFailed`.
enum Stage {
    /// Before the first frame: jobs need the egui context.
    Starting,
    /// The config file exists but cannot be read. Every tab stays empty, so nothing can
    /// overwrite the file.
    ConfigUnreadable(String),
    /// No config yet: the first-run dialog is open.
    FirstRun(Box<Connect>),
    /// Looking for the secrets in the keyring.
    Resolving(Job<Resolved>),
    /// Asking for the secrets the keyring did not hold.
    Prompt(SecretPrompt),
    /// Opening the repository. The flag asks for the typed password to go to the keyring once it
    /// worked.
    Opening(Job<Box<Storage>>, Credentials, bool),
    /// The repository is open.
    Ready,
    /// The repository could not be opened; carries why.
    OpenFailed(String),
}

/// The whole application.
pub struct App {
    state: State,
    stage: Stage,
    tab: Tab,
    backup: BackupTab,
    restore: RestoreTab,
    settings: SettingsTab,
    custom_games: CustomGamesTab,
    started: bool,
    manifest: Option<Job<ManifestLoad>>,
    manifest_error: Option<String>,
    updater: Updater,
}

impl App {
    /// An app that starts from the config in `env`. The first frame starts the background work.
    pub fn new(env: Env) -> Self {
        let mut state = State::new(env);
        let stage = match AppConfig::load(&state.env.config_dir) {
            Ok(Some(config)) => {
                state.config = Some(config);
                Stage::Starting
            }
            Ok(None) => Stage::FirstRun(Box::new(Connect::first_run())),
            Err(err) => {
                Stage::ConfigUnreadable(format!("{} ({err})", state.env.config_dir.display()))
            }
        };
        Self {
            state,
            stage,
            tab: Tab::default(),
            backup: BackupTab::default(),
            restore: RestoreTab::default(),
            settings: SettingsTab::default(),
            custom_games: CustomGamesTab::default(),
            started: false,
            manifest: None,
            manifest_error: None,
            updater: Updater::default(),
        }
    }

    /// Brings a tab to the front.
    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
    }

    /// Starts the manifest load, the update check and, for an existing config, the repository
    /// lookup. Needs the egui context, so it runs in the first frame.
    fn start(&mut self, ctx: &egui::Context) {
        let (cache_dir, refresh) = (
            self.state.env.cache_dir.clone(),
            self.state.env.refresh_manifest,
        );
        self.manifest = Some(Job::spawn(ctx, move || {
            Ok(startup::load_manifest(&cache_dir, refresh))
        }));
        self.updater.start(ctx, &self.state);
        if matches!(self.stage, Stage::Starting) {
            self.stage = self.resolve(ctx);
        }
    }

    fn resolve(&self, ctx: &egui::Context) -> Stage {
        let (env, config) = (self.state.env.clone(), self.state.config.clone());
        Stage::Resolving(Job::spawn(ctx, move || {
            let config = config.ok_or_else(|| anyhow::anyhow!("no config to resolve"))?;
            Ok(startup::resolve(&env, &config))
        }))
    }

    fn open(&self, ctx: &egui::Context, credentials: Credentials, remember: bool) -> Stage {
        let config = self.state.config.clone();
        let opening = credentials.clone();
        let job = Job::spawn(ctx, move || {
            let config = config.ok_or_else(|| anyhow::anyhow!("no config to open"))?;
            startup::open(&config, &opening)
        });
        Stage::Opening(job, credentials, remember)
    }

    fn poll_manifest(&mut self) {
        let Some(job) = &mut self.manifest else {
            return;
        };
        let Some(result) = job.take_result() else {
            return;
        };
        self.manifest = None;
        match result {
            Ok(load) => {
                self.state.manifest = load.manifest.map(Arc::new);
                self.manifest_error = load.error;
            }
            Err(err) => self.manifest_error = Some(format!("{err:#}")),
        }
    }

    /// Whether any background job runs; closing the app for an update would cut it short.
    fn busy(&self) -> bool {
        let stage = match &self.stage {
            Stage::Resolving(_) | Stage::Opening(..) => true,
            Stage::FirstRun(connect) => connect.busy(),
            _ => false,
        };
        stage
            || self.manifest.is_some()
            || self.state.busy()
            || self.backup.busy()
            || self.restore.busy()
            || self.settings.busy()
    }

    /// Moves the startup stage forward and draws its dialog.
    fn advance(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let before = mem::discriminant(&self.stage);
        self.stage = match mem::replace(&mut self.stage, Stage::Ready) {
            Stage::Starting => Stage::Starting,
            Stage::FirstRun(mut connect) => {
                let connected = dialog(&ctx, "first_run", |ui| {
                    ui.heading("Welcome to salvajocs");
                    ui.label("Choose where backups are saved, and restored from.");
                    connect.ui(ui, &self.state.env)
                });
                match connected {
                    Some(connected) => {
                        self.state.adopt(connected, &ctx);
                        self.settings.reset();
                        Stage::Ready
                    }
                    None => Stage::FirstRun(connect),
                }
            }
            Stage::Resolving(mut job) => match job.take_result() {
                None => Stage::Resolving(job),
                Some(Err(err)) => Stage::OpenFailed(format!("{err:#}")),
                Some(Ok(Resolved { found, missing })) if missing.is_empty() => {
                    self.open(&ctx, found, false)
                }
                Some(Ok(Resolved { found, missing })) => {
                    Stage::Prompt(SecretPrompt::new(found, missing))
                }
            },
            Stage::Prompt(mut prompt) => {
                // Read before the prompt hands its answers over.
                let asks_password = prompt.asks_password();
                match dialog(&ctx, "secret_prompt", |ui| {
                    ui.heading("Repository secrets");
                    prompt.ui(ui)
                }) {
                    Some(credentials) => {
                        // A password the keyring lacked or had wrong belongs back in it.
                        let remember = asks_password
                            && self
                                .state
                                .config
                                .as_ref()
                                .is_some_and(|config| config.password == PasswordMode::Keyring);
                        self.open(&ctx, credentials, remember)
                    }
                    None => Stage::Prompt(prompt),
                }
            }
            Stage::Opening(mut job, credentials, remember) => match job.take_result() {
                None => Stage::Opening(job, credentials, remember),
                Some(Err(err))
                    if matches!(
                        err.downcast_ref::<StorageError>(),
                        Some(StorageError::WrongPassword)
                    ) =>
                {
                    Stage::Prompt(SecretPrompt::wrong_password(credentials))
                }
                Some(Err(err)) => Stage::OpenFailed(format!("{err:#}")),
                Some(Ok(storage)) => {
                    self.state.credentials = credentials;
                    if remember {
                        self.state.remember_password(&ctx);
                    }
                    self.state.session = Some(Session {
                        compression: storage.compression(),
                        storage: Arc::new(std::sync::Mutex::new(*storage)),
                    });
                    self.settings.reset();
                    Stage::Ready
                }
            },
            stage @ (Stage::ConfigUnreadable(_) | Stage::Ready | Stage::OpenFailed(_)) => stage,
        };
        // A new stage changes what the other panels show, and they were drawn before it.
        if mem::discriminant(&self.stage) != before {
            ctx.request_repaint();
        }
    }

    fn status_ui(&mut self, ui: &mut Ui) {
        match &self.stage {
            Stage::ConfigUnreadable(error) => {
                ui.colored_label(
                    Color32::LIGHT_RED,
                    format!("The config file in {error} cannot be read"),
                );
            }
            Stage::Resolving(_) | Stage::Opening(..) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Opening the repository");
                });
            }
            Stage::OpenFailed(error) => {
                let error = error.clone();
                ui.horizontal(|ui| {
                    ui.colored_label(
                        Color32::LIGHT_RED,
                        format!("The repository could not be opened: {error}"),
                    );
                    if ui.button("Retry").clicked() {
                        self.stage = self.resolve(ui.ctx());
                    }
                });
            }
            _ => {}
        }
        if let Some(error) = &self.manifest_error {
            ui.colored_label(
                Color32::LIGHT_RED,
                format!("The game manifest could not be updated: {error}"),
            );
        }
        let mut dismissed = None;
        for (index, notice) in self.state.notices.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.colored_label(Color32::YELLOW, notice);
                if ui.button("Dismiss").clicked() {
                    dismissed = Some(index);
                }
            });
        }
        if let Some(index) = dismissed {
            self.state.notices.remove(index);
        }
    }
}

/// Shows `content` in a modal dialog.
fn dialog<R>(ctx: &egui::Context, id: &str, content: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Modal::new(egui::Id::unique(id))
        .show(ctx, |ui| {
            ui.set_width(480.0);
            content(ui)
        })
        .inner
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !mem::replace(&mut self.started, true) {
            self.start(&ctx);
        }
        self.poll_manifest();
        self.state.poll(&ctx);

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for tab in Tab::ALL {
                    ui.selectable_value(&mut self.tab, tab, tab.label());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match (&self.state.manifest, &self.manifest) {
                        (Some(manifest), _) => {
                            ui.weak(format!("Manifest: {} games", manifest.games.len()))
                        }
                        (None, Some(_)) => ui.weak("Manifest: loading"),
                        (None, None) => ui.weak("Manifest: not available"),
                    };
                    self.updater.ui(ui, &mut self.state);
                });
            });
            self.status_ui(ui);
            ui.add_space(4.0);
        });

        let adopted = egui::CentralPanel::default()
            .show(ui, |ui| match (&self.stage, self.tab) {
                (Stage::ConfigUnreadable(_), _) => false,
                (_, Tab::Backup) => {
                    self.backup.ui(ui, &self.state, self.manifest.is_some());
                    false
                }
                (_, Tab::Restore) => {
                    self.restore
                        .ui(ui, &mut self.state, self.manifest.is_some());
                    false
                }
                (_, Tab::CustomGames) => {
                    self.custom_games.ui(ui, &mut self.state);
                    false
                }
                (_, Tab::Settings) => self.settings.ui(ui, &mut self.state),
            })
            .inner;
        if adopted {
            // A repository chosen in Settings replaces whatever startup was still doing.
            self.stage = Stage::Ready;
            ctx.request_repaint();
        }
        self.advance(ui);
        // Last, so a job started in this frame holds the update back.
        let busy = self.busy();
        self.updater.install_when_idle(&ctx, &mut self.state, busy);
    }
}

#[cfg(test)]
mod tests;
