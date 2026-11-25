//! The files on this PC a restore may delete because its backup lacks them (ADR-0036). Nothing
//! here writes to disk.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{
    GameInfo, Listed, PRE_RESTORE_TAG, RestorePlan, components, covering_root, declared_roots,
    file_declared, resolve_path,
};
use crate::crossbuild::SnapshotRoot;
use crate::layout::{Base, GameVars};
use crate::selection::GameSettings;

/// A file on this PC that the backup a plan restores lacks (ADR-0036).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Delete {
    /// The file.
    pub target: PathBuf,
    /// `target` as `(base, tail)`, as a write's `target_file`, for the safety backup and the
    /// plan's rows.
    pub target_file: (Base, String),
    /// The raw root `target_file` lies under, for the safety backup's `roots`.
    pub target_root: SnapshotRoot,
}

/// A target dir [`deletions`] could not list, so nothing below it is deleted (ADR-0004).
#[derive(Debug, thiserror::Error)]
#[error("cannot list {path}")]
#[non_exhaustive]
pub struct UnlistedDir {
    /// The dir.
    pub path: PathBuf,
    #[source]
    pub source: io::Error,
}

/// What [`deletions`] found.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Deletions {
    /// The files a restore of the plan may delete, sorted by path.
    pub files: Vec<Delete>,
    /// The target dirs that could not be listed, so nothing below them is in `files`.
    pub unlisted: Vec<UnlistedDir>,
}

/// The files on this PC a restore of `plan` may delete because `listed`, the snapshot `plan`
/// was made from, lacks them (ADR-0036): each regular file below the target dir of a root that
/// `plan` placed a write under, which the game declares (`info`, as [`super::prepare`] took
/// it) and which a backup holding it would restore onto itself. A file is kept when the backup
/// holds it or a write targets it (both ASCII case ignored), or when the snapshot's recorded
/// selection excludes it by its snapshot-side key or `today`'s by either key. `today` is the
/// game's selection now with the pending changes, `None` when it cannot be read; its `skip`
/// (the game not backed up) protects nothing.
///
/// Nothing is found for a `pre-restore` snapshot, without `today`, under a root or mapping
/// target that is a whole base (a bare `WinAbsolute` drive too), or below a target dir that
/// cannot be listed.
pub fn deletions(
    listed: &Listed,
    plan: &RestorePlan,
    vars: &GameVars,
    info: &GameInfo,
    today: Option<&GameSettings>,
) -> Deletions {
    let mut found = Deletions::default();
    // A safety backup holds only what a restore touched, not the whole folder.
    let pre_restore = (listed.snapshot.tags.iter()).any(|tag| tag == PRE_RESTORE_TAG);
    let Some(today) = today.filter(|_| !pre_restore) else {
        return found;
    };
    let declared = declared_roots(info.entry.as_ref(), &info.pairs);
    let recorded = GameSettings {
        paths: (listed.hashes.as_ref())
            .map(|hashes| hashes.selection.clone())
            .unwrap_or_default(),
        ..GameSettings::default()
    };
    // `target_path` reuses an existing case variant, and a case-insensitive file system holds
    // one file for both, so a case variant of a write's target is kept.
    let written: HashSet<PathBuf> = (plan.writes.iter())
        .map(|write| lowered(&write.target))
        .collect();
    // What the backup holds is never deleted, whatever ticks or overrides did to the writes.
    let held: HashSet<(Base, String)> = (listed.staged.files.iter())
        .map(|(base, tail)| held_key(*base, tail))
        .collect();
    let mut files = BTreeMap::new();
    for placed in &plan.placed {
        if whole_base(&placed.root) || whole_base(&placed.target_root) {
            continue;
        }
        let (target_base, ctx) = (placed.target.0, &placed.ctx);
        let Ok(dir) = resolve_path(target_base, &placed.target.1, ctx, vars) else {
            continue;
        };
        let mut below = Vec::new();
        if let Err(source) = list_files(&dir, "", &mut below) {
            found.unlisted.push(UnlistedDir { path: dir, source });
            continue;
        }
        for (rest, path) in below {
            let (base, tail) = (placed.staged.0, format!("{}/{rest}", placed.staged.1));
            let roots = &listed.staged.meta.roots;
            // A file of a deeper root belongs to that root, placed or not.
            let Some(covering) = covering_root(roots, base, &tail, vars)
                .filter(|covering| *covering.root == placed.root)
            else {
                continue;
            };
            if !file_declared(covering.root, &tail, &declared, vars) {
                continue;
            }
            let Some(target_tail) = covering.tail(&placed.target_root.1, vars, ctx.user()) else {
                continue;
            };
            // Only a file a backup holding it would restore onto itself, as `plan` places it.
            if resolve_path(target_base, &target_tail, ctx, vars).ok() != Some(path.clone())
                || written.contains(&lowered(&path))
                || held.contains(&held_key(base, &tail))
            {
                continue;
            }
            let (staged_key, target_key) = (key(base, &tail), key(target_base, &target_tail));
            if !(recorded.path_included(&staged_key)
                && today.path_included(&staged_key)
                && today.path_included(&target_key))
            {
                continue;
            }
            let delete = Delete {
                target: path.clone(),
                target_file: (target_base, target_tail),
                target_root: placed.target_root.clone(),
            };
            files.insert(path, delete);
        }
    }
    found.files = files.into_values().collect();
    found
}

/// Whether `root` is a whole base, which would mirror every folder of it: no component below
/// its base, or for `WinAbsolute` none below the drive, as `crossbuild::too_wide` counts.
fn whole_base((base, rest): &SnapshotRoot) -> bool {
    components(rest).count() <= usize::from(*base == Base::WinAbsolute)
}

/// `path` by its components with ASCII case folded, to compare paths a case-insensitive file
/// system may equate whatever their separators (`/` or `\` on Windows), repeated separators or
/// `.` components. A `..` stays: `run` refuses to stage an existing target that has one.
pub fn lowered(path: &Path) -> PathBuf {
    (path.components())
        .map(|part| part.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect()
}

/// A staged file's `(base, tail)` with the tail's ASCII case folded, as `written` compares.
fn held_key(base: Base, tail: &str) -> (Base, String) {
    (base, tail.to_ascii_lowercase())
}

/// The selection key of `tail` below `base`, as `selection::path_key` makes it.
fn key(base: Base, tail: &str) -> String {
    format!("{}/{tail}", base.dir_name())
}

/// Pushes each regular file below `dir` as `(its /-separated path after prefix, its path)`.
/// Symlinks and special files are left out and never followed; a missing `dir` holds none.
fn list_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        entries => entries?,
    };
    for entry in entries {
        let entry = entry?;
        // Kept: a snapshot's keys are UTF-8, so the name cannot be checked against them.
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let tail = match prefix {
            "" => name,
            _ => format!("{prefix}/{name}"),
        };
        let kind = entry.file_type()?;
        if kind.is_dir() {
            list_files(&entry.path(), &tail, out)?;
        } else if kind.is_file() {
            out.push((tail, entry.path()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
