//! The blocking work of starting up, run in jobs: finding the secrets, opening the repository
//! and loading the manifest.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use anyhow::Result;
use salvajocs_core::config::{self, AppConfig, PasswordMode};
use salvajocs_core::manifest::Manifest;
use salvajocs_core::manifest::fetch;
use salvajocs_core::secret::SecretKey;
use salvajocs_core::selection::Pending;
use salvajocs_core::storage::{REST_PASSWORD_KEY, Storage};

use crate::env::{Env, RefreshManifest};

/// The secrets of the open repository, held for this session only.
#[derive(Default, Clone, PartialEq, Eq)]
pub struct Credentials {
    /// The repository password; empty means none.
    pub password: String,
    /// The values of the secret options, by key.
    pub options: BTreeMap<String, String>,
    /// Whether the secret options are kept in the keyring: one was found there at startup, or
    /// the user chose to remember them. The repository form starts ticked from it.
    pub options_remembered: bool,
}

/// Debug output hides the values: they are secrets.
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("has_password", &!self.password.is_empty())
            .field("option_keys", &self.options.keys().collect::<Vec<_>>())
            .field("options_remembered", &self.options_remembered)
            .finish()
    }
}

impl Credentials {
    /// Adds the values the user typed for the secrets that were missing.
    pub fn answer(&mut self, answers: Vec<(SecretKey, String)>) {
        for (key, value) in answers {
            match key {
                SecretKey::Password => self.password = value,
                SecretKey::Option(key) => {
                    self.options.insert(key, value);
                }
            }
        }
    }
}

/// What the keyring held for a config, and what the user still has to type.
#[derive(Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The secrets found in the keyring.
    pub found: Credentials,
    /// The secrets to ask for, as `config::missing_secrets` lists them.
    pub missing: Vec<SecretKey>,
    /// Whether the REST password moved out of the config's URL is not in the keyring, so the
    /// config file must keep it. Set by the caller that moved it, never by [`resolve`].
    pub rest_password_unstored: bool,
}

/// Looks up the keyring secrets of `config`. A keyring that fails reads as "not stored", so the
/// secret is asked for instead (ADR-0009).
pub fn resolve(env: &Env, config: &AppConfig) -> Resolved {
    let scope = config.secret_scope();
    let candidates = config
        .repository_options
        .iter()
        .filter(|option| option.secret)
        .map(|option| SecretKey::Option(option.key.clone()))
        .chain((config.password == PasswordMode::Keyring).then_some(SecretKey::Password));
    // An option the keyring could not read may still be stored there; counting it as remembered
    // keeps an Apply in Settings from deleting it.
    let mut unreadable_option = false;
    let looked_up: Vec<(SecretKey, String)> = candidates
        .filter_map(|key| match env.secrets.get(&scope, &key) {
            Ok(value) => Some((key, value?)),
            Err(_) => {
                unreadable_option |= matches!(key, SecretKey::Option(_));
                None
            }
        })
        .collect();
    // A secret option holding a value was just moved out of the config file (see
    // `move_legacy_rest_password`); the keyring may not have it.
    let held: Vec<_> = config
        .repository_options
        .iter()
        .filter(|option| option.secret)
        .filter_map(|option| Some((SecretKey::Option(option.key.clone()), option.value.clone()?)))
        .filter(|(key, _)| !looked_up.iter().any(|(found, _)| found == key))
        .collect();
    let looked_up: Vec<_> = looked_up.into_iter().chain(held).collect();
    let missing = config::missing_secrets(config, |key| {
        looked_up
            .iter()
            .find(|(found, _)| found == key)
            .map(|(_, value)| value.clone())
    });
    let mut found = Credentials {
        options_remembered: unreadable_option
            || looked_up
                .iter()
                .any(|(key, _)| matches!(key, SecretKey::Option(_))),
        ..Credentials::default()
    };
    found.answer(looked_up);
    Resolved {
        found,
        missing,
        rest_password_unstored: false,
    }
}

/// Carries over what a config that had its password in a `rest:` URL left under `old_scope`, the
/// keyring scope that held the password (`AppConfig::take_rest_password`): the secrets move to the
/// scope of `config`, each old entry is deleted once its copy is stored (the account names hold
/// the password, but a failed copy must not lose the secret), the REST password itself is stored,
/// and the pending selection changes follow the new scope. Each step is best effort: what fails
/// is asked for or re-read later. Returns whether every secret option holding a value, the REST
/// password, is in the keyring now; when not, the config file must keep it.
pub fn move_legacy_rest_password(env: &Env, config: &AppConfig, old_scope: &str) -> bool {
    let scope = config.secret_scope();
    let keys = config
        .repository_options
        .iter()
        .filter(|option| option.secret && option.key != REST_PASSWORD_KEY)
        .map(|option| SecretKey::Option(option.key.clone()))
        .chain([SecretKey::Password]);
    for key in keys {
        if let Ok(Some(value)) = env.secrets.get(old_scope, &key)
            && env.secrets.set(&scope, &key, &value).is_ok()
        {
            let _ = env.secrets.delete(old_scope, &key);
        }
    }
    let mut stored = true;
    for option in config.repository_options.iter().filter(|o| o.secret) {
        if let Some(value) = &option.value {
            let key = SecretKey::Option(option.key.clone());
            stored &= env.secrets.set(&scope, &key, value).is_ok();
        }
    }
    if let Ok(mut pending) = Pending::load(&env.config_dir)
        && pending.rescope(old_scope, &scope)
    {
        let _ = pending.save(&env.config_dir);
    }
    stored
}

/// Opens the repository of `config`; this runs scrypt, so it belongs in a job. Boxed because a
/// [`Storage`] is large and travels through a job.
pub fn open(config: &AppConfig, credentials: &Credentials) -> Result<Box<Storage>> {
    let location = config.location(&credentials.options)?;
    Ok(Box::new(Storage::open(&location, &credentials.password)?))
}

/// The manifest as far as it could be loaded, and why not further.
#[derive(Debug, Default)]
pub struct ManifestLoad {
    /// The cached manifest, refreshed when the download worked.
    pub manifest: Option<Manifest>,
    /// The download or cache failure to show in a banner.
    pub error: Option<String>,
}

fn describe(err: impl Into<anyhow::Error>) -> String {
    format!("{:#}", err.into())
}

/// Revalidates the manifest cache with `refresh`, then reads it. A failed download still leaves
/// the cached copy usable.
pub fn load_manifest(cache_dir: &Path, refresh: RefreshManifest) -> ManifestLoad {
    let mut error = refresh(cache_dir).err().map(describe);
    let manifest = match fetch::load_cached(cache_dir) {
        Ok(manifest) => manifest,
        Err(err) => {
            error.get_or_insert_with(|| describe(err));
            None
        }
    };
    ManifestLoad { manifest, error }
}

#[cfg(test)]
mod tests;
