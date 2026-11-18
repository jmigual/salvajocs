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
/// `keep_within` before `now`, or when it is the newest of its host. Ids in `keep`, such as the
/// snapshots a backup just made, are never forgotten. An off policy forgets nothing.
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
        // The sequence's newest is also the newest of its host, so this guard covers both.
        let mut hosts = HashSet::new();
        for (position, snapshot) in sequence.into_iter().enumerate() {
            let newest_of_host = hosts.insert(snapshot.host.as_str());
            let in_last = policy
                .keep_last
                .is_some_and(|n| position < n.get() as usize);
            let in_window =
                cutoff.is_some_and(|cutoff| cutoff.is_none_or(|cutoff| snapshot.time >= cutoff));
            let pinned = keep.contains(&snapshot.id.as_str());
            if !(pinned || newest_of_host || in_last || in_window) {
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

/// What [`apply`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cleanup {
    /// How many snapshots were forgotten.
    pub forgotten: usize,
    /// Whether a prune ran: after a forget, or while packs wait marked for deletion.
    pub pruned: bool,
}

/// Forgets the snapshots `policy` forgets at `now` ([`to_forget`] over every `salvajocs`
/// snapshot), never one of `keep`. Then prunes if anything was forgotten or the repository has
/// packs marked for deletion, which only a later prune frees, or recovers when a backup made
/// since uses them. An off policy forgets nothing but still prunes marked packs.
///
/// # Errors
/// The repository error of the listing, the forget or the prune.
pub fn apply(
    storage: &mut Storage,
    policy: &Retention,
    keep: &[&str],
    now: SystemTime,
) -> Result<Cleanup, StorageError> {
    let mut forgotten = 0;
    if !policy.is_off() {
        let ids = to_forget(policy, &storage.snapshots(APP_TAG)?, keep, now);
        if !ids.is_empty() {
            storage.forget(&ids.iter().map(String::as_str).collect::<Vec<_>>())?;
        }
        forgotten = ids.len();
    }
    let pruned = forgotten > 0 || storage.has_marked_packs()?;
    if pruned {
        storage.prune()?;
    }
    Ok(Cleanup { forgotten, pruned })
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
