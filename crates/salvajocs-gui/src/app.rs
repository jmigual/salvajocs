//! The window: startup stages, the title bar with the tab strip, the status bar and the dialogs
//! in front of the tabs.

use std::mem;
use std::sync::Arc;

use eframe::egui::{self, Margin, Rect, RichText, Ui};
use salvajocs_core::config::{AppConfig, PasswordMode, ThemeChoice};
use salvajocs_core::storage::{Storage, StorageError};

use crate::env::Env;
use crate::first_run::{Connect, failure_message};
use crate::job::Job;
use crate::prompt::SecretPrompt;
use crate::startup::{self, Credentials, ManifestLoad, Resolved};
use crate::state::{Session, State};
use crate::status_bar;
use crate::tabs::{
    Tab, backup::BackupTab, custom_games::CustomGamesTab, restore::RestoreTab,
    settings::SettingsTab,
};
use crate::theme::{self, icons};
use crate::title_bar;
use crate::update::Updater;

/// Where startup stands. Each stage ends in `Ready`, `RepositoryMissing` or `OpenFailed`.
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
    /// The configured repository does not exist; carries its location.
    RepositoryMissing(String),
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
    /// The close-while-running dialog is open.
    confirm_close: bool,
    /// The user chose to close despite a running job, so the next close request goes through.
    closing: bool,
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
            confirm_close: false,
            closing: false,
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

    /// Whether a job runs that writes to the repository, the config, the keyring or the games'
    /// files, and that closing would cut short. Reads and waits, such as scans and the manifest load, are
    /// not in it, and neither are the pending file writes, which `on_exit` flushes.
    fn writing(&mut self) -> bool {
        let creating = match &mut self.stage {
            Stage::FirstRun(connect) => connect.finishing(),
            _ => false,
        };
        creating
            || self.state.busy()
            || self.backup.writing()
            || self.restore.writing()
            || self.settings.writing()
    }

    /// Collects the finished jobs of the state and every tab, whichever tab is in front.
    fn poll_jobs(&mut self, ctx: &egui::Context) {
        self.state.poll(ctx);
        self.backup.poll(ctx, &self.state);
        if self.backup.take_removed() {
            self.restore.drop_listing();
        }
        self.restore
            .poll(ctx, &mut self.state, self.manifest.is_some());
        self.settings.poll(&mut self.state);
    }

    /// Cancels a close request while a job writes and opens the dialog asking what to do. The
    /// title bar's Close and the OS (Alt+F4, taskbar) both arrive here as a request.
    fn guard_close(&mut self, ctx: &egui::Context) {
        let requested = ctx.input(|i| i.viewport().close_requested());
        if requested && !self.closing && self.writing() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.updater.cancel_restart();
            // `ui` draws the dialog and does not run while the window is minimized. This does
            // nothing on Wayland or for an occluded window; the dialog waits until it is shown.
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            self.confirm_close = true;
        }
    }

    /// The dialog of a close request during a job: Wait keeps the window, Close anyway ends it.
    fn close_dialog(&mut self, ctx: &egui::Context) {
        let (mut wait, mut close) = (false, false);
        theme::modal(ctx, "confirm_close", 480.0, |ui| {
            ui.heading("A job is still running");
            ui.label("Closing now stops it. Wait for it to finish, or close anyway.");
            let error = ui.visuals().error_fg_color;
            theme::row_end(ui, |ui| {
                wait = ui.button("Wait").clicked();
                close = ui
                    .button(RichText::new("Close anyway").color(error).strong())
                    .clicked();
            });
        });
        if close {
            self.closing = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if wait {
            self.confirm_close = false;
        }
    }

    /// Moves the startup stage forward and draws its dialog.
    fn advance(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let before = mem::discriminant(&self.stage);
        self.stage = match mem::replace(&mut self.stage, Stage::Ready) {
            Stage::Starting => Stage::Starting,
            Stage::FirstRun(mut connect) => {
                let connected = theme::modal(&ctx, "first_run", 560.0, |ui| {
                    self.updater.dialog_ui(ui, &self.state);
                    ui.heading("Welcome to salvajocs");
                    ui.weak("Pick a folder or server for your backups. You can change it later in Settings.");
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
                    let repository = self.state.config.as_ref().map_or("", |c| &c.repository);
                    Stage::Prompt(SecretPrompt::new(found, missing, repository))
                }
            },
            Stage::Prompt(mut prompt) => {
                // Read before the prompt hands its answers over.
                let asks_password = prompt.asks_password();
                match theme::modal(&ctx, "secret_prompt", 560.0, |ui| {
                    self.updater.dialog_ui(ui, &self.state);
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
                Some(Err(err))
                    if matches!(
                        err.downcast_ref::<StorageError>(),
                        Some(StorageError::Missing)
                    ) =>
                {
                    // Settings starts from these, so re-creating keeps the password.
                    self.state.credentials = credentials;
                    self.settings.reset();
                    let repository = self.state.config.as_ref().map_or("", |c| &c.repository);
                    Stage::RepositoryMissing(repository.to_string())
                }
                Some(Err(err)) => {
                    // Settings starts from these, so an Apply that fixes the location or the
                    // login keeps the remembered ones.
                    self.state.credentials = credentials;
                    self.settings.reset();
                    Stage::OpenFailed(failure_message(&err))
                }
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
            stage @ (Stage::ConfigUnreadable(_)
            | Stage::Ready
            | Stage::RepositoryMissing(_)
            | Stage::OpenFailed(_)) => stage,
        };
        // A new stage changes what the other panels show, and they were drawn before it.
        if mem::discriminant(&self.stage) != before {
            ctx.request_repaint();
        }
    }

    fn status_ui(&mut self, ui: &mut Ui) {
        match &self.stage {
            Stage::ConfigUnreadable(error) => {
                theme::status(
                    ui,
                    icons::X_CIRCLE,
                    ui.visuals().error_fg_color,
                    format!("The config file in {error} cannot be read"),
                );
            }
            Stage::Resolving(_) | Stage::Opening(..) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Opening the repository");
                });
            }
            Stage::RepositoryMissing(repository) => {
                let repository = repository.clone();
                // The message is long, so the buttons get their own row to stay on screen.
                theme::status(
                    ui,
                    icons::X_CIRCLE,
                    ui.visuals().error_fg_color,
                    format!(
                        "No repository at {repository}. It may have been moved or deleted, or its \
                         drive is not mounted. Connect it and click Retry, or open Settings to \
                         create a new one there or choose another location."
                    ),
                );
                ui.horizontal(|ui| {
                    if ui.button("Open Settings").clicked() {
                        self.tab = Tab::Settings;
                    }
                    if ui.button("Retry").clicked() {
                        self.stage = self.resolve(ui.ctx());
                    }
                });
            }
            Stage::OpenFailed(error) => {
                let error = error.clone();
                ui.horizontal(|ui| {
                    theme::status(
                        ui,
                        icons::X_CIRCLE,
                        ui.visuals().error_fg_color,
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
            theme::status(
                ui,
                icons::X_CIRCLE,
                ui.visuals().error_fg_color,
                format!("The game manifest could not be updated: {error}"),
            );
        }
        let mut dismissed = None;
        for (index, notice) in self.state.notices.iter().enumerate() {
            ui.horizontal(|ui| {
                theme::status(ui, icons::WARNING, ui.visuals().warn_fg_color, notice);
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

/// The tab strip: the selected tab is a pill, the others show a frame only when hovered.
fn tabs_ui(current: &mut Tab, ui: &mut Ui) {
    ui.scope(|ui| {
        theme::pill_list(ui);
        for tab in Tab::ALL {
            let selected = *current == tab;
            let label = theme::semibold(tab.label());
            let button = egui::Button::selectable(selected, (theme::icon(tab.icon()), label));
            if ui.add(button).clicked() {
                *current = tab;
            }
        }
    });
}

impl eframe::App for App {
    // Not `ui`: eframe runs `logic` for a minimized window too, where `ui` never runs.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Polled first: a job that ended while the window was minimized must not hold the close.
        self.poll_jobs(ctx);
        self.guard_close(ctx);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !mem::replace(&mut self.started, true) {
            // Here rather than in eframe's creation hook, which the test harnesses do not run.
            theme::apply(&ctx);
            // System keeps egui's own preference, which a screenshot harness sets to a fixed theme.
            if let Some(config) = &self.state.config
                && config.theme != ThemeChoice::System
            {
                ctx.set_theme(theme::preference(config.theme));
            }
            // `ui` was styled before `apply`, and the panels below inherit its style.
            ui.set_style(ctx.global_style());
            self.start(&ctx);
        }
        self.poll_manifest();
        self.poll_jobs(&ctx);

        let margin = Margin::symmetric(16, 8);
        let frame = egui::Frame::side_top_panel(ui.style()).inner_margin(margin);
        let (mut bar, mut buttons) = (Rect::NOTHING, Rect::NOTHING);
        egui::Panel::top("tabs").frame(frame).show(ui, |ui| {
            bar = title_bar::drag_area(ui, margin);
            self.updater.poll(&mut self.state);
            ui.horizontal(|ui| {
                title_bar::app_icon(ui);
                tabs_ui(&mut self.tab, ui);
                theme::row_end(ui, |ui| {
                    // The sizing pass runs first, so the slot of the real pass is kept.
                    buttons = title_bar::reserve_buttons(ui);
                });
            });
            self.status_ui(ui);
        });

        // Decided once a frame: the tab's poll may bring a scan report before the bar is drawn.
        let backup_bar = self.tab == Tab::Backup && self.backup.has_bar(&self.state);
        let bottom = status_bar::reserve(ui, backup_bar);

        // No margin: the tabs center their own column, and side and bottom bars reach the edges.
        let adopted = egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(0))
            .show(ui, |ui| match (&self.stage, self.tab) {
                (Stage::ConfigUnreadable(_), _) => false,
                (_, Tab::Backup) => {
                    self.backup.ui(ui, &mut self.state, self.manifest.is_some());
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
        // The dialog of these stages covers the bar, so it shows the notice instead.
        let dialog_open = matches!(self.stage, Stage::FirstRun(_) | Stage::Prompt(_));
        let (updater, state, loading) = (&mut self.updater, &self.state, self.manifest.is_some());
        let start = |ui: &mut Ui| {
            status_bar::status_text(ui, |ui| {
                match (&state.manifest, loading) {
                    (Some(manifest), _) => {
                        ui.weak(format!("Manifest: {} games", manifest.games.len()))
                    }
                    (None, true) => ui.weak("Manifest: loading"),
                    (None, false) => ui.weak("Manifest: not available"),
                };
            });
        };
        let end = |ui: &mut Ui| {
            if !dialog_open {
                updater.ui(ui, state);
            }
        };
        if backup_bar {
            self.backup.bar_ui(ui, bottom, state, loading, start, end);
        } else {
            status_bar::show(ui, bottom, start, end);
        }
        self.advance(ui);
        // After the tabs, so a job their input just started counts for a close in the same frame.
        self.guard_close(&ctx);
        // A job that ended while the dialog was open leaves nothing to ask.
        self.confirm_close &= self.writing();
        if self.confirm_close {
            self.close_dialog(&ctx);
        }
        title_bar::controls(ui, bar, buttons);
        // Last, so a job started in this frame holds the update back.
        let busy = self.busy();
        self.updater.install_when_idle(&ctx, &mut self.state, busy);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Err(err) = self.backup.flush(&self.state.env.config_dir) {
            eprintln!("salvajocs: could not write the unsaved choices: {err:#}");
        }
        self.updater.restart(&self.state);
    }
}

#[cfg(test)]
mod tests;
