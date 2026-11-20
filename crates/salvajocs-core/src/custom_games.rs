//! Custom games in the repository (ADR-0028): one settings snapshot per game holding
//! `/salvajocs-settings/custom-games/<slug>/custom-game.json`, tagged `salvajocs-settings` and
//! `custom-game:<slug>`, with the name as label. A removed game is a tombstone. The Wine prefix
//! is never stored: it stays in this PC's config, whose `custom_games` is a copy of these.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::crossbuild::{self, Choice, SnapshotRoot};
use crate::manifest::{CustomFile, CustomGame, GameEntry, Integration, Pair};
use crate::selection::forget_listed;
use crate::slug::slug;
use crate::storage::{SnapshotInfo, Storage, StorageError};

/// The format of `custom-game.json` this version reads and writes.
pub const FORMAT: u32 = 1;

/// The tag every settings snapshot carries; never `salvajocs`, so the restore list skips them.
const SETTINGS_TAG: &str = "salvajocs-settings";
/// Never `settings:`: salvajocs 0.4 to 0.7 read every such tag as a game's selection.
const TAG_PREFIX: &str = "custom-game:";
const GAME_FILE: &str = "custom-game.json";
/// Where saves stage, below `<stage root>/settings`: never a slug, which always holds a hex hash.
const STAGE_DIR: &str = "custom-games";

fn game_tag(slug: &str) -> String {
    format!("{TAG_PREFIX}{slug}")
}

fn as_path(slug: &str) -> String {
    format!("/salvajocs-settings/{STAGE_DIR}/{slug}")
}

/// Errors from reading or saving custom games in the repository.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CustomGameError {
    /// The game was written by a newer version: it is neither applied nor rewritten.
    #[error("custom game format {0} is newer than this version understands")]
    NewerFormat(u32),
    /// The game is not valid JSON for its format.
    #[error("cannot parse the custom game")]
    Parse(#[from] serde_json::Error),
    /// The game's file could not be staged.
    #[error("I/O failed on {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The repository failed.
    #[error("repository error")]
    Storage(#[from] StorageError),
    /// The repository found the new snapshot unchanged from an older one and stored none.
    #[error("the custom game snapshot was not written")]
    NotWritten,
    /// The repository's newest copy is not the one the edit started from, so nothing was
    /// written. Holds that copy, `None` when the game is removed (a tombstone).
    #[error("the custom game changed in the repository since it was read")]
    Changed(Option<Box<CustomGame>>),
}

/// What the repository holds for a custom game's name.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Stored {
    /// The game, without a Wine prefix.
    Game(Box<CustomGame>),
    /// The game was removed: its newest snapshot is a tombstone.
    Removed,
}

/// The contents of `custom-game.json`: a game's shared fields, or a tombstone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct File {
    format: u32,
    name: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    removed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    integration: Option<Integration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<CustomFile>,
    #[serde(default, skip_serializing_if = "is_empty_entry")]
    entry: GameEntry,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pairs: Vec<Pair>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    skips: Vec<String>,
    /// Top-level fields of a newer version, kept so rewriting the game never drops them.
    #[serde(flatten)]
    other: BTreeMap<String, Value>,
}

fn is_empty_entry(entry: &GameEntry) -> bool {
    *entry == GameEntry::default()
}

impl File {
    /// The file of `game` under `name`, with the newer version's fields `other`.
    fn game(name: &str, game: &CustomGame, other: BTreeMap<String, Value>) -> Self {
        Self {
            format: FORMAT,
            name: name.to_owned(),
            removed: false,
            integration: Some(game.integration),
            files: game.files.clone(),
            entry: game.entry.clone(),
            pairs: game.pairs.clone(),
            skips: game.skips.clone(),
            other,
        }
    }

    fn tombstone(name: &str) -> Self {
        Self {
            format: FORMAT,
            name: name.to_owned(),
            removed: true,
            integration: None,
            files: Vec::new(),
            entry: GameEntry::default(),
            pairs: Vec::new(),
            skips: Vec::new(),
            other: BTreeMap::new(),
        }
    }

    /// Parses `custom-game.json`. A `format` above [`FORMAT`] is
    /// [`CustomGameError::NewerFormat`], whatever the rest holds.
    fn parse(bytes: &[u8]) -> Result<Self, CustomGameError> {
        #[derive(Deserialize)]
        struct Header {
            format: u32,
        }
        let Header { format } = serde_json::from_slice(bytes)?;
        if format > FORMAT {
            return Err(CustomGameError::NewerFormat(format));
        }
        Ok(serde_json::from_slice(bytes)?)
    }

    fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("string-keyed maps always serialize")
    }

    /// The game this file holds; a game without an integration does not parse.
    fn stored(&self) -> Result<Stored, CustomGameError> {
        if self.removed {
            return Ok(Stored::Removed);
        }
        let integration = self
            .integration
            .ok_or_else(|| <serde_json::Error as serde::de::Error>::missing_field("integration"))?;
        let mut game = CustomGame::new(self.name.clone(), self.entry.clone(), integration);
        game.files = self.files.clone();
        game.pairs = self.pairs.clone();
        game.skips = self.skips.clone();
        Ok(Stored::Game(Box::new(game)))
    }
}

/// `game` as the repository holds it: without its Wine prefix.
fn shared(game: &CustomGame) -> CustomGame {
    CustomGame {
        wine_prefix: None,
        ..game.clone()
    }
}

/// The newest stored state of every custom game the repository holds, by name. A game whose
/// newest snapshot cannot be read or is of a newer format has its error instead, under the
/// snapshot's label. Reindexes first, so another PC's newer snapshot is read.
///
/// # Errors
/// The repository error when the repository cannot be reindexed or listed.
pub fn load_all(
    storage: &mut Storage,
) -> Result<BTreeMap<String, Result<Stored, CustomGameError>>, StorageError> {
    storage.reindex()?;
    let mut newest: BTreeMap<String, SnapshotInfo> = BTreeMap::new();
    for snapshot in storage.snapshots(SETTINGS_TAG)? {
        if let Some(slug) = snapshot
            .tags
            .iter()
            .find_map(|tag| tag.strip_prefix(TAG_PREFIX))
        {
            newest.entry(slug.to_owned()).or_insert(snapshot);
        }
    }
    Ok(newest
        .into_iter()
        .map(|(slug, snapshot)| match read(storage, &snapshot, &slug) {
            Ok(file) => (file.name.clone(), file.stored()),
            Err(err) => (snapshot.label, Err(err)),
        })
        .collect())
}

/// The outcome of a save that succeeded.
#[derive(Debug)]
#[non_exhaustive]
pub struct Saved {
    /// The game as the repository now holds it, without a Wine prefix; `None` when removed.
    pub game: Option<CustomGame>,
    /// The new snapshot, `None` when the repository already held the game as saved.
    pub written: Option<SnapshotInfo>,
    /// Why the game's older snapshots could not be forgotten; a later save forgets them.
    pub forget_error: Option<StorageError>,
}

/// Saves the custom game `name`: `new` (its own name is not used), or a tombstone when `new` is
/// `None`. Reindexes and reads the game's newest snapshot; when its shared fields (all but
/// `wine_prefix`) are not those of `based_on`, the copy the edit started from (`None`: the
/// repository had none, or a tombstone), nothing is written. A name with no snapshot at all is
/// saved whatever `based_on` is, as a sync uploads it; removing it writes nothing. Otherwise
/// writes the game, staged in `stage_root/settings/custom-games/<slug>`, keeping the top-level
/// fields a newer version added to the newest snapshot, and forgets the older snapshots it
/// listed before writing, never one written since; also when nothing new was written.
///
/// # Errors
/// [`CustomGameError::Changed`] with the newest copy, [`CustomGameError::NewerFormat`] or the
/// parse error when the newest snapshot cannot be read, so it is never overwritten,
/// [`CustomGameError::NotWritten`], or the I/O or repository error. A failed forget is not an
/// error: it is in [`Saved::forget_error`].
pub fn save(
    storage: &mut Storage,
    name: &str,
    based_on: Option<&CustomGame>,
    new: Option<&CustomGame>,
    stage_root: &Path,
) -> Result<Saved, CustomGameError> {
    // The newest snapshot may be another PC's, written after the index was loaded.
    storage.reindex()?;
    let read = read_newest(storage, name)?;
    let newest = read.game()?;
    if read.file.is_some() && newest.as_ref() != based_on.map(shared).as_ref() {
        return Err(CustomGameError::Changed(newest.map(Box::new)));
    }
    write(storage, name, new, &read, stage_root)
}

