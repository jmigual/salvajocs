//! Which snapshots a retention policy forgets (ADR-0015).

use std::collections::{BTreeMap, HashSet};
use std::num::NonZeroU32;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::restore::PRE_RESTORE_TAG;
use crate::storage::{SnapshotInfo, Storage, StorageError};

mod repo;

pub(crate) use repo::POLICY_TAG;
pub use repo::{FORMAT, RetentionError, Saved, load, save};

const APP_TAG: &str = "salvajocs";
const GAME_TAG_PREFIX: &str = "game:";
const SETTINGS_TAG: &str = "salvajocs-settings";
const DAY_SECS: u64 = 24 * 60 * 60;

/// A retention policy. A rule left `None` is off; with both off nothing is forgotten.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Retention {
    /// Keep the newest this many snapshots of each game.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_last: Option<NonZeroU32>,
    /// Keep every snapshot taken within this span before now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_within: Option<Within>,
}

impl Retention {
    /// Whether no rule is set, so the policy forgets nothing.
    pub fn is_off(&self) -> bool {
        self.keep_last.is_none() && self.keep_within.is_none()
    }
}

/// Rules changed on this PC and not yet saved to the repository (ADR-0029).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Unsent {
    /// The rules to save with the next Back up.
    pub policy: Retention,
    /// The rules this PC had loaded when the change started.
    pub based_on: Retention,
}

impl Unsent {
    /// `policy`, changed from `based_on`.
    pub fn new(policy: Retention, based_on: Retention) -> Self {
        Self { policy, based_on }
    }

    /// Whether the repository's rules `stored` changed elsewhere since: they are neither the
    /// base nor the rules this PC saves.
    pub fn conflicts(&self, stored: &Retention) -> bool {
        *stored != self.based_on && *stored != self.policy
    }
}

/// A span of time counted back from now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Within {
    /// How many `unit`s.
    pub amount: NonZeroU32,
    /// The unit of `amount`.
    pub unit: Unit,
}

impl Within {
    /// The span as a duration.
    pub fn duration(&self) -> Duration {
        // ponytail: fixed-length months and years; calendar arithmetic if exact dates matter.
        let days = match self.unit {
            Unit::Days => 1,
            Unit::Weeks => 7,
            Unit::Months => 30,
            Unit::Years => 365,
        };
        Duration::from_secs(u64::from(self.amount.get()) * days * DAY_SECS)
    }
}

/// The unit of a [`Within`] span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Unit {
    /// Days.
    Days,
    /// Weeks of 7 days.
    Weeks,
    /// Months of 30 days.
    Months,
    /// Years of 365 days.
    Years,
}

/// Returns the ids of the snapshots `policy` forgets at `now`, in the order of `snapshots`.
///
/// Only snapshots tagged `salvajocs` and `game:<slug>`, and not `salvajocs-settings`, are
/// candidates. Each game has two sequences, regular and `pre-restore`, across all hosts. In each,
/// a snapshot is kept when it is among the newest `keep_last`, when it is no older than
/// `keep_within` before `now`, or when it is the newest of the sequence (ADR-0034). Ids in `keep`,
/// such as the snapshots a backup just made, are never forgotten. An off policy forgets nothing.
pub fn to_forget(
    policy: &Retention,
    snapshots: &[SnapshotInfo],
    keep: &[&str],
    now: SystemTime,
) -> Vec<String> {
    if policy.is_off() {
        return Vec::new();
    }
    // `None` when the span underflows `SystemTime`: then every snapshot is within it.
    let cutoff = policy
        .keep_within
        .map(|within| now.checked_sub(within.duration()));
    let mut sequences: BTreeMap<(&str, bool), Vec<&SnapshotInfo>> = BTreeMap::new();
    for snapshot in snapshots {
        if let Some(game) = candidate_game(snapshot) {
            let pre_restore = has_tag(snapshot, PRE_RESTORE_TAG);
            sequences
                .entry((game, pre_restore))
                .or_default()
                .push(snapshot);
        }
    }
    let mut forget = HashSet::new();
    for mut sequence in sequences.into_values() {
        sequence.sort_by(|a, b| b.time.cmp(&a.time).then_with(|| a.id.cmp(&b.id)));
        for (position, snapshot) in sequence.into_iter().enumerate() {
            let newest = position == 0;
            let in_last = policy
                .keep_last
                .is_some_and(|n| position < n.get() as usize);
            let in_window =
                cutoff.is_some_and(|cutoff| cutoff.is_none_or(|cutoff| snapshot.time >= cutoff));
            let pinned = keep.contains(&snapshot.id.as_str());
            if !(pinned || newest || in_last || in_window) {
                forget.insert(snapshot.id.as_str());
            }
        }
    }
    snapshots
        .iter()
        .filter(|snapshot| forget.contains(snapshot.id.as_str()))
        .map(|snapshot| snapshot.id.clone())
        .collect()
}

/// What [`apply`] or [`delete`] did.
#[derive(Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Cleanup {
    /// How many snapshots were forgotten.
    pub forgotten: usize,
    /// Whether a prune ran: after a forget, or while packs wait marked for deletion.
    pub pruned: bool,
    /// How many snapshots to forget stayed because the repository is append-only.
    pub append_only_kept: usize,
}

