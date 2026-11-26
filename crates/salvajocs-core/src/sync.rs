//! Where each game stands on this PC against the history every PC shares (ADR-0035): the base
//! this PC last synced with, kept in `sync-base.json` in the config dir, and the [`state`] of
//! its saves against the game's newest backup and that base.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::backup;
use crate::changes::{self, Baseline, Comparison, Counts};
use crate::fsutil::write_atomic;
use crate::layout::{FILES_DIR, Hashes};
use crate::scan::GameScan;
use crate::selection::{self, GameSettings};
use crate::storage::{EntryKind, SnapshotFile, SnapshotInfo};

/// The base file name inside the config directory.
const BASE_FILE: &str = "sync-base.json";
/// The [`Bases`] format this version reads and writes.
const BASE_FORMAT: u32 = 1;

/// [`record`] could not update `sync-base.json`; the file is left as it was.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SyncError {
    /// The file exists but cannot be read, so rewriting it could drop other games' bases.
    #[error("cannot read {path}")]
    Read {
        /// The file.
        path: PathBuf,
        /// The I/O error.
        #[source]
        source: io::Error,
    },
    /// The file is of a newer format, written by a newer version.
    #[error("{path} is of format {format}, newer than this version's")]
    NewerFormat {
        /// The file.
        path: PathBuf,
        /// Its format.
        format: u32,
    },
    /// The file cannot be written.
    #[error("cannot write {path}")]
    Write {
        /// The file.
        path: PathBuf,
        /// The I/O error.
        #[source]
        source: io::Error,
    },
}

/// The backup this PC last synced a game with: the one it last backed up, last found equal to
/// the newest backup, or last restored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Base {
    /// The snapshot's full hex id.
    pub snapshot: String,
    /// The staged hash list (`hashes.json`) of this PC's saves right after the sync, as Back up
    /// stages them; `None` for a seed, or when after a restore the scan does not find the game,
    /// its settings skip it or its saves cannot be staged. With it, the base still compares once
    /// the snapshot is forgotten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saves: Option<Hashes>,
    /// Whether [`Bases::base`] made this base as a seed rather than reading a record.
    #[serde(skip)]
    seed: bool,
}

impl Base {
    /// The base `snapshot`, with this PC's staged `saves` after the sync when known.
    pub fn new(snapshot: String, saves: Option<Hashes>) -> Self {
        Self {
            snapshot,
            saves,
            seed: false,
        }
    }

    /// The id of the snapshot [`state`] needs read as its `base_backup` for `saves` against the
    /// newest backup `newest`: this base's own when it has no hash list, is not `newest`, and
    /// `saves` differ from `newest`; `None` when none is needed. Callers read the snapshot only
    /// when this names it, so an up-to-date game reads nothing more.
    pub fn to_read(&self, saves: &Saves<'_>, newest: &Baseline) -> Option<&str> {
        (self.saves.is_none() && self.snapshot != newest.snapshot.id && !saves.equal_newest(newest))
            .then_some(self.snapshot.as_str())
    }
}

/// This PC's bases, as [`load`] reads them from `sync-base.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bases {
    format: u32,
    /// Repository key ([`crate::config::AppConfig::secret_scope`]) to game slug to its base.
    #[serde(default)]
    repositories: BTreeMap<String, BTreeMap<String, Base>>,
}

impl Default for Bases {
    fn default() -> Self {
        Self {
            format: BASE_FORMAT,
            repositories: BTreeMap::new(),
        }
    }
}

impl Bases {
    /// The recorded base of the game `slug` in the repository `scope`; never a seed.
    pub fn get(&self, scope: &str, slug: &str) -> Option<&Base> {
        self.repositories.get(scope)?.get(slug)
    }

    /// The base of the game `slug` in the repository `scope`: its record, else a seed, which is
    /// this PC's (`host`'s) newest snapshot of the game in `snapshots` (newest first) that is not
    /// a pre-restore safety backup, without a hash list. A seed is never written. `None` when
    /// there is neither.
    pub fn base<'a>(
        &'a self,
        scope: &str,
        slug: &str,
        snapshots: &[SnapshotInfo],
        host: &str,
    ) -> Option<Cow<'a, Base>> {
        if let Some(base) = self.get(scope, slug) {
            return Some(Cow::Borrowed(base));
        }
        let game_tag = format!("game:{slug}");
        let seed = (snapshots.iter())
            .find(|snapshot| snapshot.host == host && changes::regular(snapshot, &game_tag))?;
        let mut base = Base::new(seed.id.clone(), None);
        base.seed = true;
        Some(Cow::Owned(base))
    }
}

