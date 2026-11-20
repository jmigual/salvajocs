//! Stages each game into a mirror directory and backs it up as one snapshot (ADR-0004,
//! ADR-0010).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::changes;

use crate::layout::{
    FILES_DIR, FileHash, HASHES_FILE, HASHES_FORMAT, Hashes, META_FILE, REGISTRY_FILE,
    SNAPSHOT_FORMAT, SnapshotMeta, target_path,
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
#[non_exhaustive]
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
/// files no longer found, or deleted since the scan, are removed with the directories they
/// leave empty. `salvajocs.json`, which records `game.prefix_origin` (ADR-0023),
/// and `registry.json` are written only when their bytes change; an empty `registry` means no
/// `registry.json`.
///
/// When `game.unreadable` is not empty nothing is removed, `registry.json` included, and the
/// roots already in `salvajocs.json` are kept after the scan's, as is its prefix when the scan
/// found none: a read error must not drop saves from the next snapshot or from a restore. The
/// found files are only read.
///
/// Last, `hashes.json` lists the SHA-256 of every other staged file (ADR-0018), also written
/// only when its bytes change; the list is returned. A copied file is hashed as it is copied;
/// another keeps its entry of the previous list while its size and mtime match, and is read
/// otherwise.
pub fn stage(
    game: &GameScan,
    registry: &RegistryExport,
    stage_dir: &Path,
) -> Result<Hashes, BackupError> {
    fs::create_dir_all(stage_dir).map_err(io_error(stage_dir))?;
    let hashes_file = stage_dir.join(HASHES_FILE);
    let previous = staged_hashes(&hashes_file)?;
    let files_dir = stage_dir.join(FILES_DIR);
    let mut staged = Vec::with_capacity(game.files.len());
    for file in &game.files {
        let dest = target_path(&files_dir.join(file.base.dir_name()), &file.tail, false)
            .ok_or_else(|| BackupError::UnsafeTail(file.tail.clone()))?;
        staged.push((file.real.as_path(), dest));
    }
    let keep_all = !game.unreadable.is_empty();
    let mut keep: HashSet<&Path> = staged.iter().map(|(_, dest)| dest.as_path()).collect();
    if !keep_all {
        // Removing first lets a file replace a directory of the same name, or a case change.
        remove_stale(&files_dir, &keep)?;
    }
    let mut vanished = false;
    let mut copied = HashMap::new();
    for (real, dest) in &staged {
        match sync_file(real, dest)? {
            Synced::Vanished => {
                vanished = true;
                keep.remove(dest.as_path());
            }
            Synced::Kept => {}
            Synced::Copied(sha256) => {
                copied.insert(dest.clone(), sha256);
            }
        }
    }
    if vanished && !keep_all {
        remove_stale(&files_dir, &keep)?;
    }

    let meta_file = stage_dir.join(META_FILE);
    let mut roots = game.roots.clone();
    let mut prefix = game.prefix_origin.clone();
    if keep_all && let Some(staged) = staged_meta(&meta_file)? {
        for root in staged.roots {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        prefix = prefix.or(staged.prefix);
    }
    let meta = SnapshotMeta {
        format: SNAPSHOT_FORMAT,
        title: game.title.clone(),
        roots,
        prefix,
    };
    write_if_changed(&meta_file, &json(&meta))?;
    let registry_file = stage_dir.join(REGISTRY_FILE);
    if !registry.keys.is_empty() {
        write_if_changed(&registry_file, &json(registry))?;
    } else if !keep_all {
        remove_if_exists(&registry_file)?;
    }

    let mut files = BTreeMap::new();
    hash_staged(&files_dir, "", &copied, previous.as_ref(), &mut files)?;
    let registry = match sha256_file(&registry_file) {
        Ok(sha256) => Some(sha256),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(io_error(&registry_file)(err)),
    };
    let hashes = Hashes {
        format: HASHES_FORMAT,
        meta: sha256_file(&meta_file).map_err(io_error(&meta_file))?,
        registry,
        files,
    };
    write_if_changed(&hashes_file, &json(&hashes))?;
    Ok(hashes)
}

/// The staged `hashes.json`; `None` when there is none or it cannot be parsed, so every file
/// not copied this run is read again.
fn staged_hashes(hashes_file: &Path) -> Result<Option<Hashes>, BackupError> {
    match fs::read(hashes_file) {
        Ok(bytes) => Ok(Hashes::from_json(&bytes)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io_error(hashes_file)(err)),
    }
}

/// Adds every regular file below `dir`, whose path below `files/` is `prefix`, to `out`: with
/// its hash from `copied`, else from `previous` when its size and mtime match, else read.
fn hash_staged(
    dir: &Path,
    prefix: &str,
    copied: &HashMap<PathBuf, String>,
    previous: Option<&Hashes>,
    out: &mut BTreeMap<String, FileHash>,
) -> Result<(), BackupError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(io_error(dir)(err)),
    };
    for entry in entries {
        let entry = entry.map_err(io_error(dir))?;
        let path = entry.path();
        let key = format!("{prefix}{}", entry.file_name().to_string_lossy());
        let kind = entry.file_type().map_err(io_error(&path))?;
        if kind.is_dir() {
            hash_staged(&path, &format!("{key}/"), copied, previous, out)?;
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        // Not the listing's metadata, which can lag a file just copied on Windows (ADR-0003).
        let meta = fs::symlink_metadata(&path).map_err(io_error(&path))?;
        let size = meta.len();
        let modified = (meta.modified().ok())
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok());
        let known = (previous.and_then(|previous| previous.files.get(&key)))
            .filter(|entry| entry.size == size && modified.is_some() && entry.modified == modified)
            .map(|entry| entry.sha256.clone());
        let sha256 = match copied.get(&path).cloned().or(known) {
            Some(sha256) => sha256,
            None => sha256_file(&path).map_err(io_error(&path))?,
        };
        out.insert(
            key,
            FileHash {
                sha256,
                size,
                modified,
            },
        );
    }
    Ok(())
}

