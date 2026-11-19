//! The retention policy in the repository: one snapshot at `/salvajocs-settings/retention`
//! holding `retention.json`, tagged `salvajocs-settings` and `settings:retention` (ADR-0015).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Retention, SETTINGS_TAG};
use crate::selection::forget_listed;
use crate::storage::{SnapshotInfo, Storage, StorageError};

/// The format of `retention.json` this version reads and writes.
pub const FORMAT: u32 = 1;

/// The tag of the policy snapshots. No game's `settings:<slug>` can match it: every slug holds a
/// hex hash.
pub(crate) const POLICY_TAG: &str = "settings:retention";
const AS_PATH: &str = "/salvajocs-settings/retention";
const POLICY_FILE: &str = "retention.json";
const LABEL: &str = "Retention policy";

/// Errors from reading or saving the retention policy.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RetentionError {
    /// The policy was written by a newer version: it is neither applied nor rewritten.
    #[error("retention policy format {0} is newer than this version understands")]
    NewerFormat(u32),
    /// The policy is not valid JSON for its format, or holds a field this version does not know.
    #[error("cannot parse the retention policy")]
    Parse(#[from] serde_json::Error),
    /// The policy file could not be staged.
    #[error("I/O failed on {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The repository failed.
    #[error("repository error")]
    Storage(#[from] StorageError),
    /// The repository found the new policy unchanged from an older snapshot and stored none.
    #[error("the retention policy snapshot was not written")]
    NotWritten,
}

/// The contents of `retention.json`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    format: u32,
    policy: Retention,
}

/// The repository's retention policy: the newest policy snapshot, off when there is none.
/// Reindexes first, so another machine's newer snapshot is read.
///
/// # Errors
/// [`RetentionError::NewerFormat`] or the parse error when the newest policy cannot be used,
/// in which case nothing may be forgotten; or the repository error.
pub fn load(storage: &mut Storage) -> Result<Retention, RetentionError> {
    storage.reindex()?;
    match storage.snapshots(POLICY_TAG)?.first() {
        Some(newest) => read(storage, newest),
        None => Ok(Retention::default()),
    }
}

/// The outcome of a [`save`] that succeeded: the newest policy snapshot holds the policy.
#[derive(Debug)]
#[non_exhaustive]
pub struct Saved {
    /// The new policy snapshot, `None` when the repository already held the policy.
    pub written: Option<SnapshotInfo>,
    /// Why the older policy snapshots could not be forgotten; a later save forgets them.
    pub forget_error: Option<StorageError>,
}

/// Saves `policy` as the repository's retention policy: reindexes, reads the newest policy and,
/// when it differs, writes `policy` as a new snapshot staged in `stage_root/settings/retention`.
/// Then forgets the older policy snapshots it listed before writing, never one written since.
///
/// # Errors
/// [`RetentionError::NewerFormat`] or the parse error when the newest policy cannot be read,
/// so it is never overwritten; [`RetentionError::NotWritten`], or the I/O or repository error.
/// A failed forget is not an error: it is in [`Saved::forget_error`].
pub fn save(
    storage: &mut Storage,
    policy: &Retention,
    stage_root: &Path,
) -> Result<Saved, RetentionError> {
    storage.reindex()?;
    let listed = storage.snapshots(POLICY_TAG)?;
    let current = match listed.first() {
        Some(newest) => read(storage, newest)?,
        None => Retention::default(),
    };
    let ids: Vec<String> = listed.into_iter().map(|snapshot| snapshot.id).collect();
    if current == *policy {
        // The newest snapshot already holds the policy; only older leftovers go.
        let older = ids.get(1..).unwrap_or_default();
        return Ok(Saved {
            written: None,
            forget_error: forget_listed(storage, POLICY_TAG, older),
        });
    }
    // Selection stages in `settings/<slug>`, and no slug is `retention`.
    let dir = stage_root.join("settings").join("retention");
    let file = dir.join(POLICY_FILE);
    let stored = Stored {
        format: FORMAT,
        policy: *policy,
    };
    let json = serde_json::to_vec_pretty(&stored).expect("a policy always serializes");
    fs::create_dir_all(&dir)
        .and_then(|()| fs::write(&file, json))
        .map_err(|source| RetentionError::Io { path: file, source })?;
    let written = storage
        .backup_dir(&dir, AS_PATH, &[SETTINGS_TAG, POLICY_TAG], LABEL)?
        .ok_or(RetentionError::NotWritten)?;
    Ok(Saved {
        written: Some(written),
        forget_error: forget_listed(storage, POLICY_TAG, &ids),
    })
}

fn read(storage: &mut Storage, snapshot: &SnapshotInfo) -> Result<Retention, RetentionError> {
    let bytes = storage.read_file(&snapshot.id, &format!("{AS_PATH}/{POLICY_FILE}"))?;
    parse(&bytes)
}

/// Parses `retention.json`. A `format` above [`FORMAT`] is [`RetentionError::NewerFormat`],
/// whatever the rest holds.
fn parse(bytes: &[u8]) -> Result<Retention, RetentionError> {
    #[derive(Deserialize)]
    struct Header {
        format: u32,
    }
    let Header { format } = serde_json::from_slice(bytes)?;
    if format > FORMAT {
        return Err(RetentionError::NewerFormat(format));
    }
    let Stored { policy, .. } = serde_json::from_slice(bytes)?;
    Ok(policy)
}

#[cfg(test)]
mod tests;
