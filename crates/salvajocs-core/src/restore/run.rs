//! The runtime side of a restore: the restore list, the default targets, fetching a snapshot
//! and running a plan behind a safety backup (ADR-0005, ADR-0006, ADR-0010).

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::slice;

use serde::Deserialize;
use serde::de::{DeserializeOwned, IntoDeserializer, value};

use super::{RestorePlan, StagedGame, Targets, Write};
use crate::backup::{BackupError, backup_games};
use crate::config::AppConfig;
use crate::crossbuild::{self, Family, pair_side};
use crate::layout::{
    self, Base, Context, FILES_DIR, GameVars, META_FILE, REGISTRY_FILE, RootKind, WinFolders,
};
use crate::manifest::{CustomGame, GameEntry, Manifest, Pair, merge_custom};
use crate::prefix::{proton_prefix, wine_user};
#[cfg(windows)]
use crate::registry::windows::WindowsRegistryError;
use crate::registry::wine::RegFile;
use crate::registry::{RegistryError, RegistryExport};
use crate::scan::{Environment, FoundFile, GameScan, RegistrySource};
use crate::storage::{SnapshotInfo, Storage, StorageError};

/// The extra tag of a safety backup made before a restore (ADR-0010).
pub const PRE_RESTORE_TAG: &str = "pre-restore";

/// The snapshots of one game in the restore list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameBackups {
    /// The game's slug, from its `game:<slug>` tag.
    pub slug: String,
    /// The game's title, the label of its newest snapshot.
    pub title: String,
    /// Every snapshot of the game from every host, newest first.
    pub snapshots: Vec<SnapshotInfo>,
    /// The index in `snapshots` of the newest one without [`PRE_RESTORE_TAG`]; `None` when
    /// there is none.
    pub default: Option<usize>,
}

/// Lists every game in the repository with its snapshots from all hosts, sorted by title
/// (ADR-0010). A `pre-restore` snapshot is listed but never the default.
///
/// # Errors
/// The repository error when the snapshots cannot be read.
pub fn list(storage: &Storage) -> Result<Vec<GameBackups>, StorageError> {
    let mut games: BTreeMap<String, Vec<SnapshotInfo>> = BTreeMap::new();
    for snapshot in storage.snapshots("salvajocs")? {
        let Some(slug) = snapshot
            .tags
            .iter()
            .find_map(|tag| tag.strip_prefix("game:"))
        else {
            continue;
        };
        games.entry(slug.to_owned()).or_default().push(snapshot);
    }
    let mut list: Vec<GameBackups> = games
        .into_iter()
        .map(|(slug, snapshots)| GameBackups {
            slug,
            title: snapshots[0].label.clone(),
            default: snapshots
                .iter()
                .position(|snapshot| !snapshot.tags.iter().any(|tag| tag == PRE_RESTORE_TAG)),
            snapshots,
        })
        .collect();
    list.sort_by(|a, b| a.title.cmp(&b.title));
    Ok(list)
}

/// The targets a restore of `title` starts with (ADR-0005). On Windows the native folders; on
/// Linux the native folders plus, when it exists, the game's own prefix: the custom game's
/// `wine_prefix`, else its Proton prefix under `env.steam_roots` or a configured Steam root,
/// as the scan looks them up. The user may change them. A prefix whose Wine user cannot be
/// determined is passed over.
pub fn default_targets(
    title: &str,
    manifest: &Manifest,
    config: &AppConfig,
    env: &Environment,
) -> Targets {
    let custom = &config.custom_games;
    match &env.native {
        Some(win @ Context::WindowsNative(_)) => Targets {
            linux: None,
            win: Some(win.clone()),
        },
        native => {
            let steam_id = (game_info(title, manifest, custom).entry)
                .and_then(|entry| entry.steam?.id)
                .map(u64::from);
            let custom_prefix = (custom.iter())
                .filter(|game| game.name == title)
                .filter_map(|game| game.wine_prefix.clone())
                .next_back();
            let configured = (config.roots.iter())
                .filter(|root| root.kind == RootKind::Steam)
                .map(|root| &root.path);
            let proton = (env.steam_roots.iter())
                .chain(configured)
                .filter_map(|root| proton_prefix(root, steam_id?));
            let win = custom_prefix.into_iter().chain(proton).find_map(|prefix| {
                let user = wine_user(&prefix).ok()?;
                let folders = WinFolders::for_prefix(&prefix, &user);
                Some(Context::WinePrefix { prefix, folders })
            });
            let linux = match native {
                Some(Context::Linux(folders)) => Some(folders.clone()),
                _ => None,
            };
            Targets { linux, win }
        }
    }
}