/// The SHA-256 of the file at `path`, in hex.
pub(crate) fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 64 * 1024];
    loop {
        match file.read(&mut buf) {
            Ok(0) => return Ok(hex(hasher)),
            Ok(read) => hasher.update(&buf[..read]),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
}

/// The digest of `hasher`, in lowercase hex.
fn hex(hasher: Sha256) -> String {
    (hasher.finalize().iter())
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn remove_if_exists(path: &Path) -> Result<(), BackupError> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(io_error(path)(err)),
        _ => Ok(()),
    }
}

/// The staged `salvajocs.json`, `None` when there is none yet.
fn staged_meta(meta_file: &Path) -> Result<Option<SnapshotMeta>, BackupError> {
    let bytes = match fs::read(meta_file) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(io_error(meta_file)(err)),
    };
    let meta = serde_json::from_slice(&bytes).map_err(|source| BackupError::Meta {
        path: meta_file.to_path_buf(),
        source,
    })?;
    Ok(Some(meta))
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

/// What [`sync_file`] did.
enum Synced {
    /// The source no longer exists; nothing was copied.
    Vanished,
    /// The staged copy already had the source's size and mtime.
    Kept,
    /// The source was copied; holds the SHA-256 of what was copied, in hex.
    Copied(String),
}

/// Copies `real` to `dest` unless both have the same size and mtime, and gives the copy the
/// source's mtime. Size and mtime are read before copying, so a save written during the copy
/// differs again on the next run and is copied once more.
fn sync_file(real: &Path, dest: &Path) -> Result<Synced, BackupError> {
    let mut source = match File::open(real) {
        Ok(source) => source,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Synced::Vanished),
        Err(err) => return Err(io_error(real)(err)),
    };
    let meta = source.metadata().map_err(io_error(real))?;
    let modified = meta.modified().map_err(io_error(real))?;
    match fs::symlink_metadata(dest) {
        Ok(staged) if staged.len() == meta.len() && staged.modified().ok() == Some(modified) => {
            return Ok(Synced::Kept);
        }
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(io_error(dest)(err)),
        _ => {}
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(io_error(parent))?;
    }
    // Copied beside `dest` and renamed over it: a failed copy leaves no partial file, and a
    // link at `dest` is replaced rather than written through.
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".salvajocs-tmp");
    let tmp = PathBuf::from(tmp);
    remove_if_exists(&tmp)?;
    let copied = copy_to(&mut source, real, &tmp, dest, modified);
    if copied.is_err() {
        // Best effort: the copy's own error is the one to report.
        let _ = fs::remove_file(&tmp);
    }
    let sha256 = copied?;
    fs::rename(&tmp, dest).map_err(io_error(dest))?;
    Ok(Synced::Copied(sha256))
}

/// Writes `source` (opened from `real`) into the new file `tmp` and gives it mtime `modified`.
/// Returns the SHA-256 of what it wrote, in hex. A read error names `real`, a write error
/// `dest`.
fn copy_to(
    source: &mut File,
    real: &Path,
    tmp: &Path,
    dest: &Path,
    modified: SystemTime,
) -> Result<String, BackupError> {
    // A fresh writable file rather than `fs::copy`, which would carry a read-only permission
    // over; `create_new` does not follow a link at `tmp`.
    let mut copy = File::options()
        .write(true)
        .create_new(true)
        .open(tmp)
        .map_err(io_error(dest))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 64 * 1024];
    loop {
        let read = match source.read(&mut buf) {
            Ok(0) => break,
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(io_error(real)(err)),
        };
        copy.write_all(&buf[..read]).map_err(io_error(dest))?;
        hasher.update(&buf[..read]);
    }
    // The handle closes on return, before the rename: Windows cannot rename an open file.
    copy.set_modified(modified).map_err(io_error(dest))?;
    Ok(hex(hasher))
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

