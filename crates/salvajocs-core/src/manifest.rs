//! Typed subset of the ludusavi manifest schema, its YAML/JSON codecs and custom-game merging
//! (ADR-0008, ADR-0011).

use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use serde_saphyr::{Budget, Options};

use crate::crossbuild::row_pair;

pub mod fetch;

/// Failure to read or write a manifest.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// The YAML text is malformed or exceeds the parse budget.
    #[error("invalid manifest YAML")]
    Yaml(#[from] serde_saphyr::Error),
    /// The JSON cache is malformed.
    #[error("invalid manifest JSON")]
    Json(#[from] serde_json::Error),
}

/// Operating system a rule applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    /// Windows.
    Windows,
    /// Linux.
    Linux,
    /// macOS.
    Mac,
    /// DOS.
    Dos,
    /// A value this version does not know.
    #[serde(other)]
    Other,
}

/// Game store a rule applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Store {
    /// Steam.
    Steam,
    /// GOG.
    Gog,
    /// Epic Games Store.
    Epic,
    /// Microsoft Store.
    Microsoft,
    /// Ubisoft Connect.
    Uplay,
    /// A value this version does not know.
    #[serde(other)]
    Other,
}

/// What a path or key holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tag {
    /// Save data.
    Save,
    /// Settings.
    Config,
    /// A value this version does not know.
    #[serde(other)]
    Other,
}

/// One `when` constraint; an unset field matches anything.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct When {
    /// Operating system the rule applies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<Os>,
    /// Store the rule applies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<Store>,
}

/// A path template of a game and when it applies.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRule {
    /// What the path holds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,
    /// Constraints; the rule applies when any matches, or always when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when: Vec<When>,
}

/// A registry key of a game and when it applies.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryRule {
    /// What the key holds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,
    /// Constraints; the rule applies when any matches, or always when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when: Vec<When>,
}

/// An `installDir` entry; the manifest gives it no content.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallDir {}

/// Steam identity of a game.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Steam {
    /// Steam app id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
}

/// Save locations of one game; fields this version does not use are dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameEntry {
    /// Path templates, keyed by template.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, FileRule>,
    /// Registry keys, keyed by key path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub registry: BTreeMap<String, RegistryRule>,
    /// Folder names the game installs into, under the library's common directory.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub install_dir: BTreeMap<String, InstallDir>,
    /// Steam identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam: Option<Steam>,
}

/// All known games by title.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Manifest {
    /// Entries keyed by game title.
    pub games: BTreeMap<String, GameEntry>,
}

/// How a custom game combines with the manifest entry of the same title.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Integration {
    /// Replace the manifest entry.
    Override,
    /// Append to the manifest entry.
    Extend,
}

/// A Linux and a Windows root that hold the same saves (ADR-0011).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pair {
    /// Glob-free placeholder template on the Linux side.
    pub linux: String,
    /// Glob-free placeholder template on the Windows side.
    pub windows: String,
}

/// One file or folder of a custom game: its path on a native Linux build and in a Windows build
/// (native or in a Wine/Proton prefix). A missing side is not scanned on that system
/// (ADR-0016).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomFile {
    /// Path template on a native Linux build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linux: Option<String>,
    /// Path template in a Windows build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<String>,
}

/// A game defined or adjusted by the user (ADR-0008).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomGame {
    /// Game title; matches manifest titles exactly.
    pub name: String,
    /// File and folder rows; [`CustomGame::game_entry`] turns them into file rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<CustomFile>,
    /// Registry keys and install folders to add or substitute; its `files` are still read but
    /// no longer written (ADR-0016).
    pub entry: GameEntry,
    /// How `entry` combines with the manifest entry.
    pub integration: Integration,
    /// Wine prefix holding this game's saves (ADR-0007).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wine_prefix: Option<PathBuf>,
    /// Saved Linux/Windows root pairs for cross-build restores.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pairs: Vec<Pair>,
    /// Root templates a cross-build restore leaves out, glob-free like the sides of a pair.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skips: Vec<String>,
}

