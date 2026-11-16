//! What the app reads from its surroundings: directories, the credential store and the manifest
//! download. Tests substitute their own so nothing touches the real keyring or the network.

use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

use salvajocs_core::config;
use salvajocs_core::manifest::fetch::{self, FetchError, Freshness};
use salvajocs_core::secret::{self, SecretError, SecretKey};

/// The credential store of one repository scope (`AppConfig::secret_scope`).
pub trait SecretStore: Send + Sync {
    /// Reads a secret; `None` when there is none.
    fn get(&self, scope: &str, key: &SecretKey) -> Result<Option<String>, SecretError>;
    /// Stores a secret, replacing any previous value.
    fn set(&self, scope: &str, key: &SecretKey, value: &str) -> Result<(), SecretError>;
    /// Removes a secret; removing a missing one succeeds.
    fn delete(&self, scope: &str, key: &SecretKey) -> Result<(), SecretError>;
}

/// The OS credential store, through `salvajocs_core::secret`.
pub struct SystemSecrets;

impl SecretStore for SystemSecrets {
    fn get(&self, scope: &str, key: &SecretKey) -> Result<Option<String>, SecretError> {
        secret::get(scope, key)
    }

    fn set(&self, scope: &str, key: &SecretKey, value: &str) -> Result<(), SecretError> {
        secret::set(scope, key, value)
    }

    fn delete(&self, scope: &str, key: &SecretKey) -> Result<(), SecretError> {
        secret::delete(scope, key)
    }
}

/// A store that keeps secrets in memory. [`MemorySecrets::full`] refuses every write the way the
/// Windows store refuses an over-long value.
#[cfg(test)]
#[derive(Default)]
pub struct MemorySecrets {
    entries: Mutex<Vec<(String, SecretKey, String)>>,
    full: bool,
}

#[cfg(test)]
impl MemorySecrets {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// A store whose `set` always fails with [`SecretError::TooLong`].
    pub fn full() -> Self {
        Self {
            full: true,
            ..Self::default()
        }
    }
}

#[cfg(test)]
impl SecretStore for MemorySecrets {
    fn get(&self, scope: &str, key: &SecretKey) -> Result<Option<String>, SecretError> {
        let entries = self
            .entries
            .lock()
            .expect("no panic while the lock is held");
        Ok(entries
            .iter()
            .find(|(s, k, _)| s == scope && k == key)
            .map(|(_, _, value)| value.clone()))
    }

    fn set(&self, scope: &str, key: &SecretKey, value: &str) -> Result<(), SecretError> {
        if self.full {
            return Err(SecretError::TooLong);
        }
        self.delete(scope, key)?;
        let mut entries = self
            .entries
            .lock()
            .expect("no panic while the lock is held");
        entries.push((scope.to_string(), key.clone(), value.to_string()));
        Ok(())
    }

    fn delete(&self, scope: &str, key: &SecretKey) -> Result<(), SecretError> {
        let mut entries = self
            .entries
            .lock()
            .expect("no panic while the lock is held");
        entries.retain(|(s, k, _)| !(s == scope && k == key));
        Ok(())
    }
}

/// Downloads the manifest into a cache directory.
pub type RefreshManifest = fn(&Path) -> Result<Freshness, FetchError>;

/// Everything a worker thread needs from the environment; cheap to clone.
#[derive(Clone)]
pub struct Env {
    /// Holds `config.yaml`.
    pub config_dir: PathBuf,
    /// Holds the manifest cache.
    pub cache_dir: PathBuf,
    /// Where passwords and secret options are kept.
    pub secrets: Arc<dyn SecretStore>,
    /// Revalidates the manifest cache.
    pub refresh_manifest: RefreshManifest,
}

impl Env {
    /// The real environment, or `None` when the OS has no config or cache directory.
    pub fn system() -> Option<Self> {
        Some(Self {
            config_dir: config::default_dir()?,
            cache_dir: dirs::cache_dir()?.join("salvajocs"),
            secrets: Arc::new(SystemSecrets),
            refresh_manifest: fetch::refresh,
        })
    }
}