/// What [`backup_games`] stored for one game.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Stored {
    /// A snapshot was written.
    New(SnapshotInfo),
    /// Nothing changed since this machine's last backup of the game.
    Unchanged {
        /// The newest snapshot of the game, when another host made it.
        newer_elsewhere: Option<SnapshotInfo>,
    },
    /// The staged content equals this snapshot, the game's newest, which another host made.
    SameAsNewest(SnapshotInfo),
}

/// The outcome of one game in [`backup_games`].
#[derive(Debug)]
#[non_exhaustive]
pub struct GameBackup {
    /// The game's title.
    pub title: String,
    /// The game's slug (ADR-0010).
    pub slug: String,
    /// What was stored, or why it failed.
    pub result: Result<Stored, BackupError>,
}

impl GameBackup {
    /// The `result` of backing up game `title` with `slug`.
    pub fn new(title: String, slug: String, result: Result<Stored, BackupError>) -> Self {
        Self {
            title,
            slug,
            result,
        }
    }
}

/// What [`backup_game`] compares a staged game with.
struct Baseline {
    /// The game's newest snapshot that is not a pre-restore one, from any host.
    newest: Option<SnapshotInfo>,
    /// It as the staged game is compared with it; `None` when it cannot be read or the
    /// same-as-newest rule is off.
    compared: Option<changes::Baseline>,
}

/// Backs up each of `games` as its own snapshot (ADR-0010), one after another: the game is
/// staged in `stage_root/<slug>` with its registry keys read fresh from the scan's
/// `registry_source`, then stored as `/salvajocs/<slug>` with label = title and tags
/// `salvajocs`, `game:<slug>` and `extra_tags` (which must not contain commas).
///
/// A game makes no snapshot when nothing changed since this machine's last backup of it, or,
/// with a `same_as_newest` mapper, when its staged content equals that of the game's newest
/// snapshot that is not a pre-restore one, from any host (ADR-0018), mapped through the folder
/// pairs when that snapshot is of the other build ([`changes::Baseline::same_content`],
/// ADR-0021). The pre-restore safety backup passes `None`.
///
/// The repository is reindexed and its snapshots listed once first; only those failures are
/// returned as `Err`. A game's failure is in its [`GameBackup`] and does not stop the others.
/// `progress` gets each outcome as it is done.
pub fn backup_games(
    storage: &mut Storage,
    games: &[GameScan],
    stage_root: &Path,
    extra_tags: &[&str],
    same_as_newest: Option<&changes::Mapper<'_>>,
    mut progress: impl FnMut(&GameBackup),
) -> Result<Vec<GameBackup>, BackupError> {
    storage.reindex()?;
    let snapshots = storage.snapshots("salvajocs")?;
    let host = storage.hostname()?;
    // Read before anything is written: a read after a write would reindex first (ADR-0003).
    let baselines: Vec<Baseline> = (games.iter())
        .map(|game| {
            let slug = slug(&game.title);
            let newest = changes::newest(&snapshots, &slug).cloned();
            // A list that cannot be read or verified only costs a snapshot rustic may still
            // skip; failing the game would block its backups for good.
            let compared = match (&newest, same_as_newest) {
                (Some(newest), Some(mapper)) => mapper.baseline(storage, newest, game).ok(),
                _ => None,
            };
            Baseline { newest, compared }
        })
        .collect();
    let mut outcomes = Vec::with_capacity(games.len());
    for (game, baseline) in games.iter().zip(baselines) {
        let slug = slug(&game.title);
        let stage_dir = stage_root.join(&slug);
        let result = backup_game(
            storage, game, &stage_dir, &slug, extra_tags, baseline, &host,
        );
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

/// Stages `game` and stores it unless it equals `baseline`'s content or rustic finds it
/// unchanged; `host` is this machine's.
fn backup_game(
    storage: &mut Storage,
    game: &GameScan,
    stage_dir: &Path,
    slug: &str,
    extra_tags: &[&str],
    baseline: Baseline,
    host: &str,
) -> Result<Stored, BackupError> {
    let staged = stage(game, &read_registry(game)?, stage_dir)?;
    let Baseline { newest, compared } = baseline;
    if let (Some(newest), Some(compared)) = (&newest, compared)
        && compared.same_content(&staged)
    {
        return Ok(if newest.host == host {
            Stored::Unchanged {
                newer_elsewhere: None,
            }
        } else {
            Stored::SameAsNewest(newest.clone())
        });
    }
    let game_tag = format!("game:{slug}");
    let mut tags = vec!["salvajocs", game_tag.as_str()];
    tags.extend_from_slice(extra_tags);
    let as_path = format!("/salvajocs/{slug}");
    Ok(
        match storage.backup_dir(stage_dir, &as_path, &tags, &game.title)? {
            Some(snapshot) => Stored::New(snapshot),
            None => Stored::Unchanged {
                newer_elsewhere: newest.filter(|newest| newest.host != host),
            },
        },
    )
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
