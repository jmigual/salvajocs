//! Custom games in the repository (ADR-0028): one settings snapshot per game holding
//! `/salvajocs-settings/custom-games/<slug>/custom-game.json`, tagged `salvajocs-settings` and
//! `custom-game:<slug>`, with the name as label. A removed game is a tombstone. The Wine prefix
//! is never stored: it stays in this PC's config, whose `custom_games` holds the ones this PC
//! uses (ADR-0030).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::manifest::{CustomFile, CustomGame, GameEntry, Integration, Pair};
use crate::restore::MAX_JSON_BYTES;
use crate::selection::{Settled, forget_listed, settle};
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
    /// The repository found the new snapshot unchanged from an older one and stored none, or
    /// another PC saved the game meanwhile and the new snapshot was forgotten.
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
/// `wine_prefix`) are neither those of `based_on`, the copy the edit started from (`None`: the
/// repository had none, or a tombstone), nor those of `new`, nothing is written. A name with no
/// snapshot at all is saved whatever `based_on` is, as a first upload; removing it writes
/// nothing. Otherwise
/// writes the game, staged in `stage_root/settings/custom-games/<slug>`, keeping the top-level
/// fields a newer version added to the newest snapshot, and forgets the older snapshots it
/// listed before writing, never one written since; also when nothing new was written.
///
/// # Errors
/// [`CustomGameError::Changed`] with the newest copy, [`CustomGameError::NewerFormat`] or the
/// parse error when the newest snapshot cannot be read, so it is never overwritten,
/// [`CustomGameError::NotWritten`] (also when another PC saved the game meanwhile: the new
/// snapshot is forgotten, so retry), or the I/O or repository error. A failed forget is not an
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
    // The same change made on another PC first is no conflict.
    if read.file.is_some()
        && newest.as_ref() != based_on.map(shared).as_ref()
        && newest.as_ref() != new.map(shared).as_ref()
    {
        return Err(CustomGameError::Changed(newest.map(Box::new)));
    }
    write(storage, name, new, &read, stage_root)
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
    match settle(storage, &tag, &read.ids, &written.id)? {
        Settled::Raced => Err(CustomGameError::NotWritten),
        Settled::Done(forget_error) => Ok(Saved {
            game,
            written: Some(written),
            forget_error,
        }),
    }
}

fn read(
    storage: &mut Storage,
    snapshot: &SnapshotInfo,
    slug: &str,
) -> Result<File, CustomGameError> {
    let path = format!("{}/{GAME_FILE}", as_path(slug));
    File::parse(&storage.read_file(&snapshot.id, &path, MAX_JSON_BYTES)?)
}

/// A custom game changed on this PC and not yet saved to the repository (ADR-0029). The change
/// is the local game of `name`, or its removal when the local copy has no game of that name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Unsent {
    /// The game's name.
    pub name: String,
    /// The copy this PC had before its first unsent change, without a Wine prefix; `None` when
    /// it had no game of that name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub based_on: Option<CustomGame>,
}

impl Unsent {
    /// The change of `name` from `based_on`, whose Wine prefix is dropped.
    pub fn new(name: String, based_on: Option<CustomGame>) -> Self {
        Self {
            name,
            based_on: based_on.as_ref().map(shared),
        }
    }
}

/// Whether `entry` is a game added and removed here: other PCs never had it from this PC.
fn nothing_to_send(games: &[CustomGame], entry: &Unsent) -> bool {
    entry.based_on.is_none() && !games.iter().any(|game| game.name == entry.name)
}

/// What this PC sends for `name`: its game of `games` without the Wine prefix, `None` for a
/// removal.
fn sends(games: &[CustomGame], name: &str) -> Option<CustomGame> {
    games.iter().find(|game| game.name == name).map(shared)
}

/// An unsent custom game whose repository copy changed since its base.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Conflict {
    /// The game's name.
    pub name: String,
    /// The repository's newest copy, without a Wine prefix; `None` when it removed the game.
    pub newest: Option<CustomGame>,
}

