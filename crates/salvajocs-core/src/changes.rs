//! Which of a game's files differ from a snapshot of it, by content (ADR-0018): for the backup
//! preview, the skip rule of a backup and the restore's confirm tree.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::backup::sha256_file;
use crate::layout::{FILES_DIR, HASHES_FILE, Hashes, META_FILE, REGISTRY_FILE};
use crate::restore::PRE_RESTORE_TAG;
use crate::scan::GameScan;
use crate::selection::{self, GameSettings};
use crate::storage::{EntryKind, SnapshotFile, SnapshotInfo, Storage, StorageError};

/// How a local file compares with a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileState {
    /// The snapshot has no file at its path.
    New,
    /// The snapshot's file at its path has other content.
    Changed,
    /// The snapshot's file at its path has the same content.
    Same,
    /// The local file could not be read; holds the error text.
    Unknown(String),
}

/// What [`compare_game`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Comparison {
    /// The state of each of the scan's files, in its order.
    pub found: Vec<FileState>,
    /// The snapshot's files below `files/` that no found file matches, with `files/` stripped
    /// from their paths.
    pub gone: Vec<SnapshotFile>,
}

impl Comparison {
    /// A comparison from its fields.
    pub fn new(found: Vec<FileState>, gone: Vec<SnapshotFile>) -> Self {
        Self { found, gone }
    }
}

/// How many files a backup would add, change and remove, by [`counts`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Counts {
    /// Included files the snapshot does not have.
    pub new: usize,
    /// Included files whose content differs from the snapshot's.
    pub changed: usize,
    /// Files of the snapshot the backup would not store.
    pub removed: usize,
    /// Included files that could not be read to compare them.
    pub unknown: usize,
}

/// The newest of `snapshots` (newest first, as [`crate::storage::Storage::snapshots`] lists
/// them) of the game `slug`, from any host, that is not a pre-restore safety backup.
pub fn newest<'a>(snapshots: &'a [SnapshotInfo], slug: &str) -> Option<&'a SnapshotInfo> {
    let game_tag = format!("game:{slug}");
    snapshots.iter().find(|snapshot| {
        snapshot.tags.contains(&game_tag) && !snapshot.tags.iter().any(|t| t == PRE_RESTORE_TAG)
    })
}

/// The listing of the game `slug` in snapshot `snapshot_id` (its staged directory, ADR-0004)
/// and the snapshot's hash list, when it has one that [`verified`] accepts.
pub fn baseline(
    storage: &mut Storage,
    snapshot_id: &str,
    slug: &str,
) -> Result<(Vec<SnapshotFile>, Option<Hashes>), StorageError> {
    let dir = format!("/salvajocs/{slug}");
    let listing = storage.snapshot_files(snapshot_id, &dir)?;
    let hashes = hash_list(storage, snapshot_id, &dir, &listing)?;
    Ok((listing, hashes))
}

/// The hash list of the game's staged directory `dir` in snapshot `snapshot_id`, whose entries
/// are `listing`, when it has one that [`verified`] accepts.
pub(crate) fn hash_list(
    storage: &mut Storage,
    snapshot_id: &str,
    dir: &str,
    listing: &[SnapshotFile],
) -> Result<Option<Hashes>, StorageError> {
    let listed =
        (listing.iter()).any(|file| file.path == HASHES_FILE && file.kind == EntryKind::File);
    let hashes = if listed {
        let bytes = storage.read_file_if_present(snapshot_id, &format!("{dir}/{HASHES_FILE}"))?;
        bytes.and_then(|bytes| Hashes::from_json(&bytes))
    } else {
        None
    };
    Ok(hashes.and_then(|hashes| verified(hashes, listing)))
}

/// `hashes` when it describes the snapshot listing `listing` it was read from: the same files
/// below `files/` with the same sizes and mtimes, a `registry.json` exactly when it hashes one,
/// and `hashes.json` no older than `salvajocs.json` and `registry.json`. `None` otherwise, such
/// as for a list an older version left in the stage while it changed the files. Only the
/// regular files of `listing` count.
pub fn verified(hashes: Hashes, listing: &[SnapshotFile]) -> Option<Hashes> {
    let listing = || listing.iter().filter(|file| file.kind == EntryKind::File);
    let modified = |path: &str| {
        listing()
            .find(|file| file.path == path)
            .map(|file| file.modified)
    };
    let written = modified(HASHES_FILE)??;
    if modified(META_FILE)?? > written {
        return None;
    }
    match (modified(REGISTRY_FILE), &hashes.registry) {
        (Some(registry), Some(_)) if registry? <= written => {}
        (None, None) => {}
        _ => return None,
    }
    let prefix = format!("{FILES_DIR}/");
    let mut files = 0;
    let all_match = listing().all(|file| {
        let Some(key) = file.path.strip_prefix(&prefix) else {
            return true;
        };
        files += 1;
        hashes.files.get(key).is_some_and(|entry| {
            let entry_modified = entry.modified.map(|since| UNIX_EPOCH + since);
            entry.size == file.size && entry_modified.is_some() && entry_modified == file.modified
        })
    });
    (all_match && files == hashes.files.len()).then_some(hashes)
}

