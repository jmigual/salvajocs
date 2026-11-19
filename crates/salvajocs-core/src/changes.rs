//! Which of a game's files differ from a snapshot of it, by content (ADR-0018): for the backup
//! preview, the skip rule of a backup and the restore's confirm tree. A snapshot of the other
//! build is compared through the folder pairs a restore would use (ADR-0021).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::backup::sha256_file;
use crate::config::AppConfig;
use crate::crossbuild::{self, Choice, Confidence, Family, MappingEntry, SnapshotRoot};
use crate::layout::{
    Context, FILES_DIR, HASHES_FILE, Hashes, META_FILE, REGISTRY_FILE, WinFolders,
};
use crate::manifest::Manifest;
use crate::prefix::wine_user;
use crate::restore::{self, Listed, PRE_RESTORE_TAG, PrepareError, SkippedItem, Targets};
use crate::scan::{Environment, GameScan};
use crate::selection::{self, GameSettings};
use crate::slug::slug;
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

/// Why [`Baseline::skip_blocker`] says a backup equal to the newest one is made all the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum NoSkip {
    /// The newest backup has no hash list that [`verified`] accepts.
    NoHashList,
    /// A file of the newest backup, from the other build, has no place on this machine.
    Unmapped,
    /// The newest backup is from the other build and one side has registry keys, which the
    /// preview does not compare.
    Registry,
}

/// A newest backup of the other build as mapped onto the scan's build (ADR-0021).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Mapped {
    /// The snapshot's regular files below `files/`, each mapped file at its target's path key.
    pub listing: Vec<SnapshotFile>,
    /// The snapshot's hash list with its files keyed as in `listing`.
    pub hashes: Option<Hashes>,
    /// The roots of the other build whose files have no place here, as a restore would skip
    /// them.
    pub unplaced_roots: Vec<SnapshotRoot>,
    /// The path keys of the other build's files left at their own key: no place here, or a key
    /// another file already takes.
    pub unplaced: BTreeSet<String>,
    /// Every root a restore would map, the known ones prefilled, when one of them is unknown
    /// (`PrepareError::NeedsMapping`), for the mapping dialog; empty otherwise.
    pub entries: Vec<MappingEntry>,
}

impl Mapped {
    /// A mapped backup from its fields.
    pub fn new(
        listing: Vec<SnapshotFile>,
        hashes: Option<Hashes>,
        unplaced_roots: Vec<SnapshotRoot>,
        unplaced: BTreeSet<String>,
        entries: Vec<MappingEntry>,
    ) -> Self {
        Self {
            listing,
            hashes,
            unplaced_roots,
            unplaced,
            entries,
        }
    }
}

/// What a game's files are compared with: its newest backup, by [`Mapper::baseline`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Baseline {
    /// The newest backup.
    pub snapshot: SnapshotInfo,
    /// The listing of its staged directory (ADR-0004), at the snapshot's own paths.
    pub listing: Vec<SnapshotFile>,
    /// Its hash list, when it has one that [`verified`] accepts.
    pub hashes: Option<Hashes>,
    /// The backup mapped onto the scan's build; `None` when it is of the same build.
    pub mapped: Option<Mapped>,
}

impl Baseline {
    /// A baseline from its fields.
    pub fn new(
        snapshot: SnapshotInfo,
        listing: Vec<SnapshotFile>,
        hashes: Option<Hashes>,
        mapped: Option<Mapped>,
    ) -> Self {
        Self {
            snapshot,
            listing,
            hashes,
            mapped,
        }
    }

    /// The listing and hash list to pass [`compare_game`]: the mapped ones when mapped.
    pub fn compared(&self) -> (&[SnapshotFile], Option<&Hashes>) {
        match &self.mapped {
            Some(mapped) => (&mapped.listing, mapped.hashes.as_ref()),
            None => (&self.listing, self.hashes.as_ref()),
        }
    }

    /// Whether the staged hash list `staged` holds the same content as this backup, so no
    /// backup is made (ADR-0018). Mapped, every file must be placed, the registry hashes equal
    /// and the files equal by mapped key, size and hash; `salvajocs.json` is not compared, as
    /// its roots differ across builds (ADR-0021).
    pub fn same_content(&self, staged: &Hashes) -> bool {
        let Some(mapped) = &self.mapped else {
            return (self.hashes.as_ref()).is_some_and(|hashes| staged.same_content(hashes));
        };
        let Some(hashes) = &mapped.hashes else {
            return false;
        };
        if !mapped.unplaced.is_empty()
            || staged.registry != hashes.registry
            || staged.files.len() != hashes.files.len()
        {
            return false;
        }
        // As `compare_game` matches: exactly, else the first key equal once case is folded.
        let mut folded: HashMap<String, &str> = HashMap::new();
        for key in hashes.files.keys().rev() {
            if let Some(fold) = fold(key) {
                folded.insert(fold, key);
            }
        }
        let mut used = HashSet::new();
        staged.files.iter().all(|(key, file)| {
            let at = match hashes.files.get_key_value(key) {
                Some((key, _)) => key.as_str(),
                None => match fold(key).and_then(|key| folded.get(&key)) {
                    Some(key) => key,
                    None => return false,
                },
            };
            let theirs = &hashes.files[at];
            used.insert(at) && theirs.size == file.size && theirs.sha256 == file.sha256
        })
    }