/// What the manifest and the custom games say about one game, as a restore needs it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GameInfo {
    /// The manifest entry with the custom game applied (`manifest::merge_custom`); `None` when
    /// neither has the title.
    pub entry: Option<GameEntry>,
    /// The pairs of the custom game's file rows (ADR-0016) a saved skip does not name, then its
    /// saved pairs (ADR-0011), so a saved pair wins a tie.
    pub pairs: Vec<Pair>,
    /// The custom game's saved skips (ADR-0011).
    pub skips: Vec<String>,
}

/// The entry, pairs and skips of `title` from `manifest` and the `custom` games, the inputs
/// [`super::prepare`] takes besides the snapshot, targets and variables.
pub fn game_info(title: &str, manifest: &Manifest, custom: &[CustomGame]) -> GameInfo {
    let mut merged = Manifest {
        games: (manifest.games.get(title))
            .map(|entry| (title.to_owned(), entry.clone()))
            .into_iter()
            .collect(),
    };
    merge_custom(&mut merged, custom);
    let custom = || custom.iter().filter(|game| game.name == title);
    let skips: Vec<String> = custom().flat_map(|game| game.skips.clone()).collect();
    // A skip the user saved wins over a row's mapping of the same root, as it does over a pair.
    let skipped = |pair: &Pair| {
        [Family::Linux, Family::Windows].into_iter().any(|family| {
            let side = crossbuild::classify(pair_side(pair, family), family);
            side.is_some()
                && skips
                    .iter()
                    .any(|skip| crossbuild::classify(skip, family) == side)
        })
    };
    let rows = custom()
        .flat_map(CustomGame::file_pairs)
        .filter(|pair| !skipped(pair));
    GameInfo {
        entry: merged.games.remove(title),
        pairs: rows
            .chain(custom().flat_map(|game| game.pairs.clone()))
            .collect(),
        skips,
    }
}

/// The template variables of `title` for a restore, built as the scan builds them: the install
/// dirs and Steam id of `entry` (the manifest entry with its custom game applied, see
/// `manifest::merge_custom`) and the roots of `env` and `config`.
pub fn game_vars(
    title: &str,
    entry: Option<&GameEntry>,
    config: &AppConfig,
    env: &Environment,
) -> GameVars {
    let install_dirs = entry
        .map(|entry| entry.install_dir.keys().cloned().collect())
        .unwrap_or_default();
    let steam_id = entry.and_then(|entry| entry.steam.as_ref()?.id);
    GameVars::new(
        title,
        install_dirs,
        &crate::scan::roots(config, env),
        steam_id.map(|id| id.to_string()),
    )
}

/// Why [`fetch`] failed.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// The snapshot has no `game:<slug>` tag with a plain slug.
    #[error("snapshot {0} is not a game backup")]
    NotAGame(String),
    /// The work dir holds files, which would be taken for staged files.
    #[error("the work dir is not empty: {0}")]
    WorkDirNotEmpty(PathBuf),
    /// An entry of `files/` is not a base directory.
    #[error("not a base directory in the snapshot: {0}")]
    UnknownBase(String),
    /// `files/` is not a real directory, or `salvajocs.json` or `registry.json` not a regular
    /// file (a symlink, for instance).
    #[error("unexpected file type in the snapshot: {0}")]
    WrongKind(PathBuf),
    /// The restored game could not be read.
    #[error("I/O failed on {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// `salvajocs.json` or `registry.json` cannot be parsed.
    #[error("cannot parse {path}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    /// The repository failed.
    #[error("repository error")]
    Storage(#[from] StorageError),
}

fn fetch_io(path: &Path) -> impl FnOnce(io::Error) -> FetchError + '_ {
    move |source| FetchError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Restores the staged game `/salvajocs/<slug>` of `snapshot` into `work_dir`, which must be
