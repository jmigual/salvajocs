//! Choosing a repository: the first-run dialog and the Settings form share this flow. It
//! probes the location, asks before creating a repository, opens it, and only then saves the
//! config (ADR-0003, ADR-0009).

use std::mem;

use anyhow::Result;
use eframe::egui::{self, Ui};
use salvajocs_core::config::{AppConfig, PasswordMode};
use salvajocs_core::secret::SecretKey;
use salvajocs_core::storage::{Compression, RepoLocation, RepoState, Storage, StorageError, probe};

use crate::env::{Env, SecretStore};
use crate::job::Job;
use crate::startup::Credentials;
use crate::theme::{self, icons};
use crate::widgets::backend::BackendForm;
use crate::widgets::compression::CompressionField;
use crate::widgets::{field_hint, field_status, form_grid, labelled_edit};

/// What the app says when the storage server refuses the login (HTTP 401 or 403).
const LOGIN_REFUSED: &str = "The server refused the login. Check the user name and password.";

/// What to show for a failure to open or create a repository: its error chain, or for a
/// refused login, what to check.
pub(crate) fn failure_message(err: &anyhow::Error) -> String {
    match err.downcast_ref::<StorageError>() {
        Some(StorageError::LoginRefused) => LOGIN_REFUSED.to_string(),
        _ => format!("{err:#}"),
    }
}

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
    location: RepoLocation,
}

enum Step {
    /// Waiting for input; carries the last failure to show.
    Editing(Option<String>),
    Probing(Job<RepoState>, Attempt),
    /// No repository there: waiting for the user to allow creating one with a compression.
    Confirm(Attempt),
    /// Opening or creating the repository; the flag is set when it is created.
    Finishing(Job<Connected>, bool),
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
            remember: config.password == PasswordMode::Keyring || credentials.options_remembered,
            submit: "Apply",
            ..Self::first_run()
        }
    }

    /// Whether a probe or a repository creation is running.
    pub(crate) fn busy(&self) -> bool {
        matches!(self.step, Step::Probing(..) | Step::Finishing(..))
    }

    /// Whether the repository is being created or opened, which also stores its secrets in the
    /// keyring, so closing the app would leave it half done. A finished job does not count, even
    /// while its result waits for a frame that draws the form.
    pub(crate) fn finishing(&mut self) -> bool {
        match &mut self.step {
            Step::Finishing(job, _) => job.is_running(),
            _ => false,
        }
    }

    /// Draws the form and the current step. Returns the repository once it is open.
    pub fn ui(&mut self, ui: &mut Ui, env: &Env) -> Option<Connected> {
        // Creating a repository replaces the form instead of just disabling it: otherwise the
        // question and the form look alike.
        if !matches!(self.step, Step::Confirm(_) | Step::Finishing(_, true)) {
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
                    ui.checkbox(
                        &mut self.remember,
                        "Remember passwords in the system keyring",
                    );
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
                });
            });
        }

        match mem::replace(&mut self.step, Step::Editing(None)) {
            Step::Editing(error) => {
                if let Some(error) = &error {
                    let color = ui.visuals().error_fg_color;
                    theme::status(ui, icons::X_CIRCLE, color, error);
                }
                let ready = self.form.is_complete();
                let go =
                    theme::row_end(ui, |ui| theme::primary(ui, ready, None, self.submit)).clicked();
                self.step = if go {
                    self.begin(ui.ctx())
                } else {
                    Step::Editing(error)
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
                    "There is no repository at {}.",
                    attempt.location.repository
                ));
                ui.label("Do you want to create one?");
                ui.weak(if attempt.credentials.password.is_empty() {
                    "It will not be encrypted."
                } else {
                    "It will be encrypted with the encryption password you typed."
                });
                if !attempt.location.proves_absence() {
                    let warn = ui.visuals().warn_fg_color;
                    theme::status(
                        ui,
                        icons::WARNING,
                        warn,
                        "A server also answers \"no repository\" when it refuses your login. \
                         Check the credentials before you create one.",
                    );
                }
                form_grid("create_form").show(ui, |ui| {
                    self.compression.ui(
                        ui,
                        "Compression (new repository)",
                        "Stored in the repository; every machine that uses it shares it.",
                    );
                });
                let compression = self.compression.value().ok();
                let mut choice = None;
                theme::row_end(ui, |ui| {
                    if ui.button("Back").clicked() {
                        choice = Some(false);
                    }
                    if theme::primary(ui, compression.is_some(), None, "Create").clicked() {
                        choice = Some(true);
                    }
                });
                self.step = match (choice, compression) {
                    (Some(true), Some(compression)) => {
                        finishing(ui.ctx(), env, attempt, Some(compression))
                    }
                    (Some(false), _) => Step::Editing(None),
                    _ => Step::Confirm(attempt),
                };
                None
            }
            Step::Finishing(mut job, creating) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(if creating {
                        "Creating the repository"
                    } else {
                        "Opening the repository"
                    });
                });
                match job.take_result() {
                    None => self.step = Step::Finishing(job, creating),
                    Some(Err(err)) => self.step = Step::Editing(Some(failure_message(&err))),
                    Some(Ok(connected)) => return Some(connected),
                }
                None
            }
        }
    }

    /// Reads the form and starts probing its location.
    fn begin(&self, ctx: &egui::Context) -> Step {
        let password = match (self.password.is_empty(), self.remember) {
            (true, _) => PasswordMode::None,
            (false, true) => PasswordMode::Keyring,
            (false, false) => PasswordMode::Ask,
        };
        let mut config = AppConfig::new(self.form.repository(), password);
        config.repository_options = self.form.options();
        let options = self.form.secrets();
        let credentials = Credentials {
            password: self.password.clone(),
            options_remembered: self.remember && !options.is_empty(),
            options,
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
                location,
            },
        )
    }
}

