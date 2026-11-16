//! Stages each game into a mirror directory and backs it up as one snapshot (ADR-0004,
//! ADR-0010).

use std::collections::HashSet;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::layout::{
    FILES_DIR, META_FILE, REGISTRY_FILE, SNAPSHOT_FORMAT, SnapshotMeta, target_path,
};
use crate::registry::RegistryExport;
use crate::scan::GameScan;

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
/// When `game.unreadable` is not empty nothing is removed, `registry.json` included: a read
/// error must not drop saves from the next snapshot. The found files are only read.
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

    let meta = SnapshotMeta {
        format: SNAPSHOT_FORMAT,
        title: game.title.clone(),
        roots: game.roots.clone(),
    };
    write_if_changed(&stage_dir.join(META_FILE), &json(&meta))?;
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

#[cfg(test)]
mod tests;