/// Reads `dir/sync-base.json`. A file that is missing, cannot be read or parsed, or is of a
/// newer format reads as empty, so every game falls back to its seed or to an unknown base.
pub fn load(dir: &Path) -> Bases {
    (fs::read(dir.join(BASE_FILE)).ok())
        .and_then(|bytes| serde_json::from_slice::<Bases>(&bytes).ok())
        .filter(|bases| bases.format <= BASE_FORMAT)
        .unwrap_or_default()
}

/// Sets the base of each `(slug, base)` of `updates` in the repository `scope`, keeping every
/// other record, and writes `dir/sync-base.json` through a temp file and a rename, creating
/// `dir`. A missing file starts empty and one that does not parse is replaced. A seed from
/// [`Bases::base`] is skipped, as it is only ever used. Callers hold the session's storage lock,
/// so two jobs never read and rewrite the file at once.
///
/// # Errors
/// [`SyncError`] when the file exists but cannot be read, is of a newer format, or cannot be
/// written; nothing is written then.
pub fn record(
    dir: &Path,
    scope: &str,
    updates: impl IntoIterator<Item = (String, Base)>,
) -> Result<(), SyncError> {
    let path = dir.join(BASE_FILE);
    let mut bases = read_to_record(&path)?;
    (bases.repositories.entry(scope.to_owned()).or_default())
        .extend(updates.into_iter().filter(|(_, base)| !base.seed));
    write_bases(dir, &path, &bases)
}

/// Writes `bases` to `path` in `dir` through a temp file and a rename, creating `dir`.
fn write_bases(dir: &Path, path: &Path, bases: &Bases) -> Result<(), SyncError> {
    let json = serde_json::to_vec(bases).expect("string-keyed maps always serialize");
    fs::create_dir_all(dir)
        .and_then(|()| write_atomic(path, &json))
        .map_err(|source| SyncError::Write {
            path: path.to_owned(),
            source,
        })
}

/// The bases at `path` for [`record`] to update: empty when the file is missing or does not
/// parse; an error when it cannot be read or is of a newer format, which a rewrite would lose.
fn read_to_record(path: &Path) -> Result<Bases, SyncError> {
    #[derive(Deserialize)]
    struct Header {
        format: u32,
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Bases::default()),
        Err(source) => {
            let path = path.to_owned();
            return Err(SyncError::Read { path, source });
        }
    };
    if let Ok(Header { format }) = serde_json::from_slice(&bytes)
        && format > BASE_FORMAT
    {
        let path = path.to_owned();
        return Err(SyncError::NewerFormat { path, format });
    }
    Ok(serde_json::from_slice(&bytes).unwrap_or_default())
}

/// Where a game stands on this PC, by [`state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum State {
    /// The repository has no backup of the game: Back up stores one.
    NoBackupYet,
    /// This PC's saves equal the newest backup: Back up stores nothing, and the newest becomes
    /// the base.
    UpToDate,
    /// Only this PC changed the saves since its base, which is the newest backup: Back up stores
    /// them as the new newest.
    Ahead,
    /// Another PC made a newer backup and this PC's saves are still its base: Back up never
    /// stores them; the newer saves are restored instead.
    Behind,
    /// This PC and another changed the saves since the base: Back up asks.
    ChangedOnBoth,
    /// No base is known and this PC's saves differ from the newest backup: Back up asks, as for
    /// [`State::ChangedOnBoth`].
    UnknownBase,
}

/// This PC's saves (L), as the caller has them.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum Saves<'a> {
    /// At Back up: the staged hash list (`hashes.json`) of the game.
    Staged(&'a Hashes),
    /// In the preview: the scan. A game [`changes::counts`] gives `None` for is not backed up;
    /// the caller checks that first, since such saves equal nothing here.
    Scanned {
        /// The scan of the game.
        game: &'a GameScan,
        /// The game's backup selection.
        settings: &'a GameSettings,
        /// The scan compared with the newest backup ([`changes::compare_game`] on
        /// [`Baseline::compared`]); unused when there is none.
        compared: &'a Comparison,
    },
}

impl Saves<'_> {
    /// Whether these saves equal the newest backup `newest`.
    fn equal_newest(&self, newest: &Baseline) -> bool {
        match *self {
            Saves::Staged(staged) => newest.same_content(staged),
            Saves::Scanned {
                game,
                settings,
                compared,
            } => unchanged(game, settings, compared, newest),
        }
    }

    /// Whether these saves equal the snapshot `backup`.
    fn equal_backup(&self, backup: &Baseline) -> bool {
        match *self {
            Saves::Staged(staged) => backup.same_content(staged),
            Saves::Scanned { game, settings, .. } => {
                let (listing, hashes) = backup.compared();
                let comparison = changes::compare_game(game, listing, hashes);
                // Mapped, `skip_blocker` already refuses the registry and the meta is not compared.
                unchanged(game, settings, &comparison, backup)
                    && (backup.mapped.is_some()
                        || (backup.hashes.as_ref())
                            .is_some_and(|list| same_extras(game, settings, list)))
            }
        }
    }

    /// Whether these saves equal `list`, a staged hash list of this PC.
    fn equal_list(&self, list: &Hashes) -> bool {
        match *self {
            Saves::Staged(staged) => staged.same_content(list),
            Saves::Scanned { game, settings, .. } => {
                let comparison = changes::compare_game(game, &listing_of(list), Some(list));
                changes::counts(game, &comparison, settings) == Some(Counts::default())
                    && same_extras(game, settings, list)
            }
        }
    }
}