/// absent or empty, and reads it back (ADR-0004). The result is untrusted until
/// [`super::plan`] validates it. Files are sorted by base and tail.
///
/// # Errors
/// [`FetchError::NotAGame`] without a `game:` tag, [`FetchError::WorkDirNotEmpty`],
/// [`FetchError::UnknownBase`] for an entry of `files/` that is no base directory,
/// [`FetchError::WrongKind`] for a symlinked `files/` or JSON file, and the repository, I/O or
/// JSON error otherwise.
pub fn fetch(
    storage: &mut Storage,
    snapshot: &SnapshotInfo,
    work_dir: &Path,
) -> Result<StagedGame, FetchError> {
    let slug = (snapshot.tags.iter())
        .find_map(|tag| tag.strip_prefix("game:"))
        .filter(|slug| layout::is_plain_component(slug))
        .ok_or_else(|| FetchError::NotAGame(snapshot.id.clone()))?;
    match fs::read_dir(work_dir) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(FetchError::WorkDirNotEmpty(work_dir.to_path_buf()));
            }
        }
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(fetch_io(work_dir)(err)),
        Err(_) => {}
    }
    storage.restore_path(&snapshot.id, &format!("/salvajocs/{slug}"), work_dir)?;

    let meta_file = work_dir.join(META_FILE);
    exists_as(&meta_file, false)?;
    let meta = read_json(&meta_file)?;
    let registry_file = work_dir.join(REGISTRY_FILE);
    let registry = if exists_as(&registry_file, false)? {
        Some(read_json(&registry_file)?)
    } else {
        None
    };
    let mut files = Vec::new();
    let files_dir = work_dir.join(FILES_DIR);
    if exists_as(&files_dir, true)? {
        for entry in fs::read_dir(&files_dir).map_err(fetch_io(&files_dir))? {
            let entry = entry.map_err(fetch_io(&files_dir))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = (entry.file_type())
                .map_err(fetch_io(&entry.path()))?
                .is_dir();
            let deserializer = IntoDeserializer::<value::Error>::into_deserializer(name.as_str());
            let base = Base::deserialize(deserializer)
                .ok()
                .filter(|_| is_dir)
                .ok_or(FetchError::UnknownBase(name))?;
            let mut tails = Vec::new();
            collect_tails(&entry.path(), "", &mut tails)?;
            files.extend(tails.into_iter().map(|tail| (base, tail)));
        }
    }
    files.sort();
    Ok(StagedGame {
        root: work_dir.to_path_buf(),
        meta,
        files,
        registry,
    })
}

/// Whether `path` exists, as a real directory with `dir` or a regular file without. A snapshot
/// is untrusted, so a symlink here would read host data as staged.
fn exists_as(path: &Path, dir: bool) -> Result<bool, FetchError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if (if dir { meta.is_dir() } else { meta.is_file() }) => Ok(true),
        Ok(_) => Err(FetchError::WrongKind(path.to_path_buf())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(fetch_io(path)(err)),
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, FetchError> {
    let bytes = fs::read(path).map_err(fetch_io(path))?;
    serde_json::from_slice(&bytes).map_err(|source| FetchError::Json {
        path: path.to_path_buf(),
        source,
    })
}

/// Pushes the `/`-separated tail of every non-directory below `dir`, after `prefix`. A symlink
/// is listed, never followed, so the restore can refuse it.
fn collect_tails(dir: &Path, prefix: &str, out: &mut Vec<String>) -> Result<(), FetchError> {
    for entry in fs::read_dir(dir).map_err(fetch_io(dir))? {
        let entry = entry.map_err(fetch_io(dir))?;
        let path = entry.path();
        let name = entry.file_name();
        let tail = match prefix {
            "" => name.to_string_lossy().into_owned(),
            _ => format!("{prefix}/{}", name.to_string_lossy()),
        };
        if entry.file_type().map_err(fetch_io(&path))?.is_dir() {
            collect_tails(&path, &tail, out)?;
        } else {
            out.push(tail);
        }
    }
    Ok(())
}

/// Why [`run`] wrote nothing.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The plan replaces registry keys in a Wine prefix while a wineserver runs, which would
    /// overwrite them on its next save (ADR-0006).
    #[error("a wineserver is running: close every Wine and Proton program first")]
    WineserverRunning,
    /// An overridden target exists but has no `Absolute`/`WinAbsolute` tail (such as a UNC
    /// path), so the safety backup cannot hold it.
    #[error("cannot back up the overridden target {0}")]
    UnstageableTarget(PathBuf),
    /// The safety backup failed.
    #[error("the safety backup failed, nothing was restored")]
    SafetyBackup(#[from] BackupError),
}

/// Why one write, or the registry, of a plan was not applied.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    /// The staged source is a symlink or anything else but a regular file.
    #[error("the staged file is not a regular file")]
    SourceNotAFile,
    /// The target's final component is a symlink, which is never written through.
    #[error("target is a symlink")]
    TargetIsSymlink,
    /// Reading or writing failed.
    #[error("I/O failed on {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The prefix's `user.reg` cannot be parsed or the keys cannot be replaced.
    #[error("cannot update the registry in {path}")]
    WineRegistry {
        path: PathBuf,
        #[source]
        source: RegistryError,
    },
    /// The live Windows registry cannot be written.
    #[cfg(windows)]
    #[error("cannot write the Windows registry")]
    WindowsRegistry(#[from] WindowsRegistryError),
    /// The plan's registry target has no registry this OS can write.
    #[error("the registry target cannot be written on this OS")]
    NoRegistry,
}

fn apply_io(path: &Path) -> impl FnOnce(io::Error) -> ApplyError + '_ {
    move |source| ApplyError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// The safety backup [`run`] made before writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyBackup {
    /// No target file and none of the replaced registry keys existed, so none was needed.
    NothingExisted,
    /// The repository already held exactly that state, so no snapshot was added.
    Unchanged,
    /// The new [`PRE_RESTORE_TAG`] snapshot.
    Created(SnapshotInfo),
}

