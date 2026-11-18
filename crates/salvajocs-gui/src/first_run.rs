//! Choosing a repository: the first-run dialog and the Settings form share this flow. It
//! probes the location, asks before creating a repository, opens it, and only then saves the
//! config (ADR-0003, ADR-0009).

use std::mem;

use anyhow::Result;
use eframe::egui::{self, Ui};
use salvajocs_core::config::{AppConfig, PasswordMode};
use salvajocs_core::secret::SecretKey;
use salvajocs_core::storage::{Compression, RepoLocation, RepoState, Storage, probe};

use crate::env::{Env, SecretStore};
use crate::job::Job;
use crate::startup::Credentials;
use crate::theme::{self, icons};
use crate::widgets::backend::BackendForm;
use crate::widgets::compression::CompressionField;
use crate::widgets::{field_hint, field_status, form_grid, labelled_edit};

/// A repository the user chose and that is now open. The config is not saved yet:
/// `State::adopt` saves it with the current roots and custom games.
pub struct Connected {
    /// The config of the new repository; its roots and custom games are empty.
    pub config: AppConfig,
    /// The password and secret option values of this session.
    pub credentials: Credentials,
    /// The open repository; boxed because a [`Storage`] is large and travels through a job.
    pub storage: Box<Storage>,
    /// Keyring problems to show; the app still works, the secret is asked each session.
    pub warnings: Vec<String>,
}

/// What the user submitted, kept while the repository is probed.
struct Attempt {
    config: AppConfig,
    credentials: Credentials,
    remember: bool,
    compression: Compression,
    location: RepoLocation,
}

enum Step {
    /// Waiting for input; carries the last failure to show.
    Editing(Option<String>),
    Probing(Job<RepoState>, Attempt),
    /// No repository there: waiting for the user to allow creating one.
    Confirm(Attempt),
    Finishing(Job<Connected>),
}

/// The repository form and the state of connecting to what it describes.
pub struct Connect {
    form: BackendForm,
    compression: CompressionField,
    password: String,
    remember: bool,
    submit: &'static str,
    step: Step,
}

impl Connect {
    /// An empty form for the first run.
    pub fn first_run() -> Self {
        Self {
            form: BackendForm::default(),
            compression: CompressionField::new(Compression::Default),
            password: String::new(),
            remember: true,
            submit: "Continue",
            step: Step::Editing(None),
        }
    }

    /// A form showing the current repository.
    pub fn editing(config: &AppConfig, credentials: &Credentials) -> Self {
        Self {
            form: BackendForm::from_config(config, &credentials.options),
            password: credentials.password.clone(),
            remember: config.password == PasswordMode::Keyring,
            submit: "Apply",
            ..Self::first_run()
        }
    }

    /// Whether a probe or a repository creation is running.
    pub(crate) fn busy(&self) -> bool {
        matches!(self.step, Step::Probing(..) | Step::Finishing(_))
    }

    /// Draws the form and the current step. Returns the repository once it is open.
    pub fn ui(&mut self, ui: &mut Ui, env: &Env) -> Option<Connected> {
        let editing = matches!(self.step, Step::Editing(_));
        ui.add_enabled_ui(editing, |ui| {
            form_grid("connect_form").show(ui, |ui| {
                self.form.ui(ui, env);
                labelled_edit(ui, "Encryption password", &mut self.password, true);
                ui.end_row();
                ui.label("");
                field_hint(
                    ui,
                    "Encrypts the backups. Not the login of your server or storage account.",
                );
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.remember, "Remember in the system keyring");
                ui.end_row();
                if self.password.is_empty() {
                    ui.label("");
                    let warn = ui.visuals().warn_fg_color;
                    field_status(
                        ui,
                        icons::WARNING,
                        warn,
                        "No encryption password: the repository will not be encrypted.",
                    );
                    ui.end_row();
                }
                self.compression.ui(
                    ui,
                    "Compression (new repository)",
                    "Used only when a new repository is created.",
                );
            });
        });
        let compression = self.compression.value().ok();

