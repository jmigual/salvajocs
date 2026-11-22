//! Restore planning: maps every file of a staged game to a target path on this machine, or to a
//! skip reason, through a cross-build mapping where a base family has no target (ADR-0005,
//! ADR-0011). Nothing here writes to disk.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use crate::crossbuild::{
    self, Choice, Confidence, Family, MappingEntry, ResolveError, ResolvedMapping, SnapshotRoot,
};
use crate::layout::{self, Base, Context, GameVars, LinuxFolders, SnapshotMeta};
use crate::manifest::{GameEntry, Pair, RegistryRule};
use crate::registry::RegistryExport;

mod run;

pub(crate) use run::parse_base;
pub use run::{
    ApplyError, DefaultTargets, FetchError, GameBackups, GameInfo, Listed, PRE_RESTORE_TAG,
    PrefixReason, RestoreReport, RunError, SafetyBackup, UnreadableTarget, WriteState,
    compare_writes, default_targets, describe_prefix, download, fetch, game_info, game_vars, list,
    read, run, staged_target,
};

/// A snapshot of one game restored into a work dir (ADR-0004).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StagedGame {
    /// The work dir holding `files/`, or the one [`download`] fills.
    pub root: PathBuf,
    /// Its `salvajocs.json`.
    pub meta: SnapshotMeta,
    /// Every staged file as `(base, tail)`, from `files/<Base>/<tail>`. Untrusted snapshot data.
    pub files: Vec<(Base, String)>,
    /// Its `registry.json`, if any.
    pub registry: Option<RegistryExport>,
}

impl StagedGame {
    /// A staged game from its fields.
    pub fn new(
        root: PathBuf,
        meta: SnapshotMeta,
        files: Vec<(Base, String)>,
        registry: Option<RegistryExport>,
    ) -> Self {
        Self {
            root,
            meta,
            files,
            registry,
        }
    }
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
#[non_exhaustive]
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

impl Write {
    /// A write from its fields.
    pub fn new(
        source: PathBuf,
        target: PathBuf,
        target_file: Option<(Base, String)>,
        target_root: Option<SnapshotRoot>,
    ) -> Self {
        Self {
            source,
            target,
            target_file,
            target_root,
        }
    }
}

/// Why a staged item is not restored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
    /// The file's root is not one the game declares.
    UndeclaredRoot,
    /// `registry.json` with no Windows target.
    NoWindowsTarget,
    /// A registry key outside the keys the game's entry declares.
    UndeclaredRegistryKey,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SkipReason::NoTarget => "no target on this machine",
            SkipReason::NotInstalled => "game not found under any root",
            SkipReason::SkippedByUser => "skipped by choice",
            SkipReason::UnsafePath => "path leaves its base directory",
            SkipReason::OutsideRoots => "outside the game's save roots",
            SkipReason::UndeclaredRoot => "save root not declared for this game",
            SkipReason::NoWindowsTarget => "registry: no Windows target",
            SkipReason::UndeclaredRegistryKey => "registry key not declared for this game",
        })
    }
}

/// What a [`Skipped`] refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkippedItem {
    /// A staged file as `(base, tail)`.
    File(Base, String),
    /// The whole `registry.json`.
    Registry,
    /// One key of `registry.json`.
    RegistryKey(String),
}

/// A staged item that is not restored.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Skipped {
    /// The skipped item.
    pub item: SkippedItem,
    /// Why it is skipped.
    pub reason: SkipReason,
}

/// Everything a restore would do, shown to the user before anything is written (ADR-0005).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestorePlan {
    /// Files to copy.
    pub writes: Vec<Write>,
    /// The registry export and the Windows target it goes into.
    pub registry: Option<(RegistryExport, Context)>,
    /// Keys of the Windows target the registry restore leaves as they are, as
    /// `registry::windows::replace_subtrees` and [`RegFile::replace_subtrees`] take them
    /// (ADR-0026). Empty from [`Self::new`] and [`plan`].
    ///
    /// [`RegFile::replace_subtrees`]: crate::registry::wine::RegFile::replace_subtrees
    pub registry_kept: Vec<String>,
    /// Items not restored, with the reason.
    pub skipped: Vec<Skipped>,
    /// Whether a write goes through a cross-build mapping, so the other build's save format
    /// may differ (ADR-0011).
    pub cross_build: bool,
    /// Facts about the mapping the preview shows, such as the Unity `PlayerPrefs` note; filled
    /// by [`prepare`].
    pub notes: Vec<String>,
}