/// Compares the file `local`, last modified at `local_modified`, with the snapshot's `entry` at
/// the same path, whose SHA-256 the snapshot's hash list gives as `sha256`: a different size is
/// [`FileState::Changed`] and the same size and mtime is [`FileState::Same`], both without
/// reading the file. Otherwise the file's SHA-256 is compared with `sha256`; without one (a
/// snapshot made before hash lists) the other mtime alone makes it [`FileState::Changed`].
pub fn compare_file(
    local: &Path,
    local_modified: Option<SystemTime>,
    entry: &SnapshotFile,
    sha256: Option<&str>,
) -> FileState {
    let compare = || -> io::Result<bool> {
        if fs::metadata(local)?.len() != entry.size {
            return Ok(false);
        }
        if local_modified.is_some() && local_modified == entry.modified {
            return Ok(true);
        }
        match sha256 {
            Some(sha256) => Ok(sha256_file(local)? == sha256),
            None => Ok(false),
        }
    };
    match compare() {
        Ok(true) => FileState::Same,
        Ok(false) => FileState::Changed,
        Err(err) => FileState::Unknown(err.to_string()),
    }
}

/// Compares each of `scan`'s files with the snapshot listing `baseline` of the game's staged
/// directory (ADR-0004) and its hash list `hashes`, `None` when it has none, by
/// [`compare_file`]. A file matches the entry at `files/<path key>`; below a Windows-family base
/// an entry differing only in ASCII case matches when no entry matches exactly, as the scan and
/// the selection do (ADR-0014). Reads the files it hashes.
pub fn compare_game(
    scan: &GameScan,
    baseline: &[SnapshotFile],
    hashes: Option<&Hashes>,
) -> Comparison {
    let prefix = format!("{FILES_DIR}/");
    let entries: Vec<(&str, &SnapshotFile)> = baseline
        .iter()
        .filter_map(|entry| Some((entry.path.strip_prefix(&prefix)?, entry)))
        .collect();
    let exact: HashMap<&str, usize> = (entries.iter().enumerate())
        .map(|(at, (key, _))| (*key, at))
        .collect();
    let mut folded: HashMap<String, usize> = HashMap::new();
    for (at, (key, _)) in entries.iter().enumerate().rev() {
        if let Some(key) = fold(key) {
            folded.insert(key, at);
        }
    }
    let mut matched = vec![false; entries.len()];
    let found = (scan.files.iter())
        .map(|file| {
            let key = selection::path_key(file);
            let at = exact
                .get(key.as_str())
                .or_else(|| fold(&key).and_then(|key| folded.get(&key)));
            match at {
                Some(&at) => {
                    matched[at] = true;
                    let (key, entry) = entries[at];
                    let sha256 = hashes
                        .and_then(|hashes| hashes.files.get(key))
                        .map(|file| file.sha256.as_str());
                    compare_file(&file.real, file.modified, entry, sha256)
                }
                None => FileState::New,
            }
        })
        .collect();
    let gone = (entries.into_iter().zip(matched))
        .filter(|(_, matched)| !matched)
        .map(|((key, entry), _)| SnapshotFile {
            path: key.to_owned(),
            ..entry.clone()
        })
        .collect();
    Comparison { found, gone }
}

/// `key` (a path key) with its tail ASCII-lowercased, when its base folds case
/// ([`selection::folds_case`]); `None` when it does not.
fn fold(key: &str) -> Option<String> {
    let (base, tail) = key.split_once('/')?;
    selection::folds_case(base).then(|| format!("{base}/{}", tail.to_ascii_lowercase()))
}

/// How many of `scan`'s files a backup by `settings` would add, change and remove compared with
/// the snapshot `comparison` was made against. An included file counts by its state; an
/// excluded one that the snapshot has, and every gone file, count as removed. A game with
/// unreadable paths removes nothing, since staging then keeps every file (ADR-0004). `None` when
/// the game is skipped or nothing of it is left ([`selection::apply`]), so no snapshot is made.
/// For a game whose settings cannot be read, pass [`GameSettings::default`].
pub fn counts(scan: &GameScan, comparison: &Comparison, settings: &GameSettings) -> Option<Counts> {
    selection::apply(scan, settings)?;
    let mut counts = Counts::default();
    for (file, state) in scan.files.iter().zip(&comparison.found) {
        let included = settings.path_included(&selection::path_key(file));
        match (included, state) {
            (true, FileState::New) => counts.new += 1,
            (true, FileState::Changed) => counts.changed += 1,
            (true, FileState::Unknown(_)) => counts.unknown += 1,
            (true, FileState::Same) | (false, FileState::New) => {}
            (false, _) => counts.removed += 1,
        }
    }
    counts.removed += comparison.gone.len();
    if !scan.unreadable.is_empty() {
        counts.removed = 0;
    }
    Some(counts)
}

/// Whether the files `a` and `b` hold the same bytes: sizes first, then their contents.
pub fn same_content(a: &Path, b: &Path) -> io::Result<bool> {
    let (mut a, mut b) = (File::open(a)?, File::open(b)?);
    if a.metadata()?.len() != b.metadata()?.len() {
        return Ok(false);
    }
    let (mut buf_a, mut buf_b) = (vec![0; 64 * 1024], vec![0; 64 * 1024]);
    loop {
        let read = fill(&mut a, &mut buf_a)?;
        if read != fill(&mut b, &mut buf_b)? || buf_a[..read] != buf_b[..read] {
            return Ok(false);
        }
        if read == 0 {
            return Ok(true);
        }
    }
}

/// Reads into `buf` until it is full or the reader ends; returns how much it read.
fn fill(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests;
