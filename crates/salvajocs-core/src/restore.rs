//! Restore planning: maps every file of a staged game to a target path on this machine, or to a
//! skip reason, through a cross-build mapping where a base family has no target (ADR-0005,
//! ADR-0011). Nothing here writes to disk.

use std::fmt;
use std::path::PathBuf;

use crate::crossbuild::{self, Family, ResolvedMapping, SnapshotRoot};
use crate::layout::{self, Base, Context, GameVars, LinuxFolders, Root, SnapshotMeta};
use crate::registry::RegistryExport;

/// A snapshot of one game restored into a work dir (ADR-0004).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedGame {
    /// The staged game dir, holding `files/`.
    pub root: PathBuf,
    /// Its `salvajocs.json`.
    pub meta: SnapshotMeta,
    /// Every staged file as `(base, tail)`, from `files/<Base>/<tail>`. Untrusted snapshot data.
    pub files: Vec<(Base, String)>,
    /// Its `registry.json`, if any.
    pub registry: Option<RegistryExport>,
}

/// Where each base family restores to (ADR-0005). A family without a target is mapped onto the
/// other family (ADR-0011).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Targets {
    /// The native Linux user, for `Home`, `XdgData` and `XdgConfig`.
    pub linux: Option<LinuxFolders>,
    /// A `WindowsNative` or `WinePrefix` context, for the `Win*` bases and the registry.
    pub win: Option<Context>,
}

/// One file the restore copies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Write {
    /// The staged file.
    pub source: PathBuf,
    /// Where it is written.
    pub target: PathBuf,
    /// `target` as `(base, tail)`, for the safety backup; `None` once overridden.
    pub target_file: Option<(Base, String)>,
    /// The raw (unsubstituted) root `target_file` lies under, for the safety backup's `roots`;
    /// `None` when it lies under none or once overridden.
    pub target_root: Option<SnapshotRoot>,
}

/// Why a staged item is not restored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The base has no directory on this machine and no mapping applies.
    NoTarget,
    /// `GameDir`/`StoreRoot`: the game was not found under any root.
    NotInstalled,
    /// The user chose to skip the file's root.
    SkippedByUser,
    /// The tail would leave its base directory.
    UnsafePath,
    /// `registry.json` with no Windows target.
    NoWindowsTarget,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SkipReason::NoTarget => "no target on this machine",
            SkipReason::NotInstalled => "game not found under any root",
            SkipReason::SkippedByUser => "skipped by choice",
            SkipReason::UnsafePath => "path leaves its base directory",
            SkipReason::NoWindowsTarget => "registry: no Windows target",
        })
    }
}

/// A staged item that is not restored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    /// The staged file as `(base, tail)`; `None` for `registry.json`.
    pub file: Option<(Base, String)>,
    /// Why it is skipped.
    pub reason: SkipReason,
}

/// Everything a restore would do, shown to the user before anything is written (ADR-0005).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestorePlan {
    /// Files to copy.
    pub writes: Vec<Write>,
    /// The registry export and the Windows target it goes into.
    pub registry: Option<(RegistryExport, Context)>,
    /// Items not restored, with the reason.
    pub skipped: Vec<Skipped>,
    /// Whether a write goes through a cross-build mapping, so the other build's save format
    /// may differ (ADR-0011).
    pub cross_build: bool,
}

/// Why [`plan`] failed.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    /// A root of a family without a target has no [`ResolvedMapping`].
    #[error("no mapping for a root whose OS family has no target")]
    Unmapped {
        /// The root without a mapping.
        root: SnapshotRoot,
    },
}

/// Why [`RestorePlan::override_target`] refused a path.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum OverrideError {
    /// The new target is not an absolute path.
    #[error("the override target is not an absolute path")]
    RelativePath,
    /// The plan has no write at this index.
    #[error("no write at index {0}")]
    NoSuchWrite(usize),
}