/// Why [`plan`] failed.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
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
#[non_exhaustive]
pub enum OverrideError {
    /// The new target is not an absolute path.
    #[error("the override target is not an absolute path")]
    RelativePath,
    /// The plan has no write at this index.
    #[error("no write at index {0}")]
    NoSuchWrite(usize),
}

impl RestorePlan {
    /// A plan from its fields.
    pub fn new(
        writes: Vec<Write>,
        registry: Option<(RegistryExport, Context)>,
        skipped: Vec<Skipped>,
        cross_build: bool,
        notes: Vec<String>,
    ) -> Self {
        Self {
            writes,
            registry,
            registry_kept: Vec::new(),
            skipped,
            cross_build,
            notes,
        }
    }

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

/// The targets a [`RestorePlan`] writes into, so the preview names only those.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Used {
    /// Some write goes into a native Linux base (`Home`, `XdgData`, `XdgConfig`, `Absolute`).
    pub linux: bool,
    /// The Windows target that some write of a Windows base, or the registry, goes into.
    pub win: Option<Context>,
    /// Some write goes into the game's install folder (`GameDir`, `StoreRoot`).
    pub install_dir: bool,
}

/// Which of `targets` the writes and registry of `plan` use, by each write's target-side base.
/// An overridden write counts for no target.
pub fn used_targets(plan: &RestorePlan, targets: &Targets) -> Used {
    let bases = || {
        (plan.writes.iter())
            .filter_map(|write| write.target_file.as_ref())
            .map(|(base, _)| *base)
    };
    let family = |family| bases().any(|base| crossbuild::base_family(base) == Some(family));
    let win = family(Family::Windows) || plan.registry.is_some();
    Used {
        linux: family(Family::Linux),
        win: targets.win.clone().filter(|_| win),
        install_dir: bases().any(|base| crossbuild::base_family(base).is_none()),
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
            layout::is_plain_component(got)
                && got.len() > pre.len() + post.len()
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
        // On equal depth a literal component beats `<osUserName>`; `min_by_key` then keeps the
        // first of equals, so the first `<game>` candidate wins a tie.
        .min_by_key(|(depth, covering)| {
            let users = components(&covering.root.1)
                .filter(|part| part.contains("<osUserName>"))
                .count();
            (std::cmp::Reverse(*depth), users)
        })
        .map(|(_, covering)| covering)
}

/// The root of `roots` that the staged file `(base, tail)` lies under, as [`plan`] finds it.
pub(crate) fn root_of<'a>(
    roots: &'a [SnapshotRoot],
    base: Base,
    tail: &str,
    vars: &'a GameVars,
) -> Option<&'a SnapshotRoot> {
    covering_root(roots, base, tail, vars).map(|covering| covering.root)
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

/// A root prefix the game declares.
struct Declared {
    prefix: SnapshotRoot,
    /// For a prefix from a globbed template (`<base>/*.sav`): the template's rest, which a
    /// file's tail must match, since the prefix alone would trust its whole dir.
    glob: Option<String>,
}

/// The roots a snapshot of this game may hold (ADR-0005): the classified glob-free prefix of each
/// file template of `entry` (in either family), each side of `pairs`, and the engine-rule
/// targets of those.
fn declared_roots(entry: Option<&GameEntry>, pairs: &[Pair]) -> Vec<Declared> {
    let mut declared = Vec::new();
    for family in [Family::Linux, Family::Windows] {
        let templates = entry.into_iter().flat_map(|entry| entry.files.keys());
        for template in templates {
            let Some(prefix) = crossbuild::classify(layout::glob_free_prefix(template), family)
            else {
                continue;
            };
            // A literal template's full path is its prefix; a globbed one's prefix alone would
            // trust a whole shallow dir (`<winDir>/System32`).
            let glob = if layout::glob_free_prefix(template) == template.as_str() {
                None
            } else {
                // A template that does not classify whole declares nothing.
                let Some((_, rest)) = crossbuild::classify(template, family) else {
                    continue;
                };
                Some(rest)
            };
            declared.push(Declared { prefix, glob });
        }
        let sides = pairs.iter().map(|pair| match family {
            Family::Linux => pair.linux.as_str(),
            Family::Windows => pair.windows.as_str(),
        });
        declared.extend(
            sides
                .filter_map(|side| crossbuild::classify(side, family))
                .map(|prefix| Declared { prefix, glob: None }),
        );
    }
    let meta = SnapshotMeta {
        format: layout::SNAPSHOT_FORMAT,
        title: String::new(),
        roots: declared.iter().map(|d| d.prefix.clone()).collect(),
        prefix: None,
    };
    for family in [Family::Linux, Family::Windows] {
        let engine = crossbuild::propose(&meta, None, &[], &[], family)
            .into_iter()
            .filter(|mapping| mapping.confidence == Confidence::EngineRule)
            .filter_map(|mapping| crossbuild::classify(&mapping.proposed?, family))
            .map(|prefix| Declared { prefix, glob: None });
        declared.extend(engine);
    }
    declared
}

