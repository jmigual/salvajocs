//! Settings snapshots: one per game, stored as `/salvajocs-settings/<slug>/settings.json` with
//! tags `salvajocs-settings` and `settings:<slug>` and the title as label (ADR-0014).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use super::{Change, GameSettings, SelectionError};
use crate::restore::MAX_JSON_BYTES;
use crate::retention::POLICY_TAG;
use crate::slug::slug;
use crate::storage::{SnapshotInfo, Storage, StorageError};

/// The tag every settings snapshot carries; never `salvajocs`, so the restore list skips them.
const SETTINGS_TAG: &str = "salvajocs-settings";
const SETTINGS_FILE: &str = "settings.json";

fn game_tag(slug: &str) -> String {
    format!("settings:{slug}")
}

fn as_path(slug: &str) -> String {
    format!("/salvajocs-settings/{slug}")
}

/// Where [`save`] stages, below its `stage_root`: never a slug, which always holds a hex hash.
const STAGE_DIR: &str = "settings";

/// The newest settings of every game that has any, by title (the snapshot label). A game whose
/// settings cannot be read or are of a newer format has its error instead. Reindexes first, so
/// another machine's newer snapshot can be read.
///
/// # Errors
/// The repository error when the repository cannot be reindexed or listed.
pub fn load_all(
    storage: &mut Storage,
) -> Result<BTreeMap<String, Result<GameSettings, SelectionError>>, StorageError> {
    storage.reindex()?;
    let mut newest: BTreeMap<String, SnapshotInfo> = BTreeMap::new();
    for snapshot in storage.snapshots(SETTINGS_TAG)? {
        // The retention policy is a settings snapshot too, but of no game.
        if snapshot.tags.iter().any(|tag| tag == POLICY_TAG) {
            continue;
        }
        if let Some(slug) = snapshot
            .tags
            .iter()
            .find_map(|tag| tag.strip_prefix("settings:"))
        {
            newest.entry(slug.to_owned()).or_insert(snapshot);
        }
    }
    Ok(newest
        .into_iter()
        .map(|(slug, snapshot)| {
            let settings = read(storage, &snapshot, &slug);
            (snapshot.label, settings)
        })
        .collect())
}

/// The outcome of a [`save`] that succeeded: the game's newest settings hold the changes, so
/// the caller may drop them from its unsaved changes.
#[derive(Debug)]
#[non_exhaustive]
pub struct Saved {
    /// The new settings snapshot, `None` when the changes changed nothing.
    pub written: Option<SnapshotInfo>,
    /// Why the game's older settings snapshots could not be forgotten; they stay until a later
    /// save forgets them.
    pub forget_error: Option<StorageError>,
}

/// Saves `changes` for the game `title`: reindexes, reads the game's newest settings, applies
/// `changes` and writes the result as a new settings snapshot, staged in
/// `stage_root/settings/<slug>`. Then forgets the game's older settings snapshots it listed
/// before writing, never one written since; also when nothing new was written.
///
/// # Errors
/// [`SelectionError::NewerFormat`] when the newest settings are of a newer format, the parse
/// error when they are corrupt, [`SelectionError::NotWritten`] (also when another machine saved
/// the game meanwhile: the new snapshot is forgotten, so retry), or the I/O or repository error.
/// A failed forget is not an error: it is in [`Saved::forget_error`].
pub fn save(
    storage: &mut Storage,
    title: &str,
    changes: &[Change],
    stage_root: &Path,
) -> Result<Saved, SelectionError> {
    // The newest settings may be another machine's, written after the index was loaded.
    storage.reindex()?;
    let read = read_newest(storage, title)?;
    write(storage, title, changes, &read, stage_root)
}

/// A game's settings as read for a save, with the ids of all its snapshots listed then, newest
/// first.
struct Read {
    ids: Vec<String>,
    settings: GameSettings,
}

fn read_newest(storage: &mut Storage, title: &str) -> Result<Read, SelectionError> {
    let slug = slug(title);
    let listed = storage.snapshots(&game_tag(&slug))?;
    let settings = match listed.first() {
        Some(newest) => read(storage, newest, &slug)?,
        None => GameSettings::default(),
    };
    Ok(Read {
        ids: listed.into_iter().map(|snapshot| snapshot.id).collect(),
        settings,
    })
}

