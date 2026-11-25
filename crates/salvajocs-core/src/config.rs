//! The app config in the OS config dir (ADR-0009): repository, roots and custom games. The
//! password and secret option values are never part of it; they live in [`crate::secret`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde::{Deserialize, Serialize};

use crate::custom_games;
use crate::fsutil::write_atomic;
use crate::layout::Root;
use crate::manifest::CustomGame;
use crate::retention;
use crate::secret::SecretKey;
use crate::storage::{REST_PASSWORD_KEY, RepoLocation};

mod rest;
mod services;

pub use services::{BackendField, opendal_fields, opendal_hint, opendal_label, rest_fields};
use services::{OPENDAL_SERVICES, field_is_secret, opendal_reads_key};

/// The config file name inside the config directory.
const CONFIG_FILE: &str = "config.yaml";

/// Errors from reading, writing and resolving the config.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
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
#[non_exhaustive]
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

impl RepoOption {
    /// An option from its fields.
    pub fn new(key: String, value: Option<String>, secret: bool) -> Self {
        Self { key, value, secret }
    }
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
#[non_exhaustive]
pub enum PasswordMode {
    /// The repository has no password.
    None,
    /// The password is stored in the keyring.
    Keyring,
    /// The password is asked once per session.
    Ask,
}

/// Which color theme the app uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// Follow the system's light or dark preference.
    #[default]
    System,
    /// Always the light theme.
    Light,
    /// Always the dark theme.
    Dark,
}

impl ThemeChoice {
    /// Whether this is the default, which the config file leaves out.
    fn is_system(&self) -> bool {
        *self == Self::System
    }
}

/// What a list of games is sorted by (ADR-0027).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SortKey {
    /// The game's title, in byte order.
    #[default]
    Name,
    /// The time of the game's last backup.
    LastBackup,
    /// The size of the files in the game's last backup.
    LastBackupSize,
    /// The space all of the game's backups use in the repository.
    SpaceUsed,
    /// How many files the next backup would add, change or remove (ADR-0032). The Backup tab
    /// only.
    Changes,
}

impl SortKey {
    /// Every key, in the order a sort control lists them.
    pub const ALL: &[SortKey] = &[
        SortKey::Name,
        SortKey::LastBackup,
        SortKey::LastBackupSize,
        SortKey::SpaceUsed,
        SortKey::Changes,
    ];

    /// The key's natural direction: names A to Z, sizes largest first, times newest first,
    /// changes most first.
    pub fn descending_by_default(self) -> bool {
        self != SortKey::Name
    }
}

/// How a list of games is sorted. A value this version cannot read loads as the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ListSort {
    /// The key the list is sorted by.
    #[serde(default)]
    pub by: SortKey,
    /// Whether the order is the reverse of ascending.
    #[serde(default)]
    pub descending: bool,
}

impl ListSort {
    /// The sort by `by`, descending when `descending`.
    pub fn new(by: SortKey, descending: bool) -> Self {
        Self { by, descending }
    }

    /// Whether this is the default, name A to Z, which the config file leaves out.
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A sort that does not parse, such as a key a newer version added, reads as the default, so
/// the rest of the config still loads.
fn lenient_sort<'de, D>(deserializer: D) -> Result<ListSort, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// Titles that do not parse read as none, so the rest of the config still loads.
fn lenient_titles<'de, D>(deserializer: D) -> Result<BTreeSet<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// Unsent rules that do not parse, such as a rule a newer version added, read as none, so the
/// rest of the config still loads.
fn lenient_retention<'de, D>(deserializer: D) -> Result<Option<retention::Unsent>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
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

/// The backend kinds available in this build: local, rest, rclone, then the opendal services.
pub fn backend_kinds() -> Vec<BackendKind> {
    [BackendKind::Local, BackendKind::Rest, BackendKind::Rclone]
        .into_iter()
        .chain(
            OPENDAL_SERVICES
                .iter()
                .map(|service| BackendKind::OpenDal(service.name)),
        )
        .collect()
}

/// REST and rclone keys that rustic_backend 0.8.0 renamed, as (old, new). salvajocs 0.9.1 and
/// earlier saved the old names.
const RENAMED_HTTP_KEYS: [(&str, &str); 2] = [
    ("cacert", "http-cacert"),
    ("tls-client-cert", "http-tls-client-cert"),
];

/// The keys rustic itself reads for a backend, matched exactly: rustic does not lowercase them.
fn rustic_keys(kind: BackendKind) -> &'static [&'static str] {
    match kind {
        BackendKind::Local => &["post-create-command", "post-delete-command"],
        BackendKind::Rest => &[
            "retry",
            "timeout",
            "http-cacert",
            "http-tls-client-cert",
            "http-insecure-tls",
            "http-referer",
            "http-read-timeout",
            "http-connect-timeout",
        ],
        BackendKind::Rclone => &[
            "retry",
            "timeout",
            "http-cacert",
            "http-tls-client-cert",
            "http-insecure-tls",
            "http-referer",
            "http-read-timeout",
            "http-connect-timeout",
            "rclone-command",
            "use-password",
            "rest-url",
        ],
        BackendKind::OpenDal(_) => &[
            "retry",
            "connections",
            "throttle",
            "timeout",
            "http-cacert",
            "http-tls-client-cert",
            "http-insecure-tls",
            "http-referer",
            "http-read-timeout",
            "http-connect-timeout",
        ],
    }
}

