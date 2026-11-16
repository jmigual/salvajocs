//! Stages each game into a mirror directory and backs it up as one snapshot (ADR-0004,
//! ADR-0010).

use std::collections::HashSet;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::layout::{
    Base, FILES_DIR, META_FILE, REGISTRY_FILE, SNAPSHOT_FORMAT, SnapshotMeta, target_path,
};
#[cfg(windows)]
use crate::registry::windows::WindowsRegistryError;
use crate::registry::wine::RegFile;
use crate::registry::{RegistryError, RegistryExport};
use crate::scan::{GameScan, RegistrySource};
use crate::slug::slug;
use crate::storage::{SnapshotInfo, Storage, StorageError};

/// Errors from staging or backing up one game.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    /// A save file could not be read or the staging mirror could not be written.
    #[error("I/O failed on {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// A found file's tail is not a plain relative path, so it would leave the staging dir.
    #[error("unsafe file tail: {0}")]
    UnsafeTail(String),
    /// The staged `salvajocs.json` cannot be parsed.
    #[error("cannot parse the staged {path}")]
    Meta {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    /// The `user.reg` the scan found the game's keys in cannot be parsed or exported.
    #[error("cannot export the registry from {path}")]
    WineRegistry {
        path: PathBuf,
        #[source]
        source: RegistryError,
    },
    /// The live Windows registry cannot be exported.
    #[cfg(windows)]
    #[error("cannot export the Windows registry")]
    WindowsRegistry(#[from] WindowsRegistryError),
    /// The scan recorded the live Windows registry as the source, but this is not Windows.
    #[cfg(not(windows))]
    #[error("the Windows registry is not available on this OS")]
    NoWindowsRegistry,
    /// The repository failed.
    #[error("repository error")]
    Storage(#[from] StorageError),
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> BackupError + '_ {
    move |source| BackupError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Syncs `stage_dir` with `game` and `registry` (ADR-0004): each found file is copied to
/// `files/<Base>/<tail>` when its size or mtime differs, and gets the source's mtime. Staged
/// files no longer found are removed, with the directories they leave empty. `salvajocs.json`
/// and `registry.json` are written only when their bytes change; an empty `registry` means no
/// `registry.json`.
///
/// When `game.unreadable` is not empty nothing is removed, `registry.json` included, and the
/// roots already in `salvajocs.json` are kept after the scan's: a read error must not drop
/// saves from the next snapshot or from a restore. The found files are only read.
pub fn stage(
    game: &GameScan,
    registry: &RegistryExport,
    stage_dir: &Path,
) -> Result<(), BackupError> {
    fs::create_dir_all(stage_dir).map_err(io_error(stage_dir))?;
    let files_dir = stage_dir.join(FILES_DIR);
    let mut staged = Vec::with_capacity(game.files.len());
    for file in &game.files {
        let dest = target_path(&files_dir.join(file.base.dir_name()), &file.tail, false)
            .ok_or_else(|| BackupError::UnsafeTail(file.tail.clone()))?;
        staged.push((file.real.as_path(), dest));
    }
    let keep_all = !game.unreadable.is_empty();
    if !keep_all {
        // Removing first lets a file replace a directory of the same name, or a case change.
        let keep: HashSet<&Path> = staged.iter().map(|(_, dest)| dest.as_path()).collect();
        remove_stale(&files_dir, &keep)?;
    }
    for (real, dest) in &staged {
        sync_file(real, dest)?;
    }

    let meta_file = stage_dir.join(META_FILE);
    let mut roots = game.roots.clone();
    if keep_all {
        for root in staged_roots(&meta_file)? {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
    }
    let meta = SnapshotMeta {
        format: SNAPSHOT_FORMAT,
        title: game.title.clone(),
        roots,
    };
    write_if_changed(&meta_file, &json(&meta))?;
    let registry_file = stage_dir.join(REGISTRY_FILE);
    if !registry.keys.is_empty() {
        write_if_changed(&registry_file, &json(registry))?;
    } else if !keep_all {
        match fs::remove_file(&registry_file) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => {
                return Err(io_error(&registry_file)(err));
            }
            _ => {}
        }
    }
    Ok(())
}

/// The roots of the staged `salvajocs.json`, none when there is none yet.
fn staged_roots(meta_file: &Path) -> Result<Vec<(Base, String)>, BackupError> {
    let bytes = match fs::read(meta_file) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(io_error(meta_file)(err)),
    };
    let meta: SnapshotMeta =
        serde_json::from_slice(&bytes).map_err(|source| BackupError::Meta {
            path: meta_file.to_path_buf(),
            source,
        })?;
    Ok(meta.roots)
}

fn json(value: &impl Serialize) -> Vec<u8> {
    serde_json::to_vec_pretty(value).expect("snapshot meta and registry exports always serialize")
}

/// Removes every file below `dir` that is not in `keep`, then the directories left empty.
/// Symlinks are removed, never followed. Returns whether `dir` is empty afterwards.
fn remove_stale(dir: &Path, keep: &HashSet<&Path>) -> Result<bool, BackupError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(err) => return Err(io_error(dir)(err)),
    };
    let mut empty = true;
    for entry in entries {
        let entry = entry.map_err(io_error(dir))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(io_error(&path))?;
        if kind.is_dir() {
            if remove_stale(&path, keep)? {
                fs::remove_dir(&path).map_err(io_error(&path))?;
            } else {
                empty = false;
            }
        } else if keep.contains(path.as_path()) {
            empty = false;
        } else {
            fs::remove_file(&path).map_err(io_error(&path))?;
        }
    }
    Ok(empty)
}