/// What [`run`] did.
#[derive(Debug)]
pub struct RestoreReport {
    /// The safety backup.
    pub safety_backup: SafetyBackup,
    /// The outcome of each of the plan's writes, in order.
    pub files: Vec<Result<(), ApplyError>>,
    /// The outcome of the registry restore; `None` when the plan has none.
    pub registry: Option<Result<(), ApplyError>>,
}

/// Runs `plan` for the game `title`; the only way a restore writes (ADR-0005):
/// 1. a registry restore into a Wine prefix is refused while `wineserver_running` (ADR-0006);
/// 2. whatever the plan overwrites is backed up as a snapshot of `title` tagged
///    [`PRE_RESTORE_TAG`], with `roots` for the target roots it covers: existing target files
///    (an overridden one by its full path under `Absolute`/`WinAbsolute`) and the registry
///    subtrees it replaces. It is staged in `stage_root`, which must not be the backup mirror's
///    since staging removes files. Nothing existing at the targets counts as success;
/// 3. each write is copied into a temp file renamed over its target, then the registry keys
///    are replaced. A staged source that is not a regular file and a target that is a symlink
///    are refused; symlinked parent directories are followed (Proton's legacy dirs).
///
/// Per-file and registry failures in step 3 are in the [`RestoreReport`]; the rest goes on.
///
/// # Errors
/// [`RunError::WineserverRunning`], [`RunError::UnstageableTarget`] or
/// [`RunError::SafetyBackup`]; then nothing was written.
pub fn run(
    storage: &mut Storage,
    title: &str,
    plan: &RestorePlan,
    stage_root: &Path,
    wineserver_running: bool,
) -> Result<RestoreReport, RunError> {
    if wineserver_running && matches!(plan.registry, Some((_, Context::WinePrefix { .. }))) {
        return Err(RunError::WineserverRunning);
    }
    let checks: Vec<_> = plan.writes.iter().map(check).collect();
    let mut existing = GameScan {
        title: title.to_owned(),
        files: Vec::new(),
        roots: Vec::new(),
        registry_keys: Vec::new(),
        registry_source: None,
        unreadable: Vec::new(),
    };
    for (write, check) in plan.writes.iter().zip(&checks) {
        let Ok(Some(size)) = *check else {
            continue;
        };
        let (file, root) = match (&write.target_file, &write.target_root) {
            (Some(file), Some(root)) => (file.clone(), root.clone()),
            _ => {
                let tail = layout::tail_of(&write.target, Path::new(""))
                    .ok_or_else(|| RunError::UnstageableTarget(write.target.clone()))?;
                let base = if cfg!(windows) {
                    Base::WinAbsolute
                } else {
                    Base::Absolute
                };
                ((base, tail.clone()), (base, tail))
            }
        };
        existing.files.push(FoundFile {
            base: file.0,
            tail: file.1,
            real: write.target.clone(),
            size,
        });
        if !existing.roots.contains(&root) {
            existing.roots.push(root);
        }
    }
    if let Some((export, ctx)) = &plan.registry {
        existing.registry_keys = export.keys.keys().cloned().collect();
        existing.registry_source = registry_source(&existing.registry_keys, ctx);
    }
    let safety_backup = if existing.files.is_empty() && existing.registry_source.is_none() {
        SafetyBackup::NothingExisted
    } else {
        let tags = [PRE_RESTORE_TAG];
        let games = slice::from_ref(&existing);
        let mut outcomes = backup_games(storage, games, stage_root, &tags, |_| {})?;
        let outcome = outcomes.pop().expect("one outcome per game");
        outcome
            .result?
            .map_or(SafetyBackup::Unchanged, SafetyBackup::Created)
    };

    let files = (plan.writes.iter().zip(checks))
        .map(|(write, check)| check.and_then(|_| apply_file(write)))
        .collect();
    let registry = (plan.registry.as_ref()).map(|(export, ctx)| apply_registry(export, ctx));
    Ok(RestoreReport {
        safety_backup,
        files,
        registry,
    })
}