/// The entries of `declared` that `root` equals or lies below (ASCII case ignored for Windows
/// bases), so a pre-restore snapshot of a mapped root deeper than its pair side stays
/// restorable; none for a root with a component that is not plain (`..`). A `WinHome` root
/// under `AppData/<Roaming|Local|LocalLow>` in any case also counts as the base a template of
/// today classifies to, as snapshots made before that matched without case hold it.
fn declaring<'a>(
    root: &'a SnapshotRoot,
    declared: &'a [Declared],
) -> impl Iterator<Item = &'a Declared> {
    let windows = crossbuild::base_family(root.0) == Some(Family::Windows);
    let plain = components(&root.1).all(layout::is_plain_component);
    let normalized = layout::normalized_win_home(root.0, &root.1);
    declared.iter().filter(move |d| {
        plain
            && [Some(root), normalized.as_ref()]
                .into_iter()
                .flatten()
                .any(|root| d.prefix.0 == root.0 && below_prefix(root, &d.prefix.1, windows))
    })
}

/// Whether `root` equals or lies below the directory `prefix` of its own base.
fn below_prefix(root: &SnapshotRoot, prefix: &str, windows: bool) -> bool {
    let mut parts = components(&root.1);
    components(prefix).all(|want| {
        parts.next().is_some_and(|got| {
            if windows {
                got.eq_ignore_ascii_case(want)
            } else {
                got == want
            }
        })
    })
}

/// Whether the file `tail` under `root` is declared: `root` lies at or below a declared prefix
/// and, for a prefix from a globbed template, the tail or one of its parent dirs
/// (a matched dir's files are saved too) matches that template.
fn file_declared(root: &SnapshotRoot, tail: &str, declared: &[Declared], vars: &GameVars) -> bool {
    // Prefix scans and native Windows ignore case; only native Linux bases keep it.
    let options = glob::MatchOptions {
        case_sensitive: crossbuild::base_family(root.0) == Some(Family::Linux),
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    declaring(root, declared).any(|d| {
        let Some(glob) = &d.glob else {
            return true;
        };
        // A glob of a normalized base is relative to that base, so the tail is too.
        let normalized = (d.prefix.0 != root.0)
            .then(|| layout::normalized_win_home(root.0, tail))
            .flatten();
        let parts: Vec<&str> =
            components(normalized.as_ref().map_or(tail, |(_, tail)| tail)).collect();
        // The source machine's user is unknown here, so `<osUserName>` matches any component.
        layout::substitute(&glob.replace("<osUserName>", "*"), vars, "")
            .iter()
            .filter_map(|pattern| glob::Pattern::new(pattern).ok())
            .any(|pattern| {
                (1..=parts.len()).any(|n| pattern.matches_with(&parts[..n].join("/"), options))
            })
    })
}

/// Whether the registry `key` lies at or below a key `entry` declares, ignoring ASCII case, with
/// `\` and `/` both separating; a snapshot is untrusted, so nothing else is restored (ADR-0005).
fn declared(entry: Option<&GameEntry>, key: &str) -> bool {
    let normalize = |key: &str| key.replace('\\', "/").to_ascii_lowercase();
    let key = normalize(key);
    entry.is_some_and(|entry| {
        entry.registry.keys().any(|declared| {
            let declared = normalize(declared);
            let declared = declared.trim_end_matches('/');
            key == declared || key.starts_with(&format!("{declared}/"))
        })
    })
}

/// Maps every staged file to a target path or a skip reason. A base resolves against its own
/// family's target; a file whose family has none goes through the `mappings` entry of its root
/// (ADR-0011). `Absolute`/`WinAbsolute` restore to the same OS only (ADR-0005). A file must lie
/// under a recorded root. Its tail below the root is kept, and the root's raw rest (or the
/// mapping target's) is substituted with `vars` and the target context's user, so
/// `<osUserName>` becomes this machine's user. `GameDir`/`StoreRoot` resolve through the
/// install dirs in `vars`. A snapshot is untrusted (ADR-0005): only files under a root the
/// game declares (its `entry` templates, its saved `pairs` and their engine-rule targets) and
/// only the registry keys `entry` declares are restored. `entry` is the game's manifest entry
/// with its custom game applied (`manifest::merge_custom`).
///
/// # Errors
/// [`PlanError::Unmapped`] when a file of a family without a target lies under a root that has
/// no mapping while the other family has a target.
pub fn plan(
    staged: &StagedGame,
    targets: &Targets,
    vars: &GameVars,
    entry: Option<&GameEntry>,
    pairs: &[Pair],
    mappings: &[ResolvedMapping],
) -> Result<RestorePlan, PlanError> {
    let declared_roots = declared_roots(entry, pairs);
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
        registry_kept: Vec::new(),
        skipped: Vec::new(),
        cross_build: false,
        notes: Vec::new(),
    };
    let files_dir = staged.root.join(layout::FILES_DIR);
    for (base, tail) in &staged.files {
        let family = crossbuild::base_family(*base);
        let covering = covering_root(&staged.meta.roots, *base, tail, vars);
        let mapped = family.is_some() && target(family).is_none();
        let placed = match covering {
            None => Err(SkipReason::OutsideRoots),
            Some(covering) if !file_declared(covering.root, tail, &declared_roots, vars) => {
                Err(SkipReason::UndeclaredRoot)
            }
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
                    // A custom absolute path never is the other OS's location (ADR-0005).
                    Some((Base::Absolute | Base::WinAbsolute, _)) => Err(SkipReason::NoTarget),
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
                item: SkippedItem::File(*base, tail.clone()),
                reason,
            }),
        }
    }
    match (&staged.registry, &targets.win) {
        (Some(export), Some(win)) => {
            let (keys, undeclared): (BTreeMap<_, _>, BTreeMap<_, _>) = (export.keys.clone())
                .into_iter()
                .partition(|(key, _)| declared(entry, key));
            plan.skipped
                .extend(undeclared.into_keys().map(|key| Skipped {
                    item: SkippedItem::RegistryKey(key),
                    reason: SkipReason::UndeclaredRegistryKey,
                }));
            if !keys.is_empty() {
                plan.registry = Some((RegistryExport { keys }, win.clone()));
            }
        }
        (Some(_), None) => plan.skipped.push(Skipped {
            item: SkippedItem::Registry,
            reason: SkipReason::NoWindowsTarget,
        }),
        (None, _) => {}
    }
    Ok(plan)
}