/// Copies `real` to `dest` unless both have the same size and mtime, then gives `dest` the
/// source's mtime. Size and mtime are read before copying, so a save written during the copy
/// differs again on the next run and is copied once more.
fn sync_file(real: &Path, dest: &Path) -> Result<(), BackupError> {
    let mut source = File::open(real).map_err(io_error(real))?;
    let meta = source.metadata().map_err(io_error(real))?;
    let modified = meta.modified().map_err(io_error(real))?;
    match fs::metadata(dest) {
        Ok(staged) if staged.len() == meta.len() && staged.modified().ok() == Some(modified) => {
            return Ok(());
        }
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(io_error(dest)(err)),
        _ => {}
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(io_error(parent))?;
    }
    // A fresh writable copy rather than `fs::copy`, which would carry a read-only permission
    // over and make the next copy fail.
    let mut copy = File::create(dest).map_err(io_error(dest))?;
    io::copy(&mut source, &mut copy).map_err(io_error(real))?;
    copy.set_modified(modified).map_err(io_error(dest))
}

/// Writes `bytes` to `path` unless it already holds exactly them, so an unchanged file keeps
/// its mtime.
fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<(), BackupError> {
    match fs::read(path) {
        Ok(old) if old == bytes => Ok(()),
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(io_error(path)(err)),
        _ => fs::write(path, bytes).map_err(io_error(path)),
    }
}

/// The outcome of one game in [`backup_games`].
#[derive(Debug)]
pub struct GameBackup {
    /// The game's title.
    pub title: String,
    /// The game's slug (ADR-0010).
    pub slug: String,
    /// The new snapshot, `None` when nothing changed since the last one, or why it failed.
    pub result: Result<Option<SnapshotInfo>, BackupError>,
}

/// Backs up each of `games` as its own snapshot (ADR-0010), one after another: the game is
/// staged in `stage_root/<slug>` with its registry keys read fresh from the scan's
/// `registry_source`, then stored as `/salvajocs/<slug>` with label = title and tags
/// `salvajocs`, `game:<slug>` and `extra_tags` (which must not contain commas). An unchanged
/// game makes no snapshot.
///
/// The repository is reindexed once first; only that failure is returned as `Err`. A game's
/// failure is in its [`GameBackup`] and does not stop the others. `progress` gets each outcome
/// as it is done.
pub fn backup_games(
    storage: &mut Storage,
    games: &[GameScan],
    stage_root: &Path,
    extra_tags: &[&str],
    mut progress: impl FnMut(&GameBackup),
) -> Result<Vec<GameBackup>, BackupError> {
    storage.reindex()?;
    let mut outcomes = Vec::with_capacity(games.len());
    for game in games {
        let slug = slug(&game.title);
        let result = backup_game(storage, game, &stage_root.join(&slug), &slug, extra_tags);
        let outcome = GameBackup {
            title: game.title.clone(),
            slug,
            result,
        };
        progress(&outcome);
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

fn backup_game(
    storage: &mut Storage,
    game: &GameScan,
    stage_dir: &Path,
    slug: &str,
    extra_tags: &[&str],
) -> Result<Option<SnapshotInfo>, BackupError> {
    stage(game, &read_registry(game)?, stage_dir)?;
    let game_tag = format!("game:{slug}");
    let mut tags = vec!["salvajocs", game_tag.as_str()];
    tags.extend_from_slice(extra_tags);
    let as_path = format!("/salvajocs/{slug}");
    Ok(storage.backup_dir(stage_dir, &as_path, &tags, &game.title)?)
}

/// Exports `game.registry_keys` from the source the scan recorded. A source that cannot be read
/// now is an error, never an empty export, which would drop `registry.json` from the snapshot.
fn read_registry(game: &GameScan) -> Result<RegistryExport, BackupError> {
    match &game.registry_source {
        None => Ok(RegistryExport::default()),
        Some(RegistrySource::WinePrefix(prefix)) => {
            let path = prefix.join("user.reg");
            let text = fs::read_to_string(&path).map_err(io_error(&path))?;
            RegFile::parse(&text)
                .and_then(|file| file.export(&game.registry_keys))
                .map_err(|source| BackupError::WineRegistry { path, source })
        }
        #[cfg(windows)]
        Some(RegistrySource::Windows) => Ok(crate::registry::windows::export(&game.registry_keys)?),
        #[cfg(not(windows))]
        Some(RegistrySource::Windows) => Err(BackupError::NoWindowsRegistry),
    }
}

#[cfg(test)]
mod tests;