impl CustomGame {
    /// `entry` plus a save-tagged rule per row side, `when.os` `linux` for the Linux side and
    /// `windows` for the Windows side; a template on both sides or in several rows gets both
    /// OSes. A template already in `entry.files` keeps its rule.
    pub fn game_entry(&self) -> GameEntry {
        let mut entry = self.entry.clone();
        let mut rows: BTreeMap<String, FileRule> = BTreeMap::new();
        for file in &self.files {
            let sides = [(&file.linux, Os::Linux), (&file.windows, Os::Windows)];
            for (template, os) in sides {
                let Some(template) = template else { continue };
                let rule = rows.entry(template.clone()).or_insert_with(|| FileRule {
                    tags: vec![Tag::Save],
                    when: Vec::new(),
                });
                if !rule.when.iter().any(|when| when.os == Some(os)) {
                    rule.when.push(When {
                        os: Some(os),
                        store: None,
                    });
                }
            }
        }
        for (template, rule) in rows {
            entry.files.entry(template).or_insert(rule);
        }
        entry
    }

    /// The Linux/Windows mapping of each row with two different sides: their glob-free prefixes,
    /// kept when they pass the check [`crate::crossbuild::validate_file`] makes of a row.
    pub fn file_pairs(&self) -> Vec<Pair> {
        self.files
            .iter()
            .filter_map(|file| {
                let (Some(linux), Some(windows)) = (&file.linux, &file.windows) else {
                    return None;
                };
                (linux != windows)
                    .then(|| row_pair(linux, windows).ok())
                    .flatten()
            })
            .collect()
    }
}

/// Parse budget limit; serde-saphyr's default is too small for the real manifest (ADR-0008).
const BUDGET_LIMIT: usize = 64 * 1024 * 1024;

/// A manifest entry, or the nickname of another title.
#[derive(Deserialize)]
struct RawEntry {
    alias: Option<String>,
    #[serde(flatten)]
    entry: GameEntry,
}

/// Parses manifest YAML, skipping alias entries.
pub fn parse_yaml(yaml: &str) -> Result<Manifest, ManifestError> {
    let mut budget = Budget::default();
    budget.max_events = BUDGET_LIMIT;
    budget.max_nodes = BUDGET_LIMIT;
    let mut options = Options::default();
    options.budget = Some(budget);
    let raw: BTreeMap<String, RawEntry> = serde_saphyr::from_str_with_options(yaml, options)?;
    let games = raw
        .into_iter()
        .filter(|(_, raw)| raw.alias.is_none())
        .map(|(title, raw)| (title, raw.entry))
        .collect();
    Ok(Manifest { games })
}

/// Serializes the manifest as the JSON cache.
pub fn to_json(manifest: &Manifest) -> Result<String, ManifestError> {
    Ok(serde_json::to_string(manifest)?)
}

/// Reads a manifest back from the JSON cache.
pub fn from_json(json: &str) -> Result<Manifest, ManifestError> {
    Ok(serde_json::from_str(json)?)
}

/// Applies custom games in order: `Override` replaces the manifest entry of the same title,
/// `Extend` adds its files, registry keys and install dirs to it (same key: custom wins) and
/// takes the custom `steam` only when the manifest entry has none. A title the manifest lacks
/// is inserted either way.
pub fn merge_custom(manifest: &mut Manifest, custom: &[CustomGame]) {
    for game in custom {
        match (game.integration, manifest.games.get_mut(&game.name)) {
            (Integration::Extend, Some(entry)) => {
                let custom = game.game_entry();
                entry.files.extend(custom.files);
                entry.registry.extend(custom.registry);
                entry.install_dir.extend(custom.install_dir);
                if entry.steam.is_none() {
                    entry.steam = custom.steam;
                }
            }
            _ => {
                manifest.games.insert(game.name.clone(), game.game_entry());
            }
        }
    }
}

#[cfg(test)]
mod tests;
