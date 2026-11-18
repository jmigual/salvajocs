//! Which files and registry keys of a game are backed up (ADR-0014). A game's
//! [`GameSettings`] say what is excluded; [`apply`] filters a scan with them before backup.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::crossbuild::{Family, base_family};
use crate::layout::Base;
use crate::scan::{FoundFile, GameScan};

/// The [`GameSettings`] format this version reads and writes.
pub const FORMAT: u32 = 1;

/// Errors from reading or saving backup selections.
#[derive(Debug, thiserror::Error)]
pub enum SelectionError {
    /// The settings were written by a newer version: they are neither applied nor rewritten.
    #[error("settings format {0} is newer than this version understands")]
    NewerFormat(u32),
    /// The settings are not valid JSON for their format.
    #[error("cannot parse the settings")]
    Parse(#[from] serde_json::Error),
}

/// One game's backup selection. Paths and registry keys are included unless saved otherwise.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameSettings {
    /// Format version, [`FORMAT`].
    pub format: u32,
    /// The game is not backed up at all.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip: bool,
    /// Path key ([`path_key`]) of a file or folder to whether it is included; a path takes the
    /// value of its deepest saved ancestor-or-self.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub paths: BTreeMap<String, bool>,
    /// Manifest registry key to whether it is included, matched ignoring ASCII case.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub registry: BTreeMap<String, bool>,
    /// Fields of a newer version, kept so rewriting the settings never drops them.
    #[serde(flatten)]
    pub other: BTreeMap<String, Value>,
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            format: FORMAT,
            skip: false,
            paths: BTreeMap::new(),
            registry: BTreeMap::new(),
            other: BTreeMap::new(),
        }
    }
}

/// One unsaved edit of a game's selection, replayed onto the freshest saved copy on save.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// Sets [`GameSettings::skip`].
    Skip(bool),
    /// [`GameSettings::set_path`].
    Path {
        /// The path key of the file or folder.
        node: String,
        /// Whether it and everything below it is included.
        included: bool,
    },
    /// [`GameSettings::set_registry`].
    Registry {
        /// The manifest registry key.
        key: String,
        /// Whether it is included.
        included: bool,
    },
}

impl GameSettings {
    /// Parses settings JSON. A `format` above [`FORMAT`] is [`SelectionError::NewerFormat`],
    /// whatever the rest holds.
    pub fn from_json(bytes: &[u8]) -> Result<Self, SelectionError> {
        check_format(bytes)?;
        Ok(serde_json::from_slice(bytes)?)
    }

    /// The settings as JSON: deterministic (sorted maps), empty fields left out.
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("string-keyed maps always serialize")
    }

    /// Whether the file or folder `path` (a [`path_key`]) is included: the value of its deepest
    /// saved ancestor-or-self, included when there is none.
    pub fn path_included(&self, path: &str) -> bool {
        self.paths
            .iter()
            .filter(|(node, _)| covers(node, path))
            .max_by_key(|(node, _)| components(node).count())
            .is_none_or(|(_, &included)| included)
    }

    /// Includes or excludes `node` and everything below it: removes every saved entry at or
    /// below `node`, then saves `node` only if `included` differs from what it inherits.
    pub fn set_path(&mut self, node: &str, included: bool) {
        self.paths.retain(|saved, _| !covers(node, saved));
        if self.path_included(node) != included {
            self.paths.insert(node.to_owned(), included);
        }
    }

    /// Whether the manifest registry key `key` is included; absent means included.
    pub fn registry_included(&self, key: &str) -> bool {
        self.registry
            .iter()
            .find(|(saved, _)| saved.eq_ignore_ascii_case(key))
            .is_none_or(|(_, &included)| included)
    }

    /// Includes or excludes the registry key `key`; only an exclusion is saved.
    pub fn set_registry(&mut self, key: &str, included: bool) {
        self.registry
            .retain(|saved, _| !saved.eq_ignore_ascii_case(key));
        if !included {
            self.registry.insert(key.to_owned(), false);
        }
    }

    /// Applies `changes` in order.
    pub fn apply_changes(&mut self, changes: &[Change]) {
        for change in changes {
            match change {
                Change::Skip(skip) => self.skip = *skip,
                Change::Path { node, included } => self.set_path(node, *included),
                Change::Registry { key, included } => self.set_registry(key, *included),
            }
        }
    }
}

/// Fails with [`SelectionError::NewerFormat`] when the JSON object's `format` is above
/// [`FORMAT`], before the rest is parsed: a newer format may change any other field.
pub(crate) fn check_format(bytes: &[u8]) -> Result<(), SelectionError> {
    #[derive(Deserialize)]
    struct Header {
        format: u32,
    }
    let Header { format } = serde_json::from_slice(bytes)?;
    if format > FORMAT {
        return Err(SelectionError::NewerFormat(format));
    }
    Ok(())
}

/// The key of a found file in [`GameSettings::paths`]: `<Base>/<tail>`, its staging path
/// (ADR-0004). A folder's key is a prefix of it, such as `<Base>` or `<Base>/<dir>`.
pub fn path_key(file: &FoundFile) -> String {
    format!("{}/{}", file.base.dir_name(), file.tail)
}

/// `scan` without the files and registry keys `settings` exclude; `None` when the game is
/// skipped or nothing is left, so no empty snapshot is made.
pub fn apply(scan: &GameScan, settings: &GameSettings) -> Option<GameScan> {
    if settings.skip {
        return None;
    }
    let mut kept = scan.clone();
    kept.files
        .retain(|file| settings.path_included(&path_key(file)));
    kept.registry_keys
        .retain(|key| settings.registry_included(key));
    if kept.registry_keys.is_empty() {
        kept.registry_source = None;
    }
    (!kept.files.is_empty() || kept.registry_source.is_some()).then_some(kept)
}

/// Whether paths below the base directory name `base_dir_name` (the first component of a
/// [`path_key`], such as `WinAppData`) compare ignoring ASCII case: true for Windows-family
/// bases, false for the others and for unknown names.
pub fn folds_case(base_dir_name: &str) -> bool {
    serde_json::from_value::<Base>(Value::from(base_dir_name))
        .is_ok_and(|base| base_family(base) == Some(Family::Windows))
}

fn components(key: &str) -> impl Iterator<Item = &str> {
    key.split('/').filter(|part| !part.is_empty())
}

/// Whether `node` is `path` or one of its ancestors, component by component. Below a
/// Windows-family base the tail ignores ASCII case, as the scan does in prefixes.
fn covers(node: &str, path: &str) -> bool {
    let (mut node, mut path) = (components(node), components(path));
    let (Some(base), Some(path_base)) = (node.next(), path.next()) else {
        return false;
    };
    if base != path_base {
        return false;
    }
    let fold = folds_case(base);
    node.all(|part| {
        path.next().is_some_and(|other| {
            if fold {
                part.eq_ignore_ascii_case(other)
            } else {
                part == other
            }
        })
    })
}

#[cfg(test)]
mod tests;
