//! The runtime side of a restore: the restore list, the default targets, fetching a snapshot
//! and running a plan behind a safety backup (ADR-0005, ADR-0006, ADR-0010).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::{DeserializeOwned, IntoDeserializer, value};

use super::{StagedGame, Targets};
use crate::layout::{self, Base, Context, FILES_DIR, META_FILE, REGISTRY_FILE, WinFolders};
use crate::manifest::{CustomGame, Manifest, merge_custom};
use crate::prefix::{proton_prefix, wine_user};
use crate::scan::Environment;
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
/// `wine_prefix`, else its Proton prefix under `env.steam_roots`. The user may change them.
/// A prefix whose Wine user cannot be determined is passed over.
pub fn default_targets(
    title: &str,
    manifest: &Manifest,
    custom: &[CustomGame],
    env: &Environment,
) -> Targets {
    match &env.native {
        Some(win @ Context::WindowsNative(_)) => Targets {
            linux: None,
            win: Some(win.clone()),
        },
        native => {
            let mut merged = Manifest {
                games: (manifest.games.get(title))
                    .map(|entry| (title.to_owned(), entry.clone()))
                    .into_iter()
                    .collect(),
            };
            merge_custom(&mut merged, custom);
            let steam_id = (merged.games.get(title))
                .and_then(|entry| entry.steam.as_ref()?.id)
                .map(u64::from);
            let custom_prefix = (custom.iter())
                .filter(|game| game.name == title)
                .filter_map(|game| game.wine_prefix.clone())
                .next_back();
            let proton = (env.steam_roots.iter()).filter_map(|root| proton_prefix(root, steam_id?));
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
/// [`FetchError::UnknownBase`] for an entry of `files/` that is no base directory, and the
/// repository, I/O or JSON error otherwise.
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

    let meta = read_json(&work_dir.join(META_FILE))?;
    let registry_file = work_dir.join(REGISTRY_FILE);
    let registry = if fs::exists(&registry_file).map_err(fetch_io(&registry_file))? {
        Some(read_json(&registry_file)?)
    } else {
        None
    };
    let mut files = Vec::new();
    let files_dir = work_dir.join(FILES_DIR);
    if fs::exists(&files_dir).map_err(fetch_io(&files_dir))? {
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

#[cfg(test)]
mod tests;