/// Decides what a probe result means for the form (ADR-0009): open it, ask before creating, or
/// show why not.
fn next_after_probe(state: RepoState, attempt: Attempt, ctx: &egui::Context, env: &Env) -> Step {
    match state {
        RepoState::Ready => finishing(ctx, env, attempt, None),
        RepoState::Missing => Step::Confirm(attempt),
        RepoState::WrongPassword => Step::Editing(Some("Wrong encryption password.".to_string())),
        RepoState::LoginRefused => Step::Editing(Some(LOGIN_REFUSED.to_string())),
        RepoState::Error(message) => Step::Editing(Some(message)),
        _ => Step::Editing(Some("The repository cannot be opened.".to_string())),
    }
}

/// Starts opening the repository, or creating it with `create`'s compression when that is set.
fn finishing(
    ctx: &egui::Context,
    env: &Env,
    attempt: Attempt,
    create: Option<Compression>,
) -> Step {
    let env = env.clone();
    let creating = create.is_some();
    Step::Finishing(
        Job::spawn(ctx, move || finish(&env, attempt, create)),
        creating,
    )
}

/// Creates the repository when `create` holds its compression, otherwise opens it, then keeps
/// its secrets. Nothing is written to the config before the repository opened, so a failure
/// leaves nothing that points at it.
fn finish(env: &Env, attempt: Attempt, create: Option<Compression>) -> Result<Connected> {
    let Attempt {
        mut config,
        credentials,
        remember,
        location,
    } = attempt;
    let storage = Box::new(match create {
        Some(compression) => Storage::init(&location, &credentials.password, compression)?,
        None => Storage::open(&location, &credentials.password)?,
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

/// Gives the form a repository creation that runs until the returned sender is dropped.
#[cfg(test)]
impl Connect {
    pub(crate) fn hold_finishing(&mut self, ctx: &egui::Context) -> std::sync::mpsc::Sender<()> {
        let (release, job) = crate::job::held(ctx);
        self.step = Step::Finishing(job, true);
        release
    }
}

#[cfg(test)]
mod tests;
