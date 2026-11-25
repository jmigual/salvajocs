//! What every screen shares: the config, the open repository and the notices to show.

use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex, MutexGuard};

use eframe::egui;
use salvajocs_core::config::{AppConfig, PasswordMode};
use salvajocs_core::manifest::Manifest;
use salvajocs_core::secret::SecretKey;
use salvajocs_core::storage::{Compression, Storage};

use crate::env::Env;
use crate::first_run::Connected;
use crate::job::Job;
use crate::startup::Credentials;

mod custom_games;

pub use custom_games::{CustomGameJobs, Edit};

/// The open repository, shared with the jobs that work on it.
pub type SharedStorage = Arc<Mutex<Storage>>;

/// Locks the repository for a job; a lock poisoned by a panicked job is reported, not unwrapped.
pub fn lock_storage(storage: &SharedStorage) -> anyhow::Result<MutexGuard<'_, Storage>> {
    storage
        .lock()
        .map_err(|_| anyhow::anyhow!("the repository lock is poisoned; restart the app"))
}

/// `error` and its sources, joined.
pub(crate) fn chain(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(next) = source {
        text = format!("{text}: {next}");
        source = next.source();
    }
    text
}

/// The space each game's backups use, by the game's sorted snapshot ids: a value stays right as
/// long as that exact set of snapshots does (ADR-0027).
pub type SpaceCache = Arc<Mutex<HashMap<Vec<String>, u64>>>;

/// An open repository and what the screens show about it without locking it.
pub struct Session {
    /// The repository itself.
    pub storage: SharedStorage,
    /// Its compression as last read.
    pub compression: Compression,
    /// The space used of the games computed so far in this repository.
    pub space: SpaceCache,
}

/// Keyring entries of a repository location the app no longer uses (ADR-0009).
#[derive(Debug, PartialEq, Eq)]
pub struct StaleCredentials {
    /// The scope of the old location (`AppConfig::secret_scope`).
    pub scope: String,
    /// The secrets that may be stored under it.
    pub keys: Vec<SecretKey>,
}

/// The keyring entries `old` may have left behind when the config becomes `new`; `None` when the
/// location is unchanged or nothing could have been stored.
pub fn stale_credentials(old: &AppConfig, new: &AppConfig) -> Option<StaleCredentials> {
    let scope = old.secret_scope();
    if scope == new.secret_scope() {
        return None;
    }
    let keys: Vec<_> = (old.password == PasswordMode::Keyring)
        .then_some(SecretKey::Password)
        .into_iter()
        .chain(
            old.repository_options
                .iter()
                .filter(|option| option.secret)
                .map(|option| SecretKey::Option(option.key.clone())),
        )
        .collect();
    (!keys.is_empty()).then_some(StaleCredentials { scope, keys })
}

/// The data the tabs read and change.
pub struct State {
    /// Where the app finds its directories, keyring and manifest.
    pub env: Env,
    /// The saved config; `None` until the first run completed.
    pub config: Option<AppConfig>,
    /// The secrets of the open repository.
    pub credentials: Credentials,
    /// The open repository, once startup got that far.
    pub session: Option<Session>,
    /// The loaded manifest.
    pub manifest: Option<Arc<Manifest>>,
    /// Keyring entries of the previous repository, waiting for the user's decision.
    pub stale: Option<StaleCredentials>,
    /// Messages for the banner, until dismissed.
    pub notices: Vec<String>,
    /// The reads of the custom games in the repository (ADR-0028, ADR-0029).
    pub custom_games: CustomGameJobs,
    /// The config's REST password came out of its URL at load and is not yet in the keyring, so
    /// a save writes it back into the URL rather than lose it.
    pub keep_legacy_rest_url: bool,
    saving: Option<Job<()>>,
    save_again: bool,
    remembering: Option<Job<()>>,
}

impl State {
    /// State with nothing loaded yet.
    pub fn new(env: Env) -> Self {
        Self {
            env,
            config: None,
            credentials: Credentials::default(),
            session: None,
            manifest: None,
            stale: None,
            notices: Vec::new(),
            custom_games: CustomGameJobs::default(),
            keep_legacy_rest_url: false,
            saving: None,
            save_again: false,
            remembering: None,
        }
    }