/// The entries of `unsent` that conflict with the repository: it has a snapshot of the name
/// whose newest copy is neither the entry's base nor what this PC sends from `games`. A name with
/// no snapshot, or whose newest copy cannot be read, is none, and so is a game added and removed
/// here, which has nothing to send. Reindexes first; writes nothing.
///
/// # Errors
/// The repository error when it cannot be reindexed.
pub fn conflicts(
    storage: &mut Storage,
    games: &[CustomGame],
    unsent: &[Unsent],
) -> Result<Vec<Conflict>, StorageError> {
    storage.reindex()?;
    let mut found = Vec::new();
    for entry in unsent.iter().filter(|entry| !nothing_to_send(games, entry)) {
        // An unreadable copy is the send's to report.
        let Ok(read) = read_newest(storage, &entry.name) else {
            continue;
        };
        let Ok(newest) = read.game() else {
            continue;
        };
        let base = entry.based_on.as_ref().map(shared);
        if read.file.is_some() && newest != base && newest != sends(games, &entry.name) {
            found.push(Conflict {
                name: entry.name.clone(),
                newest,
            });
        }
    }
    Ok(found)
}

/// What a [`send`] did.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Sent {
    /// Each name done, with the copy the repository now holds as this PC sent it (no Wine
    /// prefix; `None` for a removal, or for a game added and removed here, which is not sent),
    /// for [`after_send`].
    pub sent: Vec<(String, Option<CustomGame>)>,
    /// The names of `sent` written as a new snapshot.
    pub written: Vec<String>,
    /// The names that could not be saved, with why; they stay unsent.
    pub failed: Vec<(String, CustomGameError)>,
    /// Removals not tried because a game's write failed for a reason that may pass; they stay
    /// unsent.
    pub held_back: Vec<String>,
    /// Saved names whose older snapshots could not be forgotten.
    pub forget_errors: Vec<(String, StorageError)>,
}

impl Sent {
    fn save(
        &mut self,
        storage: &mut Storage,
        entry: &Unsent,
        game: Option<&CustomGame>,
        stage_root: &Path,
    ) {
        let name = &entry.name;
        match save(storage, name, entry.based_on.as_ref(), game, stage_root) {
            Ok(saved) => {
                if saved.written.is_some() {
                    self.written.push(name.clone());
                }
                if let Some(err) = saved.forget_error {
                    self.forget_errors.push((name.clone(), err));
                }
                self.sent.push((name.clone(), saved.game));
            }
            Err(err) => self.failed.push((name.clone(), err)),
        }
    }
}

/// Saves each entry of `unsent` with [`save`]: first the games this PC has in `games`, then the
/// removals, staged below `stage_root`. The removals are held back when a game's write failed
/// for a reason that may pass (repository, I/O, not written), so a rename never leaves the game
/// missing on other PCs; a failure tied to one game does not hold them back. A game added and
/// removed here is done without a write.
pub fn send(
    storage: &mut Storage,
    games: &[CustomGame],
    unsent: &[Unsent],
    stage_root: &Path,
) -> Sent {
    let mut sent = Sent::default();
    let local = |entry: &Unsent| games.iter().find(|game| game.name == entry.name);
    for entry in unsent {
        if let Some(game) = local(entry) {
            sent.save(storage, entry, Some(game), stage_root);
        }
    }
    let retry = (sent.failed.iter()).any(|(_, err)| {
        matches!(
            err,
            CustomGameError::Storage(_) | CustomGameError::Io { .. } | CustomGameError::NotWritten
        )
    });
    for entry in unsent.iter().filter(|entry| local(entry).is_none()) {
        if nothing_to_send(games, entry) {
            sent.sent.push((entry.name.clone(), None));
        } else if retry {
            sent.held_back.push(entry.name.clone());
        } else {
            sent.save(storage, entry, None, stage_root);
        }
    }
    sent
}