    /// Why a backup of `game`, filtered as the backup stores it ([`selection::apply`]), is made
    /// even when its files equal this backup's; `None` when it is skipped. Mapped, the registry
    /// is not previewed, so a skip is promised only when neither side has keys.
    pub fn skip_blocker(&self, game: &GameScan) -> Option<NoSkip> {
        let Some(hashes) = self.compared().1 else {
            return Some(NoSkip::NoHashList);
        };
        let mapped = self.mapped.as_ref()?;
        if !mapped.unplaced.is_empty() {
            Some(NoSkip::Unmapped)
        } else if game.registry_source.is_some() || hashes.registry.is_some() {
            Some(NoSkip::Registry)
        } else {
            None
        }
    }
}

/// Maps a game's newest backup of the other build onto the scan's build as a restore would
/// (ADR-0011, ADR-0021), from the `manifest`, the custom games of `config` and the machine `env`.
#[derive(Clone, Copy)]
pub struct Mapper<'a> {
    manifest: &'a Manifest,
    config: &'a AppConfig,
    env: &'a Environment,
}

impl<'a> Mapper<'a> {
    /// A mapper over `manifest`, `config` and `env`.
    pub fn new(manifest: &'a Manifest, config: &'a AppConfig, env: &'a Environment) -> Self {
        Self {
            manifest,
            config,
            env,
        }
    }

    /// The snapshot `newest` of `game` as `game`'s files are compared with it. When the scan's
    /// roots are of one build and the snapshot holds files of the other, its metadata is read
    /// ([`restore::read`], nothing downloaded) and mapped; roots a restore would ask about stay
    /// unplaced and nothing is saved. A snapshot that cannot be mapped is compared by path.
    ///
    /// # Errors
    /// The repository error when the snapshot cannot be listed or its hash list read.
    pub fn baseline(
        &self,
        storage: &mut Storage,
        newest: &SnapshotInfo,
        game: &GameScan,
    ) -> Result<Baseline, StorageError> {
        let dir = format!("/salvajocs/{}", slug(&game.title));
        let listing = storage.snapshot_files(&newest.id, &dir)?;
        let listed = self.targets(game, &listing).and_then(|(family, targets)| {
            let listed = restore::read(storage, newest, Path::new("baseline")).ok()?;
            Some((family, targets, listed))
        });
        let (hashes, mapped) = match listed {
            Some((family, targets, listed)) => {
                let mapped = self.map(&listed, game, family, &targets);
                (listed.hashes, mapped)
            }
            None => (hash_list(storage, &newest.id, &dir, &listing)?, None),
        };
        Ok(Baseline {
            snapshot: newest.clone(),
            listing,
            hashes,
            mapped,
        })
    }

    /// The scan's build and where its files are, when `listing` (a snapshot's staged directory)
    /// holds files of the other build; `None` otherwise, for a scan of both builds or of none,
    /// or when this machine has no folders for the scan's build.
    fn targets(&self, game: &GameScan, listing: &[SnapshotFile]) -> Option<(Family, Targets)> {
        let mut families =
            (game.roots.iter()).filter_map(|(base, _)| crossbuild::base_family(*base));
        let family = families.next()?;
        if !families.all(|other| other == family) {
            return None;
        }
        let prefix = format!("{FILES_DIR}/");
        let other_build = (listing.iter())
            .filter_map(|file| key_family(file.path.strip_prefix(&prefix)?))
            .any(|other| other != family);
        if !other_build {
            return None;
        }
        // From the scan's build, never `default_targets`, which on Linux may hold both builds.
        let targets = match (family, &self.env.native) {
            (Family::Linux, Some(Context::Linux(folders))) => Targets {
                linux: Some(folders.clone()),
                win: None,
            },
            (Family::Linux, _) => return None,
            (Family::Windows, Some(native @ Context::WindowsNative(_))) => Targets {
                linux: None,
                win: Some(native.clone()),
            },
            (Family::Windows, _) => {
                let prefix = game.prefixes.first()?;
                let user = wine_user(prefix).ok()?;
                let folders = WinFolders::for_prefix(prefix, &user);
                Targets {
                    linux: None,
                    win: Some(Context::WinePrefix {
                        prefix: prefix.clone(),
                        folders,
                    }),
                }
            }
        };
        Some((family, targets))
    }

