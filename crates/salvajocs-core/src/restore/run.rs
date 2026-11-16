//! The runtime side of a restore: the restore list, the default targets, fetching a snapshot
//! and running a plan behind a safety backup (ADR-0005, ADR-0006, ADR-0010).

use std::collections::BTreeMap;

use super::Targets;
use crate::layout::{Context, WinFolders};
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

#[cfg(test)]
mod tests;