        match mem::replace(&mut self.step, Step::Editing(None)) {
            Step::Editing(error) => {
                if let Some(error) = &error {
                    let color = ui.visuals().error_fg_color;
                    theme::status(ui, icons::X_CIRCLE, color, error);
                }
                let ready = self.form.is_complete() && compression.is_some();
                let go =
                    theme::row_end(ui, |ui| theme::primary(ui, ready, None, self.submit)).clicked();
                self.step = match compression {
                    Some(compression) if go => self.begin(ui.ctx(), compression),
                    _ => Step::Editing(error),
                };
                None
            }
            Step::Probing(mut job, attempt) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking the repository");
                });
                self.step = match job.take_result() {
                    None => Step::Probing(job, attempt),
                    Some(Err(err)) => Step::Editing(Some(format!("{err:#}"))),
                    Some(Ok(state)) => next_after_probe(state, attempt, ui.ctx(), env),
                };
                None
            }
            Step::Confirm(attempt) => {
                ui.label(format!(
                    "Create a new repository at {}?",
                    attempt.location.repository
                ));
                let mut choice = None;
                theme::row_end(ui, |ui| {
                    if ui.button("Back").clicked() {
                        choice = Some(false);
                    }
                    if theme::primary(ui, true, None, "Create").clicked() {
                        choice = Some(true);
                    }
                });
                self.step = match choice {
                    Some(true) => finishing(ui.ctx(), env, attempt, true),
                    Some(false) => Step::Editing(None),
                    None => Step::Confirm(attempt),
                };
                None
            }
            Step::Finishing(mut job) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Opening the repository");
                });
                match job.take_result() {
                    None => self.step = Step::Finishing(job),
                    Some(Err(err)) => self.step = Step::Editing(Some(format!("{err:#}"))),
                    Some(Ok(connected)) => return Some(connected),
                }
                None
            }
        }
    }

    /// Reads the form and starts probing its location.
    fn begin(&self, ctx: &egui::Context, compression: Compression) -> Step {
        let password = match (self.password.is_empty(), self.remember) {
            (true, _) => PasswordMode::None,
            (false, true) => PasswordMode::Keyring,
            (false, false) => PasswordMode::Ask,
        };
        let config = AppConfig {
            repository: self.form.repository(),
            repository_options: self.form.options(),
            password,
            roots: Vec::new(),
            custom_games: Vec::new(),
            check_updates: true,
            ignored_prefixes: Vec::new(),
        };
        let credentials = Credentials {
            password: self.password.clone(),
            options: self.form.secrets(),
        };
        let location = match config.location(&credentials.options) {
            Ok(location) => location,
            Err(err) => return Step::Editing(Some(err.to_string())),
        };
        let (probe_location, probe_password) = (location.clone(), credentials.password.clone());
        let job = Job::spawn(ctx, move || Ok(probe(&probe_location, &probe_password)));
        Step::Probing(
            job,
            Attempt {
                config,
                credentials,
                remember: self.remember,
                compression,
                location,
            },
        )
    }
}

/// Decides what a probe result means for the form (ADR-0009): open it, ask before creating, or
/// show why not.
fn next_after_probe(state: RepoState, attempt: Attempt, ctx: &egui::Context, env: &Env) -> Step {
    match state {
        RepoState::Ready => finishing(ctx, env, attempt, false),
        RepoState::Missing => Step::Confirm(attempt),
        RepoState::WrongPassword => Step::Editing(Some("Wrong encryption password.".to_string())),
        RepoState::Error(message) => Step::Editing(Some(message)),
    }
}

fn finishing(ctx: &egui::Context, env: &Env, attempt: Attempt, create: bool) -> Step {
    let env = env.clone();
    Step::Finishing(Job::spawn(ctx, move || finish(&env, attempt, create)))
}

/// Creates or opens the repository, then keeps its secrets. Nothing is written to the config
/// before the repository opened, so a failure leaves nothing that points at it.
fn finish(env: &Env, attempt: Attempt, create: bool) -> Result<Connected> {
    let Attempt {
        mut config,
        credentials,
        remember,
        compression,
        location,
    } = attempt;
    let storage = Box::new(if create {
        Storage::init(&location, &credentials.password, compression)?
    } else {
        Storage::open(&location, &credentials.password)?
    });
    let warnings = persist_secrets(env.secrets.as_ref(), &mut config, &credentials, remember);
    Ok(Connected {
        config,
        credentials,
        storage,
        warnings,
    })
}

/// Stores the password and the secret options in the keyring when `remember` is set, or removes
/// what an earlier "remember" left. A secret the store refuses is asked for each session, and a
/// refused password turns the config's mode into `Ask`. Returns the warnings to show.
fn persist_secrets(
    store: &dyn SecretStore,
    config: &mut AppConfig,
    credentials: &Credentials,
    remember: bool,
) -> Vec<String> {
    let scope = config.secret_scope();
    let mut entries = vec![(SecretKey::Password, credentials.password.as_str())];
    for option in config.repository_options.iter().filter(|o| o.secret) {
        if let Some(value) = credentials.options.get(&option.key) {
            entries.push((SecretKey::Option(option.key.clone()), value));
        }
    }
    let mut warnings = Vec::new();
    for (key, value) in entries {
        let keep =
            remember && (key != SecretKey::Password || config.password == PasswordMode::Keyring);
        if keep {
            if let Err(err) = store.set(&scope, &key, value) {
                if key == SecretKey::Password {
                    config.password = PasswordMode::Ask;
                }
                warnings.push(format!(
                    "Could not store a secret in the keyring ({err}); it will be asked each session."
                ));
            }
        } else {
            // Best effort: a missing store has nothing to forget.
            let _ = store.delete(&scope, &key);
        }
    }
    warnings
}

#[cfg(test)]
mod tests;