    /// `listed` mapped onto `targets` of the build `family` by [`restore::prepare`], with every
    /// unknown root skipped; `None` when it cannot be planned.
    fn map(
        &self,
        listed: &Listed,
        game: &GameScan,
        family: Family,
        targets: &Targets,
    ) -> Option<Mapped> {
        let info = restore::game_info(&game.title, self.manifest, &self.config.custom_games);
        let vars = restore::game_vars(&game.title, info.entry.as_ref(), self.config, self.env);
        let prepare =
            |choices: &[_]| restore::prepare(&listed.staged, targets, &vars, &info, choices);
        let mut entries = Vec::new();
        let plan = match prepare(&[]) {
            Err(PrepareError::NeedsMapping(needed)) => {
                let skips: Vec<_> = (needed.iter())
                    .filter(|entry| entry.confidence == Confidence::Unknown)
                    .map(|entry| (entry.root.clone(), Choice::Skip))
                    .collect();
                entries = needed;
                prepare(&skips)
            }
            plan => plan,
        }
        .ok()?;
        let other = family.other();
        let prefix = format!("{FILES_DIR}/");
        let mut moved: HashMap<&str, String> = HashMap::new();
        for write in &plan.writes {
            let (Some(entry), Some((base, tail))) = (listed.source(write), &write.target_file)
            else {
                continue;
            };
            if let Some(key) = entry.path.strip_prefix(&prefix)
                && key_family(key) == Some(other)
            {
                moved.insert(key, format!("{}/{tail}", base.dir_name()));
            }
        }
        let files: Vec<(&str, &SnapshotFile)> = (listed.listing.iter())
            .filter(|entry| entry.kind == EntryKind::File)
            .filter_map(|entry| Some((entry.path.strip_prefix(&prefix)?, entry)))
            .collect();
        // Files left at their key reserve it first, so a mapped file never displaces one.
        let mut taken: HashSet<String> = HashSet::new();
        let mut reserve = |key: &str| {
            let folded = fold(key);
            let free = !taken.contains(key) && folded.as_ref().is_none_or(|f| !taken.contains(f));
            taken.insert(key.to_owned());
            taken.extend(folded);
            free
        };
        for (key, _) in &files {
            if !moved.contains_key(key) {
                reserve(key);
            }
        }
        let mut unplaced = BTreeSet::new();
        let mut keys = Vec::with_capacity(files.len());
        for &(key, _) in &files {
            let placed = match moved.get(key) {
                Some(target) if reserve(target) => Some(target.clone()),
                Some(_) => None,
                None if key_family(key) == Some(other) => None,
                None => Some(key.to_owned()),
            };
            if placed.is_none() {
                unplaced.insert(key.to_owned());
            }
            keys.push(placed.unwrap_or_else(|| key.to_owned()));
        }
        let listing = (files.iter().zip(&keys))
            .map(|((_, entry), key)| SnapshotFile {
                path: format!("{prefix}{key}"),
                ..(*entry).clone()
            })
            .collect();
        let hashes = listed.hashes.as_ref().map(|hashes| Hashes {
            format: hashes.format,
            meta: hashes.meta.clone(),
            registry: hashes.registry.clone(),
            files: (files.iter().zip(&keys))
                .filter_map(|((old, _), key)| Some((key.clone(), hashes.files.get(*old)?.clone())))
                .collect(),
        });
        let mut unplaced_roots: Vec<SnapshotRoot> = Vec::new();
        for skipped in &plan.skipped {
            if let SkippedItem::File(base, tail) = &skipped.item
                && crossbuild::base_family(*base) == Some(other)
                && let Some(root) = restore::root_of(&listed.staged.meta.roots, *base, tail, &vars)
                && !unplaced_roots.contains(root)
            {
                unplaced_roots.push(root.clone());
            }
        }
        Some(Mapped {
            listing,
            hashes,
            unplaced_roots,
            unplaced,
            entries,
        })
    }
}

/// The base family of the path key `key` (`<Base>/<tail>`); `None` for an install-relative or
/// unknown base.
fn key_family(key: &str) -> Option<Family> {
    let (base, _) = key.split_once('/')?;
    crossbuild::base_family(restore::parse_base(base)?)
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

#[cfg(test)]
mod tests;