impl RestorePlan {
    /// Redirects write `index` to `target` for this restore only; it is not saved as a pair
    /// (ADR-0005). The safety backup then stages the file by its full path.
    ///
    /// # Errors
    /// [`OverrideError::RelativePath`] when `target` is not absolute;
    /// [`OverrideError::NoSuchWrite`] when `index` is out of range.
    pub fn override_target(&mut self, index: usize, target: PathBuf) -> Result<(), OverrideError> {
        if !target.is_absolute() {
            return Err(OverrideError::RelativePath);
        }
        let write = self
            .writes
            .get_mut(index)
            .ok_or(OverrideError::NoSuchWrite(index))?;
        write.target = target;
        write.target_file = None;
        write.target_root = None;
        Ok(())
    }
}

fn components(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|part| !part.is_empty())
}

/// `rest` with the literal values of `<game>`, `<osUserName>` and `<storeGameId>`; `None` when
/// it needs a value `vars` lacks.
fn substitute(rest: &str, vars: &GameVars, game: Option<&str>) -> Option<String> {
    let mut rest = rest.replace("<osUserName>", &vars.os_user_name);
    for (placeholder, value) in [
        ("<game>", game),
        ("<storeGameId>", vars.store_game_id.as_deref()),
    ] {
        if rest.contains(placeholder) {
            rest = rest.replace(placeholder, value?);
        }
    }
    Some(rest)
}

/// The root a staged file lies under.
struct Covering<'a> {
    root: &'a SnapshotRoot,
    /// The `<game>` candidate the root matched with.
    game: Option<&'a str>,
    /// The tail below the root.
    below: String,
}

/// The deepest of `roots` that `(base, tail)` lies under, once `vars` is substituted into the
/// root. Windows roots compare ASCII case-insensitively, as a prefix scan does (ADR-0011).
fn covering_root<'a>(
    roots: &'a [SnapshotRoot],
    base: Base,
    tail: &str,
    vars: &'a GameVars,
) -> Option<Covering<'a>> {
    let games: Vec<Option<&str>> = if vars.game.is_empty() {
        vec![None]
    } else {
        vars.game.iter().map(|game| Some(game.as_str())).collect()
    };
    let windows = crossbuild::base_family(base) == Some(Family::Windows);
    roots
        .iter()
        .filter(|root| root.0 == base)
        .flat_map(|root| games.iter().map(move |&game| (root, game)))
        .filter_map(|(root, game)| {
            let rest = substitute(&root.1, vars, game)?;
            let mut parts = components(tail);
            let mut depth = 0;
            for want in components(&rest) {
                let got = parts.next()?;
                let same = if windows {
                    got.eq_ignore_ascii_case(want)
                } else {
                    got == want
                };
                if !same {
                    return None;
                }
                depth += 1;
            }
            let below = parts.collect::<Vec<_>>().join("/");
            Some((depth, Covering { root, game, below }))
        })
        // `min_by_key` keeps the first of equals, so the first `<game>` candidate wins a tie.
        .min_by_key(|(depth, _)| std::cmp::Reverse(*depth))
        .map(|(_, covering)| covering)
}

/// The path `tail` restores to below `base` in `ctx`. Every path goes through `target_path` or
/// `win_absolute_target`, which refuse a tail that leaves its base.
fn resolve_path(
    base: Base,
    tail: &str,
    ctx: Option<&Context>,
    vars: &GameVars,
) -> Result<PathBuf, SkipReason> {
    let ctx = ctx.ok_or(SkipReason::NoTarget)?;
    if base == Base::WinAbsolute {
        return match ctx {
            Context::WindowsNative(_) => {
                layout::win_absolute_target(tail, ctx).ok_or(SkipReason::UnsafePath)
            }
            _ => Err(SkipReason::NoTarget),
        };
    }
    let dir = layout::base_dir(base, ctx, vars).ok_or(match base {
        Base::GameDir | Base::StoreRoot => SkipReason::NotInstalled,
        _ => SkipReason::NoTarget,
    })?;
    // Only the Windows folders live inside a prefix; install dirs and roots are host paths.
    let case_insensitive =
        ctx.case_insensitive() && crossbuild::base_family(base) == Some(Family::Windows);
    layout::target_path(&dir, tail, case_insensitive).ok_or(SkipReason::UnsafePath)
}

