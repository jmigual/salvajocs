//! Selection changes not yet saved to the repository, kept in `pending-selection.json` in the
//! config dir until the next backup saves them (ADR-0014).

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Change, SelectionError, check_format};
use crate::config::AppConfig;

/// The pending file name inside the config directory.
const PENDING_FILE: &str = "pending-selection.json";
/// The [`Pending`] format this version reads and writes.
const PENDING_FORMAT: u32 = 1;

/// This machine's unsaved selection changes, kept apart per repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    format: u32,
    /// Repository key ([`AppConfig::secret_scope`]: the repository string and its non-secret
    /// options) to exact game title to its changes, oldest first.
    #[serde(default)]
    pub repositories: BTreeMap<String, BTreeMap<String, Vec<Change>>>,
}

impl Default for Pending {
    fn default() -> Self {
        Self {
            format: PENDING_FORMAT,
            repositories: BTreeMap::new(),
        }
    }
}

impl Pending {
    /// Reads `dir/pending-selection.json`; a missing file has no changes.
    ///
    /// # Errors
    /// [`SelectionError::NewerFormat`] for a newer version's file, the parse or I/O error.
    pub fn load(dir: &Path) -> Result<Self, SelectionError> {
        let path = dir.join(PENDING_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(SelectionError::Io { path, source }),
        };
        check_format(&bytes, PENDING_FORMAT)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Writes `dir/pending-selection.json`, every repository's changes, through a temp file and
    /// a rename, creating `dir`.
    pub fn save(&self, dir: &Path) -> Result<(), SelectionError> {
        let json = serde_json::to_vec_pretty(self).expect("string-keyed maps always serialize");
        let tmp = dir.join(format!("{PENDING_FILE}.tmp"));
        let written = fs::create_dir_all(dir).and_then(|()| {
            // Sync before the rename so a crash cannot leave an empty file.
            let mut file = fs::File::create(&tmp)?;
            file.write_all(&json)?;
            file.sync_all()?;
            fs::rename(&tmp, dir.join(PENDING_FILE))
        });
        written.map_err(|source| SelectionError::Io {
            path: dir.join(PENDING_FILE),
            source,
        })
    }

    /// The changes, by game title, for the repository of `config`.
    pub fn games_mut(&mut self, config: &AppConfig) -> &mut BTreeMap<String, Vec<Change>> {
        self.repositories.entry(config.secret_scope()).or_default()
    }

    /// Moves the changes kept under the repository key `from` to the key `to`, for a repository
    /// whose key changed; returns whether there were any. Where both have changes for a game,
    /// those under `to` stay.
    pub fn rescope(&mut self, from: &str, to: &str) -> bool {
        if from == to {
            return false;
        }
        let Some(games) = self.repositories.remove(from) else {
            return false;
        };
        let target = self.repositories.entry(to.to_string()).or_default();
        for (title, changes) in games {
            target.entry(title).or_insert(changes);
        }
        true
    }

    /// Drops the changes of `title` for the repository of `config`, once they are saved to it.
    pub fn remove(&mut self, config: &AppConfig, title: &str) {
        let key = config.secret_scope();
        if let Some(games) = self.repositories.get_mut(&key) {
            games.remove(title);
            if games.is_empty() {
                self.repositories.remove(&key);
            }
        }
    }
}

#[cfg(test)]
mod tests;