/// Where the safety backup reads the `keys` a restore into `ctx` replaces; `None` when none of
/// them exists there, so nothing is lost. A source that cannot be read is kept, so the backup
/// fails on it rather than the restore overwriting what it could not save.
fn registry_source(keys: &[String], ctx: &Context) -> Option<RegistrySource> {
    match ctx {
        Context::WinePrefix { prefix, .. } => {
            let holds_keys = match fs::read_to_string(prefix.join("user.reg")) {
                // No `user.reg`, no keys to lose; applying reports it missing.
                Err(err) if err.kind() == io::ErrorKind::NotFound => false,
                Err(_) => true,
                Ok(text) => holds_keys(RegFile::parse(&text).and_then(|file| file.export(keys))),
            };
            holds_keys.then(|| RegistrySource::WinePrefix(prefix.clone()))
        }
        #[cfg(windows)]
        Context::WindowsNative(_) => {
            holds_keys(crate::registry::windows::export(keys)).then_some(RegistrySource::Windows)
        }
        // Off Windows the backup reports that the live registry cannot be read.
        #[cfg(not(windows))]
        Context::WindowsNative(_) => Some(RegistrySource::Windows),
        Context::Linux(_) => None,
    }
}

/// Whether an export holds keys; a failed one counts, so the backup reports the failure.
fn holds_keys<E>(export: Result<RegistryExport, E>) -> bool {
    export.map_or(true, |export| !export.keys.is_empty())
}

/// Whether `write` may be applied: its source is a regular file and its target is not a
/// symlink. Gives the target's size when it is an existing regular file, to back up.
fn check(write: &Write) -> Result<Option<u64>, ApplyError> {
    let source = fs::symlink_metadata(&write.source).map_err(apply_io(&write.source))?;
    if !source.is_file() {
        return Err(ApplyError::SourceNotAFile);
    }
    match fs::symlink_metadata(&write.target) {
        Ok(target) if target.is_symlink() => Err(ApplyError::TargetIsSymlink),
        Ok(target) => Ok(target.is_file().then_some(target.len())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(apply_io(&write.target)(err)),
    }
}

fn apply_file(write: &Write) -> Result<(), ApplyError> {
    let mut source = File::open(&write.source).map_err(apply_io(&write.source))?;
    let modified = (source.metadata())
        .and_then(|meta| meta.modified())
        .map_err(apply_io(&write.source))?;
    replace(&write.target, |file| {
        io::copy(&mut source, file)?;
        file.set_modified(modified)
    })
}

fn apply_registry(export: &RegistryExport, ctx: &Context) -> Result<(), ApplyError> {
    match ctx {
        Context::WinePrefix { prefix, .. } => {
            let path = prefix.join("user.reg");
            let text = fs::read_to_string(&path).map_err(apply_io(&path))?;
            let mut file = RegFile::parse(&text).map_err(|source| ApplyError::WineRegistry {
                path: path.clone(),
                source,
            })?;
            (file.replace_subtrees(export)).map_err(|source| ApplyError::WineRegistry {
                path: path.clone(),
                source,
            })?;
            replace(&path, |out| out.write_all(file.to_string().as_bytes()))
        }
        #[cfg(windows)]
        Context::WindowsNative(_) => Ok(crate::registry::windows::replace_subtrees(export)?),
        _ => Err(ApplyError::NoRegistry),
    }
}

/// Writes `target` through `fill` into a new temp file beside it, then renames it over
/// `target`: a failure never leaves a half-written file, and a link at `target` is replaced,
/// not written through.
fn replace(
    target: &Path,
    fill: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<(), ApplyError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(apply_io(parent))?;
    }
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(".salvajocs-tmp");
    let tmp = PathBuf::from(tmp);
    match fs::remove_file(&tmp) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(apply_io(&tmp)(err)),
        _ => {}
    }
    // `create_new` does not follow a link at `tmp`; the file is closed before the rename, which
    // Windows needs, and synced so a crash cannot leave the renamed file empty.
    let written = File::create_new(&tmp).and_then(|mut file| {
        fill(&mut file)?;
        file.sync_all()
    });
    let result = written.and_then(|()| fs::rename(&tmp, target));
    if result.is_err() {
        // Best effort: the write's own error is the one to report.
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(apply_io(target))
}

#[cfg(test)]
mod tests;