/// Saves the restore mapping `choices` of the game `title` ([`crossbuild::apply_choices`]) on
/// the repository's newest copy of it, so another PC's newer rows are kept. When the repository
/// has no snapshot of it at all (its upload failed), they go on `local`, this PC's copy; a game
/// neither has, or the repository removed, becomes an `Extend` game. Writes and forgets as
/// [`save`] does.
///
/// # Errors
/// As [`save`], but never [`CustomGameError::Changed`].
pub fn save_choices(
    storage: &mut Storage,
    title: &str,
    local: Option<&CustomGame>,
    choices: &[(SnapshotRoot, Choice)],
    stage_root: &Path,
) -> Result<Saved, CustomGameError> {
    storage.reindex()?;
    let read = read_newest(storage, title)?;
    let mut games: Vec<CustomGame> = match read.file {
        None => local.map(shared).into_iter().collect(),
        Some(_) => read.game()?.into_iter().collect(),
    };
    crossbuild::apply_choices(&mut games, title, choices);
    write(storage, title, games.first(), &read, stage_root)
}

/// A game's newest snapshot as read for a save, with the ids of all its snapshots listed then,
/// newest first.
struct Read {
    ids: Vec<String>,
    file: Option<File>,
}

impl Read {
    /// The game the newest snapshot holds; `None` for none or a tombstone.
    fn game(&self) -> Result<Option<CustomGame>, CustomGameError> {
        Ok(match self.file.as_ref().map(File::stored).transpose()? {
            Some(Stored::Game(game)) => Some(*game),
            Some(Stored::Removed) | None => None,
        })
    }
}

fn read_newest(storage: &mut Storage, name: &str) -> Result<Read, CustomGameError> {
    let slug = slug(name);
    let listed = storage.snapshots(&game_tag(&slug))?;
    let file = match listed.first() {
        Some(newest) => Some(read(storage, newest, &slug)?),
        None => None,
    };
    Ok(Read {
        ids: listed.into_iter().map(|snapshot| snapshot.id).collect(),
        file,
    })
}

fn write(
    storage: &mut Storage,
    name: &str,
    game: Option<&CustomGame>,
    read: &Read,
    stage_root: &Path,
) -> Result<Saved, CustomGameError> {
    let slug = slug(name);
    let tag = game_tag(&slug);
    let file = match game {
        Some(game) => {
            let other = (read.file.as_ref()).map_or_else(BTreeMap::new, |file| file.other.clone());
            File::game(name, game, other)
        }
        None => File::tombstone(name),
    };
    let game = game.map(|game| CustomGame {
        name: name.to_owned(),
        ..shared(game)
    });
    // Removing a game the repository never had leaves nothing to mark.
    let unchanged = match &read.file {
        Some(current) => *current == file,
        None => game.is_none(),
    };
    if unchanged {
        let older = read.ids.get(1..).unwrap_or_default();
        return Ok(Saved {
            game,
            written: None,
            forget_error: forget_listed(storage, &tag, older),
        });
    }
    let dir = stage_root.join("settings").join(STAGE_DIR).join(&slug);
    let path = dir.join(GAME_FILE);
    fs::create_dir_all(&dir)
        .and_then(|()| fs::write(&path, file.to_json()))
        .map_err(|source| CustomGameError::Io { path, source })?;
    let written = storage
        .backup_dir(&dir, &as_path(&slug), &[SETTINGS_TAG, &tag], name)?
        .ok_or(CustomGameError::NotWritten)?;
    Ok(Saved {
        game,
        written: Some(written),
        forget_error: forget_listed(storage, &tag, &read.ids),
    })
}

fn read(
    storage: &mut Storage,
    snapshot: &SnapshotInfo,
    slug: &str,
) -> Result<File, CustomGameError> {
    let path = format!("{}/{GAME_FILE}", as_path(slug));
    File::parse(&storage.read_file(&snapshot.id, &path)?)
}

/// This PC's custom games after a [`merge`] with the repository's.
#[derive(Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Merged {
    /// The new local copy: the local order, then the repository's other games by name.
    pub games: Vec<CustomGame>,
    /// Local games the repository has no snapshot of, to upload; only when migrating.
    pub upload: Vec<String>,
    /// Local games whose copy differed from the repository's, which replaced it; only when
    /// migrating.
    pub replaced: Vec<String>,
    /// Local games dropped because the repository removed them; only when migrating.
    pub removed: Vec<String>,
}

