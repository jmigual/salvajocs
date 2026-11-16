//! Restore planning: maps every file of a staged game to a target path on this machine, or to a
//! skip reason, through a cross-build mapping where a base family has no target (ADR-0005,
//! ADR-0011). Nothing here writes to disk.

use std::fmt;
use std::path::PathBuf;

use crate::crossbuild::{
    self, Choice, Family, MappingEntry, ResolveError, ResolvedMapping, SnapshotRoot,
};
use crate::layout::{self, Base, Context, GameVars, LinuxFolders, Root, SnapshotMeta};
use crate::manifest::{GameEntry, Pair, RegistryRule};
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
    /// `None` once overridden.
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
    /// The file lies under none of the snapshot's recorded roots.
    OutsideRoots,
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
            SkipReason::OutsideRoots => "outside the game's save roots",
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

/// `rest` with each `(placeholder, value)` replaced literally; `None` when it holds a
/// placeholder whose value is missing or empty.
fn substitute(rest: &str, values: &[(&str, Option<&str>)]) -> Option<String> {
    let mut rest = rest.to_owned();
    for &(placeholder, value) in values {
        if rest.contains(placeholder) {
            rest = rest.replace(placeholder, value.filter(|value| !value.is_empty())?);
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

impl Covering<'_> {
    /// The file's tail under `rest`, a raw root or mapping target, for a target whose user is
    /// `user`; `None` when `rest` needs a value `vars` lacks.
    fn tail(&self, rest: &str, vars: &GameVars, user: &str) -> Option<String> {
        let rest = substitute(
            rest,
            &[
                ("<game>", self.game),
                ("<storeGameId>", vars.store_game_id.as_deref()),
                ("<osUserName>", Some(user)),
            ],
        )?;
        let parts: Vec<&str> = components(&rest).chain(components(&self.below)).collect();
        Some(parts.join("/"))
    }
}

/// Whether the disk component `got` matches the root component `want`. `<osUserName>` stands
/// for the source machine's user, so it matches any non-empty text within the component.
fn component_matches(want: &str, got: &str, windows: bool) -> bool {
    let same = |a: &str, b: &str| {
        if windows {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    match want.split_once("<osUserName>") {
        Some((pre, post)) => {
            got.len() > pre.len() + post.len()
                && got.get(..pre.len()).is_some_and(|got| same(got, pre))
                && got
                    .get(got.len() - post.len()..)
                    .is_some_and(|got| same(got, post))
        }
        None => same(want, got),
    }
}

/// The deepest of `roots` that `(base, tail)` lies under, once `<game>` and `<storeGameId>` are
/// substituted into the root. Windows roots compare ASCII case-insensitively, as a prefix scan
/// does (ADR-0011).
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
            let values = [
                ("<game>", game),
                ("<storeGameId>", vars.store_game_id.as_deref()),
            ];
            let rest = substitute(&root.1, &values)?;
            let mut parts = components(tail);
            let mut depth = 0;
            for want in components(&rest) {
                if !component_matches(want, parts.next()?, windows) {
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
    ctx: &Context,
    vars: &GameVars,
) -> Result<PathBuf, SkipReason> {
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
/// (ADR-0011). `Absolute`/`WinAbsolute` restore to the same OS only (ADR-0005). A file must lie
/// under a recorded root. Its tail below the root is kept, and the root's raw rest (or the
/// mapping target's) is substituted with `vars` and the target context's user, so
/// `<osUserName>` becomes this machine's user. `_roots` is unused: `vars` already holds the
/// install dirs found through them.
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
        let placed = match covering {
            None => Err(SkipReason::OutsideRoots),
            Some(covering) if !mapped => Ok((covering.root, covering)),
            Some(_)
                if matches!(base, Base::Absolute | Base::WinAbsolute)
                    || target(family.map(Family::other)).is_none() =>
            {
                Err(SkipReason::NoTarget)
            }
            Some(covering) => {
                let mapping = mappings
                    .iter()
                    .find(|mapping| mapping.root == *covering.root)
                    .ok_or_else(|| PlanError::Unmapped {
                        root: covering.root.clone(),
                    })?;
                match &mapping.target {
                    None => Err(SkipReason::SkippedByUser),
                    Some(target_root) => Ok((target_root, covering)),
                }
            }
        };
        let write = placed.and_then(|((target_base, target_rest), covering)| {
            let ctx = target(crossbuild::base_family(*target_base)).ok_or(SkipReason::NoTarget)?;
            let target_tail = covering
                .tail(target_rest, vars, ctx.user())
                .ok_or(SkipReason::NoTarget)?;
            let target = resolve_path(*target_base, &target_tail, ctx, vars)?;
            let source = layout::target_path(&files_dir.join(base.dir_name()), tail, false)
                .ok_or(SkipReason::UnsafePath)?;
            Ok(Write {
                source,
                target,
                target_file: Some((*target_base, target_tail)),
                target_root: Some((*target_base, target_rest.clone())),
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

/// Why [`prepare`] returned no plan.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum PrepareError {
    /// An unknown root has no choice. Holds every mapped root of the game with its file count,
    /// the known ones prefilled, for the mapping dialog (ADR-0011).
    #[error("the cross-build restore needs a choice for an unmapped root")]
    NeedsMapping(Vec<MappingEntry>),
    /// A chosen template is not a glob-free placeholder template of the target OS.
    #[error("not a placeholder template of the target OS: {template}")]
    Unclassifiable {
        /// The root the template was meant for.
        root: SnapshotRoot,
        /// The rejected template.
        template: String,
    },
    /// Planning failed.
    #[error(transparent)]
    Plan(#[from] PlanError),
}

/// Plans a restore, mapping the roots of a base family without a target onto the other family:
/// [`crossbuild::propose`], then [`crossbuild::resolve`] with `choices`, then [`plan`]. Saved
/// pairs, engine rules and auto-pairs apply without choices (ADR-0011). `entry` is the game's
/// manifest entry and `pairs` its saved pairs.
///
/// # Errors
/// [`PrepareError::NeedsMapping`] while an unknown root has no choice;
/// [`PrepareError::Unclassifiable`] for a chosen template that is not a placeholder template of
/// the target OS; [`PrepareError::Plan`] when [`plan`] fails.
pub fn prepare(
    staged: &StagedGame,
    targets: &Targets,
    roots: &[Root],
    vars: &GameVars,
    entry: Option<&GameEntry>,
    pairs: &[Pair],
    choices: &[(SnapshotRoot, Choice)],
) -> Result<RestorePlan, PrepareError> {
    let target_family = match (&targets.linux, &targets.win) {
        (Some(_), None) => Some(Family::Linux),
        (None, Some(_)) => Some(Family::Windows),
        _ => None,
    };
    let mut mappings = Vec::new();
    if let Some(family) = target_family {
        // Custom absolute paths restore to the same OS only (ADR-0005), so they are not mapped.
        let meta = SnapshotMeta {
            roots: (staged.meta.roots.iter())
                .filter(|root| !matches!(root.0, Base::Absolute | Base::WinAbsolute))
                .cloned()
                .collect(),
            ..staged.meta.clone()
        };
        // Staged registry keys count like manifest keys for the PlayerPrefs note.
        let mut entry = entry.cloned().unwrap_or_default();
        if let Some(export) = &staged.registry {
            let keys = export.keys.keys();
            (entry.registry).extend(keys.map(|key| (key.clone(), RegistryRule::default())));
        }
        let mut entries = crossbuild::propose(&meta, Some(&entry), pairs, family);
        for mapping in &mut entries {
            mapping.files = (staged.files.iter())
                .filter(|(base, tail)| {
                    covering_root(&staged.meta.roots, *base, tail, vars)
                        .is_some_and(|covering| *covering.root == mapping.root)
                })
                .count();
        }
        mappings = match crossbuild::resolve(&entries, choices) {
            Ok(mappings) => mappings,
            Err(ResolveError::MissingChoice { .. }) => {
                return Err(PrepareError::NeedsMapping(entries));
            }
            Err(ResolveError::Unclassifiable { root, template }) => {
                return Err(PrepareError::Unclassifiable { root, template });
            }
        };
    }
    Ok(plan(staged, targets, roots, vars, &mappings)?)
}

#[cfg(test)]
mod tests;