    /// The manifest to scan and restore with: the loaded one, or an empty one so that custom games
    /// still work.
    pub fn manifest_or_empty(&self) -> Arc<Manifest> {
        self.manifest.clone().unwrap_or_default()
    }

    /// Makes a repository the user just connected the current one and saves the config. Roots,
    /// custom games, their unsent changes and the set-aside ones (which apply only to the
    /// repository they came from), the unsent Old backups rules, the update setting,
    /// the theme, the sorts, the games whose restore deletes and the ignored prefixes come from
    /// the current config, which may have changed since the form was built. Then reads the
    /// custom games of the new repository, which writes nothing to it (ADR-0029).
    pub fn adopt(&mut self, connected: Connected, ctx: &egui::Context) {
        let Connected {
            mut config,
            credentials,
            storage,
            warnings,
        } = connected;
        if let Some(old) = &self.config {
            config.roots = old.roots.clone();
            config.custom_games = old.custom_games.clone();
            config.custom_games_repository = old.custom_games_repository.clone();
            config.custom_games_unsent = old.custom_games_unsent.clone();
            config.custom_games_not_now = old.custom_games_not_now.clone();
            config.retention_unsent = old.retention_unsent;
            config.check_updates = old.check_updates;
            config.theme = old.theme;
            config.backup_sort = old.backup_sort;
            config.restore_sort = old.restore_sort;
            config.restore_deletes = old.restore_deletes.clone();
            config.ignored_prefixes = old.ignored_prefixes.clone();
            self.stale = stale_credentials(old, &config).or(self.stale.take());
        }
        // The last read's offers are the old repository's: accepted here, they would reach the
        // new one.
        self.custom_games.forget_read();
        self.session = Some(Session {
            compression: storage.compression(),
            storage: Arc::new(Mutex::new(*storage)),
            space: SpaceCache::default(),
        });
        self.config = Some(config);
        self.credentials = credentials;
        self.notices.extend(warnings);
        self.save_config(ctx);
        self.read_custom_games(ctx);
    }

    /// Writes the config in the background. A save already running is followed by another, so
    /// the file ends up with the latest config.
    pub fn save_config(&mut self, ctx: &egui::Context) {
        if self.saving.is_some() {
            self.save_again = true;
            return;
        }
        let Some(mut config) = self.config.clone() else {
            return;
        };
        if self.keep_legacy_rest_url {
            config.put_back_rest_password();
        }
        let dir = self.env.config_dir.clone();
        self.saving = Some(Job::spawn(ctx, move || Ok(config.save(&dir)?)));
    }

    /// Stores the session's password in the keyring in the background. When the store refuses
    /// it, the config falls back to asking each session.
    pub fn remember_password(&mut self, ctx: &egui::Context) {
        let Some(config) = &self.config else {
            return;
        };
        let (store, scope) = (self.env.secrets.clone(), config.secret_scope());
        let password = self.credentials.password.clone();
        self.remembering = Some(Job::spawn(ctx, move || {
            Ok(store.set(&scope, &SecretKey::Password, &password)?)
        }));
    }

    /// Whether a config save, a keyring write or a custom games read is running.
    pub(crate) fn busy(&self) -> bool {
        self.saving.is_some() || self.remembering.is_some() || self.custom_games.busy()
    }

    /// Whether a config save or a keyring write is running.
    pub(crate) fn writing(&self) -> bool {
        self.saving.is_some() || self.remembering.is_some()
    }

    /// Collects finished background work, reporting failures.
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.poll_custom_games(ctx);
        if let Some(job) = &mut self.remembering
            && let Some(result) = job.take_result()
        {
            self.remembering = None;
            if let (Err(err), Some(config)) = (result, &mut self.config) {
                config.password = PasswordMode::Ask;
                self.notices.push(format!(
                    "Could not store the encryption password in the keyring ({err:#}); it will be asked each session."
                ));
                self.save_config(ctx);
            }
        }
        let Some(job) = &mut self.saving else {
            return;
        };
        let Some(result) = job.take_result() else {
            return;
        };
        self.saving = None;
        if let Err(err) = result {
            self.notices
                .push(format!("Could not save the config: {err:#}"));
        }
        if std::mem::take(&mut self.save_again) {
            self.save_config(ctx);
        }
    }
}

#[cfg(test)]
mod tests;
