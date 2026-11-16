//! The app config in the OS config dir (ADR-0009): repository, roots and custom games. The
//! password and secret option values are never part of it; they live in [`crate::secret`].

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::layout::Root;
use crate::manifest::CustomGame;
use crate::secret::SecretKey;
use crate::storage::RepoLocation;

/// The config file name inside the config directory.
const CONFIG_FILE: &str = "config.yaml";

/// Errors from reading, writing and resolving the config.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The config file or its directory could not be read or written.
    #[error("config file error: {0}")]
    Io(#[from] io::Error),
    /// The config file is not valid YAML for [`AppConfig`].
    #[error("config file is invalid: {0}")]
    Parse(#[from] serde_saphyr::Error),
    /// The config could not be turned into YAML.
    #[error("config cannot be serialized: {0}")]
    Serialize(#[from] serde_saphyr::SerializeError),
    /// An option has no value: a secret option absent from the secrets, or a plain one with none.
    #[error("no value for repository option `{0}`")]
    MissingValue(String),
}

/// One backend option. A secret option keeps only its key in the saved config; its value is in
/// the keyring and is never written by [`AppConfig::save`].
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoOption {
    /// The option name, such as an opendal service key.
    pub key: String,
    /// The value. For a secret option it may be held in memory but is never saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Whether the value is kept in the keyring instead of the config.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secret: bool,
}

/// Debug output hides the value of a secret option.
impl fmt::Debug for RepoOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match (&self.value, self.secret) {
            (Some(_), true) => Some("<redacted>"),
            (value, _) => value.as_deref(),
        };
        f.debug_struct("RepoOption")
            .field("key", &self.key)
            .field("value", &value)
            .field("secret", &self.secret)
            .finish()
    }
}

/// Where the repository password comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PasswordMode {
    /// The repository has no password.
    None,
    /// The password is stored in the keyring.
    Keyring,
    /// The password is asked once per session.
    Ask,
}

/// A kind of repository backend the picker offers (ADR-0009).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// A local folder.
    Local,
    /// A REST server.
    Rest,
    /// An rclone remote; needs the `rclone` binary.
    Rclone,
    /// An opendal service, by its name (`s3`, `webdav`, ...).
    OpenDal(&'static str),
}

/// The opendal services offered in the picker: those `rustic_backend` builds for this OS, minus
/// the ones ADR-0009 hides. `sftp` is Unix-only (ADR-0003).
const OPENDAL_SERVICES: &[&str] = &[
    "azblob",
    "azdls",
    "azfile",
    "b2",
    "cos",
    "dropbox",
    "ftp",
    "gcs",
    "gdrive",
    "ipmfs",
    "obs",
    "onedrive",
    "oss",
    "pcloud",
    "s3",
    #[cfg(not(windows))]
    "sftp",
    "swift",
    "webdav",
    "webhdfs",
    "yandex-disk",
];

/// The backend kinds available in this build: local, rest, rclone, then the opendal services.
pub fn backend_kinds() -> Vec<BackendKind> {
    [BackendKind::Local, BackendKind::Rest, BackendKind::Rclone]
        .into_iter()
        .chain(OPENDAL_SERVICES.iter().copied().map(BackendKind::OpenDal))
        .collect()
}

/// The persisted app configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    /// The rustic repository string: a folder, `rest:<url>`, `rclone:<remote>`, `opendal:<service>`.
    pub repository: String,
    /// Backend options for the repository.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repository_options: Vec<RepoOption>,
    /// Where the repository password comes from.
    pub password: PasswordMode,
    /// Configured game roots (ADR-0005).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<Root>,
    /// Games defined or adjusted by the user (ADR-0008).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_games: Vec<CustomGame>,
}

/// The salvajocs directory inside the OS config dir, or `None` when the OS has none.
pub fn default_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("salvajocs"))
}

impl AppConfig {
    /// Reads `dir/config.yaml`; `None` means it does not exist, so this is the first run.
    pub fn load(dir: &Path) -> Result<Option<Self>, ConfigError> {
        match fs::read_to_string(dir.join(CONFIG_FILE)) {
            Ok(yaml) => Ok(Some(serde_saphyr::from_str(&yaml)?)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Writes `dir/config.yaml` through a temp file and a rename, creating `dir`. Values of
    /// secret options are dropped.
    pub fn save(&self, dir: &Path) -> Result<(), ConfigError> {
        let mut stripped = self.clone();
        for option in stripped.repository_options.iter_mut().filter(|o| o.secret) {
            option.value = None;
        }
        let yaml = serde_saphyr::to_string(&stripped)?;
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{CONFIG_FILE}.tmp"));
        // Sync before the rename so a crash cannot leave an empty config.yaml.
        let mut file = fs::File::create(&tmp)?;
        file.write_all(yaml.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, dir.join(CONFIG_FILE))?;
        Ok(())
    }

    /// The keyring scope of this repository: JSON of the repository string and its sorted
    /// non-secret options, so two buckets on one service never share a password.
    pub fn secret_scope(&self) -> String {
        let options: BTreeMap<_, _> = self
            .repository_options
            .iter()
            .filter(|option| !option.secret)
            .filter_map(|option| Some((&option.key, option.value.as_ref()?)))
            .collect();
        serde_json::to_string(&(&self.repository, options))
            .expect("strings and string maps always serialize")
    }

    /// The repository location, with secret options taken from `secrets` (key to value).
    pub fn location(
        &self,
        secrets: &BTreeMap<String, String>,
    ) -> Result<RepoLocation, ConfigError> {
        let options = self
            .repository_options
            .iter()
            .map(|option| {
                let value = if option.secret {
                    secrets.get(&option.key)
                } else {
                    option.value.as_ref()
                };
                value
                    .map(|value| (option.key.clone(), value.clone()))
                    .ok_or_else(|| ConfigError::MissingValue(option.key.clone()))
            })
            .collect::<Result<_, _>>()?;
        Ok(RepoLocation {
            repository: self.repository.clone(),
            options,
        })
    }
}

/// The secrets the GUI must ask for before opening the repository: the password when its mode
/// needs one (`Ask` always, `Keyring` when `lookup` has none) and each secret option `lookup`
/// does not return.
pub fn missing_secrets(
    config: &AppConfig,
    lookup: impl Fn(&SecretKey) -> Option<String>,
) -> Vec<SecretKey> {
    let password = match config.password {
        PasswordMode::None => None,
        PasswordMode::Ask => Some(SecretKey::Password),
        PasswordMode::Keyring => Some(SecretKey::Password).filter(|key| lookup(key).is_none()),
    };
    let options = config
        .repository_options
        .iter()
        .filter(|option| option.secret)
        .map(|option| SecretKey::Option(option.key.clone()))
        .filter(|key| lookup(key).is_none());
    password.into_iter().chain(options).collect()
}

#[cfg(test)]
mod tests;