/// Why [`apply`] or [`delete`] stopped, with how many snapshots it had forgotten by then.
#[derive(Debug, thiserror::Error)]
#[error("the cleanup stopped after forgetting {forgotten} snapshots")]
#[non_exhaustive]
pub struct CleanupError {
    /// How many snapshots were forgotten before the error; their data waits for a later prune.
    /// After a failed forget, those no longer listed, or all it tried when listing fails too.
    pub forgotten: usize,
    /// The repository error.
    #[source]
    pub source: StorageError,
}

impl CleanupError {
    /// The cleanup stopped by `source` after forgetting `forgotten` snapshots.
    pub fn new(forgotten: usize, source: StorageError) -> Self {
        Self { forgotten, source }
    }
}

/// Forgets the snapshots `policy` forgets at `now` ([`to_forget`] over every `salvajocs`
/// snapshot), never one of `keep`. Then prunes if anything was forgotten or the repository has
/// packs marked for deletion, which only a later prune frees, or recovers when a backup made
/// since uses them. An off policy forgets nothing but still prunes marked packs. An append-only
/// repository is neither forgotten from nor pruned: the snapshots to forget are only counted.
///
/// # Errors
/// The repository error of the listing, the forget or the prune, with how many snapshots were
/// forgotten before it. A forget that fails partway counts by listing again (see
/// [`CleanupError::forgotten`]).
pub fn apply(
    storage: &mut Storage,
    policy: &Retention,
    keep: &[&str],
    now: SystemTime,
) -> Result<Cleanup, CleanupError> {
    let ids = if policy.is_off() {
        Vec::new()
    } else {
        let snapshots = storage.snapshots(APP_TAG).map_err(|source| CleanupError {
            forgotten: 0,
            source,
        })?;
        to_forget(policy, &snapshots, keep, now)
    };
    forget_and_prune(storage, &ids.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Why [`delete`] did not finish.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DeleteError {
    /// The id is not a listed game backup: not tagged `salvajocs` and `game:<slug>`, tagged
    /// `salvajocs-settings`, or gone. Nothing was forgotten.
    #[error("snapshot {0} is not a game backup in the repository")]
    NotAGameBackup(String),
    /// The listing, the forget or the prune failed, with how many snapshots were forgotten.
    #[error(transparent)]
    Stopped(#[from] CleanupError),
}

/// Forgets the game backups `ids` (full hex ids), then prunes as [`apply`] does, never with
/// `instant_delete`. Every id must be a snapshot [`to_forget`] could pick, else nothing is
/// forgotten. An append-only repository is neither forgotten from nor pruned: the ids are only
/// counted in [`Cleanup::append_only_kept`].
///
/// # Errors
/// [`DeleteError::NotAGameBackup`] for the first id that is not a listed game backup, or the
/// repository error with the forgotten count as [`apply`] reports it.
pub fn delete(storage: &mut Storage, ids: &[&str]) -> Result<Cleanup, DeleteError> {
    let snapshots = storage.snapshots(APP_TAG).map_err(|source| CleanupError {
        forgotten: 0,
        source,
    })?;
    let listed = |id: &&str| (snapshots.iter()).any(|s| s.id == *id && candidate_game(s).is_some());
    if let Some(id) = ids.iter().find(|id| !listed(id)) {
        return Err(DeleteError::NotAGameBackup((*id).to_owned()));
    }
    Ok(forget_and_prune(storage, ids)?)
}

/// Forgets `ids`, then prunes if anything was forgotten or packs wait marked for deletion. In
/// an append-only repository it only counts `ids`.
fn forget_and_prune(storage: &mut Storage, ids: &[&str]) -> Result<Cleanup, CleanupError> {
    let stopped = |forgotten| move |source| CleanupError { forgotten, source };
    let append_only = storage.append_only();
    let (mut forgotten, mut append_only_kept) = (0, 0);
    if append_only {
        append_only_kept = ids.len();
    } else {
        if !ids.is_empty()
            && let Err(source) = storage.forget(ids)
        {
            // rustic deletes in parallel, so some may be gone. Over-counting when the
            // listing fails too only makes a caller drop a list it would reload.
            let forgotten = match storage.snapshots(APP_TAG) {
                Ok(left) => ids
                    .iter()
                    .filter(|id| !left.iter().any(|s| s.id == **id))
                    .count(),
                Err(_) => ids.len(),
            };
            return Err(CleanupError { forgotten, source });
        }
        forgotten = ids.len();
    }
    // rustic refuses an append-only prune only after planning it, which walks every tree.
    let pruned = forgotten > 0
        || (!append_only && storage.has_marked_packs().map_err(stopped(forgotten))?);
    if pruned {
        storage.prune().map_err(stopped(forgotten))?;
    }
    Ok(Cleanup {
        forgotten,
        pruned,
        append_only_kept,
    })
}

/// The `game:` tag of a snapshot retention may forget.
fn candidate_game(snapshot: &SnapshotInfo) -> Option<&str> {
    if !has_tag(snapshot, APP_TAG) || has_tag(snapshot, SETTINGS_TAG) {
        return None;
    }
    snapshot
        .tags
        .iter()
        .find(|tag| tag.starts_with(GAME_TAG_PREFIX))
        .map(String::as_str)
}

fn has_tag(snapshot: &SnapshotInfo, tag: &str) -> bool {
    snapshot.tags.iter().any(|t| t == tag)
}

#[cfg(test)]
mod tests;
