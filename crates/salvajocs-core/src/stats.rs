//! What the game lists show and sort by about each game's backups (ADR-0027). Pure: the
//! snapshots come from [`crate::storage::Storage::snapshots`], the space from
//! [`crate::storage::Storage::space_used`].

use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::SystemTime;

use crate::config::{ListSort, SortKey};
use crate::restore::PRE_RESTORE_TAG;
use crate::storage::SnapshotInfo;

/// One game's backups, as a game list shows them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct BackupStats {
    /// When the last backup was made: the newest snapshot from any host that is not a
    /// pre-restore safety backup. `None` when there is no such snapshot.
    pub last: Option<SystemTime>,
    /// The size of the files in the last backup, before compression; `None` without a last
    /// backup or when its size is unknown.
    pub last_size: Option<u64>,
    /// The space all of the game's snapshots use, safety backups included; `None` until the
    /// caller sets it from [`crate::storage::Storage::space_used`] of `snapshot_ids`.
    pub space_used: Option<u64>,
    /// The ids of all of the game's snapshots, safety backups included, sorted.
    pub snapshot_ids: Vec<String>,
}

impl BackupStats {
    /// The figures of one game's `snapshots`, newest first, without the space used.
    pub fn of(snapshots: &[SnapshotInfo]) -> Self {
        let last = snapshots
            .iter()
            .find(|snapshot| !snapshot.tags.iter().any(|tag| tag == PRE_RESTORE_TAG));
        let mut snapshot_ids: Vec<String> = snapshots.iter().map(|s| s.id.clone()).collect();
        snapshot_ids.sort();
        Self {
            last: last.map(|snapshot| snapshot.time),
            last_size: last.and_then(|snapshot| snapshot.size),
            space_used: None,
            snapshot_ids,
        }
    }
}

/// The [`BackupStats`] of each game in `snapshots` (newest first), by the slug of its
/// `game:<slug>` tag. Snapshots without one are left out.
pub fn by_game(snapshots: &[SnapshotInfo]) -> HashMap<String, BackupStats> {
    let mut games: HashMap<&str, Vec<SnapshotInfo>> = HashMap::new();
    for snapshot in snapshots {
        let slug = (snapshot.tags.iter()).find_map(|tag| tag.strip_prefix("game:"));
        if let Some(slug) = slug {
            games.entry(slug).or_default().push(snapshot.clone());
        }
    }
    (games.into_iter())
        .map(|(slug, snapshots)| (slug.to_owned(), BackupStats::of(&snapshots)))
        .collect()
}

/// The indices of `rows`, each a title, its stats and its number of changes (ADR-0032), in the
/// order `sort` shows them. A row without a value for the key sorts after every row with one, in
/// both directions. Ties fall back to the title in byte order, then to the index, so name A to Z
/// keeps a list already in that order as it is.
pub fn order(rows: &[(&str, Option<&BackupStats>, Option<usize>)], sort: ListSort) -> Vec<usize> {
    let mut order: Vec<usize> = (0..rows.len()).collect();
    // A stable sort, so equal rows keep their index order.
    order.sort_by(|&a, &b| {
        let ((title_a, a, changes_a), (title_b, b, changes_b)) = (rows[a], rows[b]);
        let titles = title_a.cmp(title_b);
        let by_value = match sort.by {
            SortKey::Name if sort.descending => titles.reverse(),
            SortKey::Name => titles,
            SortKey::LastBackup => valued(a, b, sort.descending, |s| s.last),
            SortKey::LastBackupSize => valued(a, b, sort.descending, |s| s.last_size),
            SortKey::SpaceUsed => valued(a, b, sort.descending, |s| s.space_used),
            SortKey::Changes => present(changes_a, changes_b, sort.descending),
        };
        by_value.then(titles)
    });
    order
}

/// `a` against `b` by the `value` of each: a present value first, then by the value, reversed
/// when `descending`.
fn valued<T: Ord>(
    a: Option<&BackupStats>,
    b: Option<&BackupStats>,
    descending: bool,
    value: impl Fn(&BackupStats) -> Option<T>,
) -> Ordering {
    present(a.and_then(&value), b.and_then(&value), descending)
}

/// [`valued`] for values already read.
fn present<T: Ord>(a: Option<T>, b: Option<T>, descending: bool) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) if descending => b.cmp(&a),
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests;