/// Maps every staged file to a target path or a skip reason. A base resolves against its own
/// family's target; a file whose family has none goes through the `mappings` entry of its root
/// (ADR-0011). `Absolute`/`WinAbsolute` restore to the same OS only (ADR-0005). `GameVars` is
/// substituted into the raw roots and mapping targets. `_roots` is unused: `vars` already holds
/// the install dirs found through them.
///
/// # Errors
/// [`PlanError::Unmapped`] when a file of a family without a target lies under a root that has
/// no mapping while the other family has a target.
pub fn plan(
    staged: &StagedGame,
    targets: &Targets,
    _roots: &[Root],
    vars: &GameVars,
    mappings: &[ResolvedMapping],
) -> Result<RestorePlan, PlanError> {
    let linux = targets.linux.clone().map(Context::Linux);
    let target = |family: Option<Family>| match family {
        Some(Family::Linux) => linux.as_ref(),
        Some(Family::Windows) => targets.win.as_ref(),
        // Install-relative bases resolve through `vars` in either context.
        None => targets.win.as_ref().or(linux.as_ref()),
    };
    let mut plan = RestorePlan {
        writes: Vec::new(),
        registry: None,
        skipped: Vec::new(),
        cross_build: false,
    };
    let files_dir = staged.root.join(layout::FILES_DIR);
    for (base, tail) in &staged.files {
        let family = crossbuild::base_family(*base);
        let covering = covering_root(&staged.meta.roots, *base, tail, vars);
        let mapped = family.is_some() && target(family).is_none();
        let placed = if !mapped {
            Ok(((*base, tail.clone()), covering.map(|c| c.root.clone())))
        } else if matches!(base, Base::Absolute | Base::WinAbsolute)
            || target(family.map(Family::other)).is_none()
        {
            Err(SkipReason::NoTarget)
        } else if let Some(covering) = covering {
            let mapping = mappings
                .iter()
                .find(|mapping| mapping.root == *covering.root)
                .ok_or_else(|| PlanError::Unmapped {
                    root: covering.root.clone(),
                })?;
            match &mapping.target {
                None => Err(SkipReason::SkippedByUser),
                Some((target_base, target_rest)) => substitute(target_rest, vars, covering.game)
                    .map(|rest| {
                        let tail = components(&rest)
                            .chain(components(&covering.below))
                            .collect::<Vec<_>>()
                            .join("/");
                        (
                            (*target_base, tail),
                            Some((*target_base, target_rest.clone())),
                        )
                    })
                    .ok_or(SkipReason::NoTarget),
            }
        } else {
            Err(SkipReason::NoTarget)
        };
        let write = placed.and_then(|(file, root)| {
            let ctx = target(crossbuild::base_family(file.0));
            let target = resolve_path(file.0, &file.1, ctx, vars)?;
            let source = layout::target_path(&files_dir.join(base.dir_name()), tail, false)
                .ok_or(SkipReason::UnsafePath)?;
            Ok(Write {
                source,
                target,
                target_file: Some(file),
                target_root: root,
            })
        });
        match write {
            Ok(write) => {
                plan.cross_build |= mapped;
                plan.writes.push(write);
            }
            Err(reason) => plan.skipped.push(Skipped {
                file: Some((*base, tail.clone())),
                reason,
            }),
        }
    }
    match (&staged.registry, &targets.win) {
        (Some(export), Some(win)) => plan.registry = Some((export.clone(), win.clone())),
        (Some(_), None) => plan.skipped.push(Skipped {
            file: None,
            reason: SkipReason::NoWindowsTarget,
        }),
        (None, _) => {}
    }
    Ok(plan)
}

#[cfg(test)]
mod tests;