fn write(
    storage: &mut Storage,
    title: &str,
    changes: &[Change],
    read: &Read,
    stage_root: &Path,
) -> Result<Saved, SelectionError> {
    let slug = slug(title);
    let tag = game_tag(&slug);
    let mut settings = read.settings.clone();
    settings.apply_changes(changes);
    if settings == read.settings {
        // The newest snapshot already holds the settings; only older leftovers go.
        let older = read.ids.get(1..).unwrap_or_default();
        return Ok(Saved {
            written: None,
            forget_error: forget_listed(storage, &tag, older),
        });
    }
    let dir = stage_root.join(STAGE_DIR).join(&slug);
    let file = dir.join(SETTINGS_FILE);
    fs::create_dir_all(&dir)
        .and_then(|()| fs::write(&file, settings.to_json()))
        .map_err(|source| SelectionError::Io { path: file, source })?;
    // The fresh mtime keeps rustic's skip-if-unchanged from matching an older snapshot; if it
    // still does, the changes may be in no newest snapshot, so the caller must keep them.
    let written = storage
        .backup_dir(&dir, &as_path(&slug), &[SETTINGS_TAG, &tag], title, true)?
        .ok_or(SelectionError::NotWritten)?;
    match settle(storage, &tag, &read.ids, &written.id)? {
        Settled::Raced => Err(SelectionError::NotWritten),
        Settled::Done(forget_error) => Ok(Saved {
            written: Some(written),
            forget_error,
        }),
    }
}

/// What [`settle`] did after a save wrote its snapshot.
pub(crate) enum Settled {
    /// Another machine saved meanwhile; the save's own snapshot was forgotten.
    Raced,
    /// The snapshot stands; holds why the older snapshots the save read could not be forgotten.
    Done(Option<StorageError>),
}

/// Lists `tag` once after a save wrote `written`, having read the snapshots `read`. When a
/// snapshot it did not read is there, another machine saved from the same one, so each lacks the
/// other's changes: `written` is forgotten and the caller's save must not count as done (keeping
/// it would let a retry forget the other's snapshot as older). Otherwise forgets those of `read`
/// still listed.
///
/// # Errors
/// The repository error when listing fails or `written` cannot be forgotten.
pub(crate) fn settle(
    storage: &Storage,
    tag: &str,
    read: &[String],
    written: &str,
) -> Result<Settled, StorageError> {
    let listed = storage.snapshots(tag)?;
    if (listed.iter()).any(|snapshot| snapshot.id != written && !read.contains(&snapshot.id)) {
        storage.forget(&[written])?;
        return Ok(Settled::Raced);
    }
    Ok(Settled::Done(forget_stale(storage, &listed, read)))
}

/// Forgets those of `ids` still listed under `tag`, so one another machine already forgot is
/// not forgotten twice. Returns the error rather than failing: the settings are saved either way.
pub(crate) fn forget_listed(storage: &Storage, tag: &str, ids: &[String]) -> Option<StorageError> {
    if ids.is_empty() {
        return None;
    }
    let listed = match storage.snapshots(tag) {
        Ok(listed) => listed,
        Err(err) => return Some(err),
    };
    forget_stale(storage, &listed, ids)
}

/// Forgets those of `ids` among the `listed` snapshots.
fn forget_stale(
    storage: &Storage,
    listed: &[SnapshotInfo],
    ids: &[String],
) -> Option<StorageError> {
    let stale: Vec<&str> = listed
        .iter()
        .map(|snapshot| snapshot.id.as_str())
        .filter(|id| ids.iter().any(|read| read == id))
        .collect();
    if stale.is_empty() {
        return None;
    }
    storage.forget(&stale).err()
}

fn read(
    storage: &mut Storage,
    snapshot: &SnapshotInfo,
    slug: &str,
) -> Result<GameSettings, SelectionError> {
    let path = format!("{}/{SETTINGS_FILE}", as_path(slug));
    GameSettings::from_json(&storage.read_file(&snapshot.id, &path, MAX_JSON_BYTES)?)
}

#[cfg(test)]
mod tests;