/// Updates `unsent` once the repository holds the copies `sent` ([`Sent::sent`]): an entry
/// whose game in `games` is still what was sent is dropped; one changed since, during the
/// backup, stays, based on what was sent.
pub fn after_send(
    unsent: &mut Vec<Unsent>,
    games: &[CustomGame],
    sent: &[(String, Option<CustomGame>)],
) {
    for (name, copy) in sent {
        let Some(index) = unsent.iter().position(|entry| entry.name == *name) else {
            continue;
        };
        if sends(games, name) == *copy {
            unsent.remove(index);
        } else {
            unsent[index].based_on = copy.clone();
        }
    }
}

/// This PC's custom games after a [`merge`] with the repository's.
#[derive(Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Merged {
    /// The new local copy, in the local order. It never gains a game or a change from the
    /// repository: those are [`Merged::offered`].
    pub games: Vec<CustomGame>,
    /// The repository's games that are new here or differ from this PC's copy, without a Wine
    /// prefix, by name; they are used here only once the user accepts them (ADR-0030).
    pub offered: Vec<CustomGame>,
    /// Local games a migration keeps that the repository lacks or holds otherwise, as unsent
    /// changes based on this PC's copy, so Back up saves them or asks.
    pub new_unsent: Vec<Unsent>,
    /// Whether every local game was compared with the repository's copy, so the copy may be
    /// marked as mirroring it; a migration that met an unreadable copy is not.
    pub complete: bool,
}

/// Merges the repository's custom games `listed` ([`load_all`]) into this PC's `local` copy
/// (ADR-0029, ADR-0030). Every game of [`Merged::games`] was edited on this PC, kept from
/// `local`, or accepted on this PC with its exact shared content: the merge never adds or
/// replaces a game from the repository, it only drops one the repository removed.
///
/// A name with an entry in `unsent` stays as this PC has it, a removal included, and is never
/// offered. When `mirrors` (the copy mirrors this repository), each other local game stays and
/// a different repository copy is offered; a removed one is dropped; an unreadable one stays,
/// and so does one the repository has no snapshot of, as a new game to send, so a copy wrongly
/// marked as mirroring loses nothing. Otherwise (a migration) each local game stays: one equal
/// to the repository's copy as it is, any other as a new unsent change. In both, the
/// repository's games the copy and `unsent` lack are offered.
pub fn merge(
    local: &[CustomGame],
    unsent: &[Unsent],
    listed: &BTreeMap<String, Result<Stored, CustomGameError>>,
    mirrors: bool,
) -> Merged {
    let is_unsent = |name: &str| unsent.iter().any(|entry| entry.name == name);
    let mut merged = Merged {
        complete: true,
        ..Merged::default()
    };
    for game in local {
        let stored = listed.get(&game.name);
        if is_unsent(&game.name) {
            merged.games.push(game.clone());
            continue;
        }
        match stored {
            Some(Ok(Stored::Game(stored))) if mirrors => {
                if shared(game) != **stored {
                    merged.offered.push((**stored).clone());
                }
                merged.games.push(game.clone());
            }
            Some(Ok(Stored::Removed)) if mirrors => {}
            None if mirrors => {
                merged.new_unsent.push(Unsent::new(game.name.clone(), None));
                merged.games.push(game.clone());
            }
            Some(Ok(Stored::Game(stored))) if shared(game) == **stored => {
                merged.games.push(game.clone());
            }
            Some(Err(_)) => {
                // Migrating, the local copy was never compared with the repository's.
                merged.complete &= mirrors;
                merged.games.push(game.clone());
            }
            Some(Ok(_)) | None => {
                let entry = Unsent::new(game.name.clone(), Some(game.clone()));
                merged.new_unsent.push(entry);
                merged.games.push(game.clone());
            }
        }
    }
    for (name, stored) in listed {
        if let Ok(Stored::Game(stored)) = stored
            && !local.iter().any(|game| game.name == *name)
            && !is_unsent(name)
        {
            merged.offered.push((**stored).clone());
        }
    }
    // The first loop offers in the local order.
    merged.offered.sort_by(|a, b| a.name.cmp(&b.name));
    merged
}

#[cfg(test)]
mod tests;