/// Why [`prepare`] returned no plan.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
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
/// pairs, row pairs, engine rules and auto-pairs apply without choices (ADR-0011, ADR-0016).
/// `info` is what the manifest and the custom game say about the game ([`game_info`]); its row
/// pairs are proposed before its saved pairs, so a saved pair wins a tie, and declare no roots.
///
/// # Errors
/// [`PrepareError::NeedsMapping`] while an unknown root has no choice;
/// [`PrepareError::Unclassifiable`] for a chosen template that is not a placeholder template of
/// the target OS; [`PrepareError::Plan`] when [`plan`] fails.
pub fn prepare(
    staged: &StagedGame,
    targets: &Targets,
    vars: &GameVars,
    info: &GameInfo,
    choices: &[(SnapshotRoot, Choice)],
) -> Result<RestorePlan, PrepareError> {
    let (entry, pairs) = (info.entry.as_ref(), &info.pairs);
    let target_family = match (&targets.linux, &targets.win) {
        (Some(_), None) => Some(Family::Linux),
        (None, Some(_)) => Some(Family::Windows),
        _ => None,
    };
    let mut mappings = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    if let Some(family) = target_family {
        // Undeclared roots are skipped by `plan`, so they are not offered for mapping either.
        let declared = declared_roots(entry, pairs);
        let meta = SnapshotMeta {
            roots: (staged.meta.roots.iter())
                .filter(|root| declaring(root, &declared).next().is_some())
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
        let proposing: Vec<Pair> = info.row_pairs.iter().chain(pairs).cloned().collect();
        let mut entries = crossbuild::propose(&meta, Some(&entry), &proposing, &info.skips, family);
        for mapping in &mut entries {
            mapping.files = (staged.files.iter())
                .filter(|(base, tail)| {
                    covering_root(&staged.meta.roots, *base, tail, vars)
                        .is_some_and(|covering| *covering.root == mapping.root)
                })
                .count();
        }
        // Notes reach the preview whether the mapping was automatic or chosen (ADR-0011).
        for note in entries.iter().flat_map(|mapping| &mapping.notes) {
            if !notes.contains(note) {
                notes.push(note.clone());
            }
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
    let mut plan = plan(staged, targets, vars, entry, pairs, &mappings)?;
    plan.notes = notes;
    Ok(plan)
}

#[cfg(test)]
mod tests;