/// Merges the repository's custom games `listed` ([`load_all`]) into this PC's `local` copy.
/// Each local game in order takes the repository's shared fields and keeps its Wine prefix; a
/// removed one is dropped; an unreadable one stays as it is. Then the repository's games the
/// copy lacks are appended by name. A local game the repository has no snapshot of is dropped
/// when `mirrors` (the copy mirrors this repository), else it is kept and listed to upload.
pub fn merge(
    local: &[CustomGame],
    listed: &BTreeMap<String, Result<Stored, CustomGameError>>,
    mirrors: bool,
) -> Merged {
    let mut merged = Merged::default();
    for game in local {
        match listed.get(&game.name) {
            Some(Ok(Stored::Game(stored))) => {
                if !mirrors && shared(game) != **stored {
                    merged.replaced.push(game.name.clone());
                }
                merged.games.push(CustomGame {
                    wine_prefix: game.wine_prefix.clone(),
                    ..(**stored).clone()
                });
            }
            Some(Ok(Stored::Removed)) => {
                if !mirrors {
                    merged.removed.push(game.name.clone());
                }
            }
            Some(Err(_)) => merged.games.push(game.clone()),
            None if mirrors => {}
            None => {
                merged.upload.push(game.name.clone());
                merged.games.push(game.clone());
            }
        }
    }
    for (name, stored) in listed {
        if let Ok(Stored::Game(stored)) = stored
            && !local.iter().any(|game| game.name == *name)
        {
            merged.games.push((**stored).clone());
        }
    }
    merged
}

/// What a [`sync`] found and did.
#[derive(Debug)]
#[non_exhaustive]
pub struct Synced {
    /// This PC's new copy of the custom games ([`Merged::games`]).
    pub games: Vec<CustomGame>,
    /// Local games uploaded to the repository.
    pub uploaded: Vec<String>,
    /// Local games the repository's version replaced ([`Merged::replaced`]).
    pub replaced: Vec<String>,
    /// Local games dropped because the repository removed them ([`Merged::removed`]).
    pub removed: Vec<String>,
    /// Local games that could not be uploaded, with why; they stay in `games`.
    pub failed: Vec<(String, CustomGameError)>,
    /// Whether the copy may be marked as mirroring the repository: no upload failed and, when
    /// migrating, no local game's repository copy was unreadable. Until then a mirror sync would
    /// drop or replace those games without the migration's notices.
    pub complete: bool,
    /// Games whose newest snapshot cannot be used, by name.
    pub unreadable: BTreeMap<String, CustomGameError>,
    /// Uploaded games whose older snapshots could not be forgotten.
    pub forget_errors: Vec<(String, StorageError)>,
}

/// Loads the repository's custom games, [`merge`]s them into `local` and uploads the local
/// games the merge lists, staged below `stage_root`.
///
/// # Errors
/// The repository error when the custom games cannot be listed; an upload's error is in
/// [`Synced::failed`].
pub fn sync(
    storage: &mut Storage,
    local: &[CustomGame],
    mirrors: bool,
    stage_root: &Path,
) -> Result<Synced, StorageError> {
    let listed = load_all(storage)?;
    let merged = merge(local, &listed, mirrors);
    let kept_unreadable =
        !mirrors && (local.iter()).any(|game| matches!(listed.get(&game.name), Some(Err(_))));
    let mut synced = Synced {
        games: merged.games,
        uploaded: Vec::new(),
        replaced: merged.replaced,
        removed: merged.removed,
        failed: Vec::new(),
        complete: false,
        unreadable: (listed.into_iter())
            .filter_map(|(name, stored)| Some((name, stored.err()?)))
            .collect(),
        forget_errors: Vec::new(),
    };
    for game in local
        .iter()
        .filter(|game| merged.upload.contains(&game.name))
    {
        let name = game.name.clone();
        match save(storage, &name, None, Some(game), stage_root) {
            Ok(saved) => {
                if let Some(err) = saved.forget_error {
                    synced.forget_errors.push((name.clone(), err));
                }
                synced.uploaded.push(name);
            }
            Err(err) => synced.failed.push((name, err)),
        }
    }
    synced.complete = synced.failed.is_empty() && !kept_unreadable;
    Ok(synced)
}

#[cfg(test)]
mod tests;