/// Whether `key` is one of the keys rustic itself reads for the backend. They are exact, even
/// next to an opendal service, whose own keys are read in any letter case.
pub fn is_rustic_key(kind: BackendKind, key: &str) -> bool {
    rustic_keys(kind).contains(&key)
}

/// Whether an Advanced option named `key` should start out secret. A key of rustic's own or of a
/// non-secret field of a service, such as `use-password` or `access_key_id`, is not; a secret
/// field's key or alias is; any other name is when it has `secret`, `password`, `token` or `key`
/// in it. Letter case does not matter.
pub fn looks_secret(key: &str) -> bool {
    let key = key.trim().to_ascii_lowercase();
    let rustic = [
        BackendKind::Local,
        BackendKind::Rest,
        BackendKind::Rclone,
        BackendKind::OpenDal(""),
    ]
    .into_iter()
    .any(|kind| is_rustic_key(kind, &key));
    if rustic {
        return false;
    }
    field_is_secret(&key).unwrap_or_else(|| {
        ["secret", "password", "token", "key"]
            .iter()
            .any(|word| key.contains(word))
    })
}

/// Whether the backend reads the option `key`: one of rustic's own keys, the password of a REST
/// server, or for an opendal service a key of its config or an alias of one. An unknown key is
/// allowed but does nothing.
pub fn option_key_known(kind: BackendKind, key: &str) -> bool {
    is_rustic_key(kind, key)
        || (kind == BackendKind::Rest && key == REST_PASSWORD_KEY)
        || matches!(kind, BackendKind::OpenDal(service) if opendal_reads_key(service, key))
}

/// The remotes of the user's rclone config, from `rclone listremotes`. A missing `rclone` binary
/// is an [`io::ErrorKind::NotFound`] error; a failing one carries its stderr.
pub fn rclone_remotes() -> io::Result<Vec<String>> {
    // No password prompt and no stdin: an encrypted rclone config would otherwise wait forever.
    let mut command = Command::new("rclone");
    command
        .args(["listremotes", "--ask-password=false"])
        .stdin(Stdio::null());
    // The app has no console on Windows, so a console program would open a window of its own.
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(
        &mut command,
        crate::storage::CREATE_NO_WINDOW,
    );
    let output = command.output()?;
    if !output.status.success() {
        return Err(failure(&output));
    }
    Ok(parse_remotes(&String::from_utf8_lossy(&output.stdout)))
}

/// The error for a failed `rclone`: its stderr, or its exit status when it said nothing.
fn failure(output: &Output) -> io::Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    match stderr.trim() {
        "" => io::Error::other(format!("rclone {}", output.status)),
        message => io::Error::other(message),
    }
}

/// The remote names of `rclone listremotes` output: one `name:` per line.
fn parse_remotes(output: &str) -> Vec<String> {
    output
        .lines()
        .map(|line| line.trim().trim_end_matches(':'))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// The persisted app configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
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
    /// Games defined or adjusted by the user (ADR-0008): this PC's copy of the repository's,
    /// with each game's Wine prefix (ADR-0028).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_games: Vec<CustomGame>,
    /// Whether the app asks GitHub for a newer release at start (ADR-0013).
    #[serde(default = "default_true")]
    pub check_updates: bool,
    /// Launcher prefixes that scans and restores leave out (ADR-0017).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored_prefixes: Vec<PathBuf>,
    /// The color theme: the system's, or always light or dark.
    #[serde(default, skip_serializing_if = "ThemeChoice::is_system")]
    pub theme: ThemeChoice,
    /// How the Backup tab sorts its games (ADR-0027).
    #[serde(
        default,
        deserialize_with = "lenient_sort",
        skip_serializing_if = "ListSort::is_default"
    )]
    pub backup_sort: ListSort,
    /// How the Restore tab sorts its games (ADR-0027).
    #[serde(
        default,
        deserialize_with = "lenient_sort",
        skip_serializing_if = "ListSort::is_default"
    )]
    pub restore_sort: ListSort,
    /// The key ([`AppConfig::secret_scope`]) of the repository whose custom games
    /// `custom_games` mirrors; `None` until one was read (ADR-0028). Written under a new name,
    /// so salvajocs 0.8.0 never mirrors over the unsent games (ADR-0029).
    #[serde(
        rename = "custom_games_read_from",
        alias = "custom_games_repository",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub custom_games_repository: Option<String>,
    /// The custom games changed here since the last Back up, in the order changed (ADR-0029).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_games_unsent: Vec<custom_games::Unsent>,
    /// The repository's custom game copies set aside here with **Not now** (ADR-0030).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_games_not_now: Option<custom_games::NotNow>,
    /// The Old backups rules changed here since the last Back up (ADR-0029).
    #[serde(
        default,
        deserialize_with = "lenient_retention",
        skip_serializing_if = "Option::is_none"
    )]
    pub retention_unsent: Option<retention::Unsent>,
    /// The titles of the games whose "Delete files the backup does not have" box was last left
    /// on; off for every other game (ADR-0036).
    #[serde(
        default,
        deserialize_with = "lenient_titles",
        skip_serializing_if = "BTreeSet::is_empty"
    )]
    pub restore_deletes: BTreeSet<String>,
}