/// Whether a backup of `game` by `settings` would stage the `salvajocs.json` that `list` hashes,
/// and neither has registry keys, which a scan does not read: so a scan that finds `list`'s files
/// is equal by [`Hashes::same_content`] too.
fn same_extras(game: &GameScan, settings: &GameSettings, list: &Hashes) -> bool {
    list.registry.is_none()
        && selection::apply(game, settings).is_some_and(|applied| {
            applied.registry_source.is_none()
                && backup::meta_sha256(&applied).is_some_and(|meta| meta == list.meta)
        })
}

/// Whether `comparison` of `game` with `backup` finds nothing that a backup by `settings` would
/// change, and nothing makes that backup all the same ([`Baseline::skip_blocker`]).
fn unchanged(
    game: &GameScan,
    settings: &GameSettings,
    comparison: &Comparison,
    backup: &Baseline,
) -> bool {
    changes::counts(game, comparison, settings) == Some(Counts::default())
        && selection::apply(game, settings)
            .is_some_and(|applied| backup.skip_blocker(&applied).is_none())
}

/// The files of the staged hash list `list` as a listing of a snapshot's staged directory, for
/// [`changes::compare_game`].
fn listing_of(list: &Hashes) -> Vec<SnapshotFile> {
    (list.files.iter())
        .map(|(key, file)| SnapshotFile {
            path: format!("{FILES_DIR}/{key}"),
            size: file.size,
            modified: file.modified.map(|since| UNIX_EPOCH + since),
            kind: EntryKind::File,
        })
        .collect()
}

/// Where a game stands on this PC (ADR-0035), from this PC's `saves` (L), the game's newest
/// backup `newest` (N: [`changes::newest`] through [`changes::Mapper::baseline`]; `None` when the
/// repository has none) and this PC's `base` (B: [`Bases::base`]; `None` when there is none).
/// `base_backup` is the snapshot [`Base::to_read`] names, through the same mapper, when the
/// repository still has it; another snapshot is ignored. Callers read it only when `to_read`
/// names it, since the state depends on it only then. The preview and Back up both decide here,
/// from the scan and from the staged list. A scan does not read the registry or
/// `salvajocs.json`, so the two can differ, and Back up's decision then wins. A scan reads as B
/// only when neither can differ, so the preview never predicts [`State::Behind`] for a game Back
/// up asks about; against a newest of the same build it can read [`State::UpToDate`] when Back up
/// does not.
///
/// - No N: [`State::NoBackupYet`].
/// - L = N: [`State::UpToDate`]. Staged, by [`Baseline::same_content`]; scanned, when the counts
///   are all zero and nothing blocks the skip. N without a verified hash list is never equal.
/// - No B: [`State::UnknownBase`].
/// - N = B, by snapshot id or by B's hash list against N's: [`State::Ahead`]; but
///   [`State::UnknownBase`] for a seed, which is only this hostname's last backup and so may be
///   another PC's of the same name.
/// - L = B, by B's hash list, else by `base_backup` as L = N: [`State::Behind`].
/// - Otherwise, also when B has no list and `base_backup` is `None`: [`State::ChangedOnBoth`].
///
/// Reads nothing from the repository; a scan's files are read to hash those whose size and mtime
/// do not settle a comparison.
pub fn state(
    saves: &Saves<'_>,
    newest: Option<&Baseline>,
    base: Option<&Base>,
    base_backup: Option<&Baseline>,
) -> State {
    let Some(newest) = newest else {
        return State::NoBackupYet;
    };
    if saves.equal_newest(newest) {
        return State::UpToDate;
    }
    let Some(base) = base else {
        return State::UnknownBase;
    };
    let newest_is_base = newest.snapshot.id == base.snapshot
        || (base.saves.as_ref()).is_some_and(|list| newest.same_content(list));
    if newest_is_base {
        return if base.seed {
            State::UnknownBase
        } else {
            State::Ahead
        };
    }
    let base_backup = base_backup.filter(|backup| backup.snapshot.id == base.snapshot);
    let at_base = match (&base.saves, base_backup) {
        (Some(list), _) => saves.equal_list(list),
        (None, Some(backup)) => saves.equal_backup(backup),
        (None, None) => false,
    };
    if at_base {
        State::Behind
    } else {
        State::ChangedOnBoth
    }
}

#[cfg(test)]
mod tests;