fn default_true() -> bool {
    true
}

/// The salvajocs directory inside the OS config dir, or `None` when the OS has none.
pub fn default_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("salvajocs"))
}

impl AppConfig {
    /// A config for `repository` and `password` with every other field as an omitted field
    /// reads from the file: no options, roots or custom games, update checks on, the system
    /// theme, both lists sorted by name, no repository's custom games mirrored, nothing
    /// unsent or set aside and no restore that deletes.
    pub fn new(repository: String, password: PasswordMode) -> Self {
        Self {
            repository,
            repository_options: Vec::new(),
            password,
            roots: Vec::new(),
            custom_games: Vec::new(),
            check_updates: default_true(),
            ignored_prefixes: Vec::new(),
            theme: ThemeChoice::System,
            backup_sort: ListSort::default(),
            restore_sort: ListSort::default(),
            custom_games_repository: None,
            custom_games_unsent: Vec::new(),
            custom_games_not_now: None,
            retention_unsent: None,
            restore_deletes: BTreeSet::new(),
        }
    }

    /// Reads `dir/config.yaml`; `None` means it does not exist, so this is the first run.
    pub fn load(dir: &Path) -> Result<Option<Self>, ConfigError> {
        match fs::read_to_string(dir.join(CONFIG_FILE)) {
            Ok(yaml) => {
                let mut config: Self = serde_saphyr::from_str(&yaml)?;
                config.rename_legacy_http_keys();
                Ok(Some(config))
            }
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
        write_atomic(&dir.join(CONFIG_FILE), yaml.as_bytes())?;
        Ok(())
    }

    /// Whether the repository is a REST server or an rclone remote.
    fn is_rest_or_rclone(&self) -> bool {
        self.repository.starts_with("rest:") || self.repository.starts_with("rclone:")
    }

    /// Renames the REST and rclone keys of salvajocs 0.9.1 and earlier to those of
    /// rustic_backend 0.8.0, so the next save writes the new names. An old key is left alone when
    /// the new one is also set.
    fn rename_legacy_http_keys(&mut self) {
        if !self.is_rest_or_rclone() {
            return;
        }
        for (old, new) in RENAMED_HTTP_KEYS {
            if self
                .repository_options
                .iter()
                .any(|option| option.key == new)
            {
                continue;
            }
            for option in self.repository_options.iter_mut().filter(|o| o.key == old) {
                option.key = new.to_string();
            }
        }
    }

    /// The keyring scope of this repository: JSON of the repository string and its sorted
    /// non-secret options, so two buckets on one service never share a password. A renamed REST
    /// or rclone key counts under its old name, unless the old key is also set, so a rename keeps
    /// the scope and the secrets stored under it.
    pub fn secret_scope(&self) -> String {
        let rest_or_rclone = self.is_rest_or_rclone();
        let has_key = |key: &str| self.repository_options.iter().any(|o| o.key == key);
        let options: BTreeMap<_, _> = self
            .repository_options
            .iter()
            .filter(|option| !option.secret)
            .filter_map(|option| {
                let key = RENAMED_HTTP_KEYS
                    .iter()
                    .find(|(old, new)| rest_or_rclone && option.key == *new && !has_key(old))
                    .map_or(&option.key[..], |(old, _)| old);
                Some((key, option.value.as_ref()?))
            })
            .collect();
        serde_json::to_string(&(&self.repository, options))
            .expect("strings and string maps always serialize")
    }

    /// Whether the offered custom game `offered`, from this config's repository, is set aside
    /// with **Not now**: [`AppConfig::custom_games_not_now`] holds exactly that copy for this
    /// repository (ADR-0030).
    pub fn set_aside(&self, offered: &CustomGame) -> bool {
        (self.custom_games_not_now.as_ref()).is_some_and(|not_now| {
            not_now.repository == self.secret_scope() && not_now.games.contains(offered)
        })
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
