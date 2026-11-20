//! The targets a restore starts with: the game's own prefix, the prefix its snapshot came from,
//! or the one prefix that holds its saves (ADR-0005, ADR-0017, ADR-0023).

use std::fs;
use std::path::PathBuf;

use super::{game_info, game_vars};
use crate::config::AppConfig;
use crate::crossbuild::{self, Family};
use crate::launcher::LauncherPrefix;
use crate::layout::{
    self, Base, Context, GameVars, LinuxFolders, PrefixOrigin, RootKind, WinFolders,
};
use crate::manifest::Manifest;
use crate::prefix::{proton_prefix, wine_user};
use crate::restore::{StagedGame, Targets, covering_root};
use crate::scan::{Environment, describe_prefix, launcher_prefixes};

/// Why [`DefaultTargets::targets`] holds the prefix it does (ADR-0023).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PrefixReason {
    /// The custom game's `wine_prefix`.
    CustomGame,
    /// The game's own prefix: Proton's by its Steam app id, or the launcher prefix mapped to it.
    OwnPrefix,
    /// The prefix the snapshot was backed up from.
    Backup,
    /// The one known prefix that holds the snapshot's save folders.
    Saves,
}

/// The targets a restore starts with, and why its prefix was chosen (ADR-0023).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DefaultTargets {
    /// The targets; the user may change them.
    pub targets: Targets,
    /// Why `targets.win` is the prefix it is; `None` without a prefix or on Windows.
    pub reason: Option<PrefixReason>,
    /// The chosen prefix as this machine's launchers name it; `None` when it has no such name.
    pub origin: Option<PrefixOrigin>,
    /// The several prefixes that all hold the snapshot's save folders, among those its prefix
    /// name fits or in the search, so none was chosen.
    pub ambiguous: Vec<PathBuf>,
    /// The several prefixes the snapshot's prefix name fits here when none of them holds its save
    /// folders and the search found none either, so none was chosen.
    pub stored_several: Vec<PathBuf>,
    /// Whether the snapshot names a prefix that this machine does not have, ignores, or cannot
    /// use since its Wine user cannot be read.
    pub stored_missing: bool,
}

/// The targets a restore of `title` starts with (ADR-0005, ADR-0023). On Windows the native
/// folders. On Linux the native folders plus the first prefix of: the custom game's
/// `wine_prefix`; the game's own prefix (Proton under `env.steam_roots` or a configured Steam
/// root, else its launcher prefix); the prefix `staged`'s snapshot names, as this machine's
/// launchers know it (of several, the one that holds its saves); else, for a snapshot of the
/// Windows build, the one known prefix that holds its save folders. Launcher prefixes in
/// `config.ignored_prefixes` are passed over (ADR-0017), as is a prefix whose Wine user cannot
/// be determined. `staged: None` (no snapshot chosen, or
/// its read failed) looks for neither the snapshot's prefix nor its saves.
pub fn default_targets(
    title: &str,
    manifest: &Manifest,
    config: &AppConfig,
    env: &Environment,
    staged: Option<&StagedGame>,
) -> DefaultTargets {
    let mut defaults = DefaultTargets {
        targets: Targets {
            linux: None,
            win: None,
        },
        reason: None,
        origin: None,
        ambiguous: Vec::new(),
        stored_several: Vec::new(),
        stored_missing: false,
    };
    let linux = match &env.native {
        Some(win @ Context::WindowsNative(_)) => {
            defaults.targets.win = Some(win.clone());
            return defaults;
        }
        Some(Context::Linux(folders)) => Some(folders),
        _ => None,
    };
    defaults.targets.linux = linux.cloned();
    let configured = (config.roots.iter())
        .filter(|root| root.kind == RootKind::Steam)
        .map(|root| &root.path);
    let steam_roots: Vec<&PathBuf> = env.steam_roots.iter().chain(configured).collect();
    let info = game_info(title, manifest, &config.custom_games);
    let steam_id = (info.entry.as_ref())
        .and_then(|entry| entry.steam.as_ref()?.id)
        .map(u64::from);
    let custom_prefix = (config.custom_games.iter())
        .filter(|game| game.name == title)
        .filter_map(|game| game.wine_prefix.clone())
        .next_back();
    let (mut own, shared) = launcher_prefixes(manifest, config, env);
    let own = own.remove(title).unwrap_or_default();
    let proton = (steam_roots.iter()).filter_map(|root| proton_prefix(root, steam_id?));
    let own_paths = own.iter().map(|prefix| prefix.path.clone());
    let mut chosen = (custom_prefix.and_then(in_prefix))
        .map(|ctx| (ctx, PrefixReason::CustomGame))
        .or_else(|| {
            let ctx = proton.chain(own_paths).find_map(in_prefix)?;
            Some((ctx, PrefixReason::OwnPrefix))
        });
    let vars = game_vars(title, info.entry.as_ref(), config, env);
    let mut stored_several = Vec::new();
    if let Some(staged) = staged
        && let Some(origin) = &staged.meta.prefix
    {
        let stored = unique(stored_prefix(origin, config, env, &steam_roots, linux));
        let mut found: Vec<Context> = stored.into_iter().filter_map(in_prefix).collect();
        defaults.stored_missing = found.is_empty();
        if chosen.is_none() && found.len() > 1 {
            // One name for several prefixes, such as a launcher's native and Flatpak installs.
            let holders: Vec<&Context> = (found.iter())
                .filter(|ctx| holds_saves(staged, ctx, &vars))
                .collect();
            match holders.as_slice() {
                [one] => chosen = Some(((*one).clone(), PrefixReason::Backup)),
                // Named only if the search finds nothing either.
                [] => stored_several = found.iter().filter_map(prefix_path).collect(),
                many => defaults.ambiguous = many.iter().copied().filter_map(prefix_path).collect(),
            }
        } else if chosen.is_none() {
            chosen = found.pop().map(|ctx| (ctx, PrefixReason::Backup));
        }
    }
    if chosen.is_none()
        && defaults.ambiguous.is_empty()
        && let Some(staged) = staged.filter(|staged| windows_build(staged))
    {
        let mut hits: Vec<Context> = (known_prefixes(config, env, &steam_roots).into_iter())
            .filter_map(in_prefix)
            .filter(|ctx| holds_saves(staged, ctx, &vars))
            .collect();
        if hits.len() == 1 {
            chosen = hits.pop().map(|ctx| (ctx, PrefixReason::Saves));
        } else {
            defaults.ambiguous = hits.iter().filter_map(prefix_path).collect();
        }
    }
    if chosen.is_none() && defaults.ambiguous.is_empty() {
        defaults.stored_several = stored_several;
    }
    if let Some((ctx, reason)) = chosen {
        if let Context::WinePrefix { prefix, .. } = &ctx {
            defaults.origin = describe_prefix(prefix, &own, &shared, env);
        }
        defaults.reason = Some(reason);
        defaults.targets.win = Some(ctx);
    }
    defaults
}

/// The prefix at `prefix` as a target; `None` when its Wine user cannot be determined.
fn in_prefix(prefix: PathBuf) -> Option<Context> {
    let user = wine_user(&prefix).ok()?;
    let folders = WinFolders::for_prefix(&prefix, &user);
    Some(Context::WinePrefix { prefix, folders })
}

/// The prefix of a `WinePrefix` context.
fn prefix_path(ctx: &Context) -> Option<PathBuf> {
    match ctx {
        Context::WinePrefix { prefix, .. } => Some(prefix.clone()),
        _ => None,
    }
}

/// `prefixes` without a second path to the same folder: a Steam root can be a link to another
/// one, and a bottle's placeholder can lead where another install's bottle is.
fn unique(prefixes: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = Vec::new();
    let mut unique = Vec::new();
    for prefix in prefixes {
        let real = fs::canonicalize(&prefix).unwrap_or_else(|_| prefix.clone());
        if !seen.contains(&real) {
            seen.push(real);
            unique.push(prefix);
        }
    }
    unique
}

/// The launcher prefixes of `env` that `config` does not ignore, native installs first.
fn launchers<'a>(
    config: &'a AppConfig,
    env: &'a Environment,
) -> impl Iterator<Item = &'a LauncherPrefix> {
    (env.launcher_prefixes.iter()).filter(|prefix| !config.ignored_prefixes.contains(&prefix.path))
}

/// Where this machine has the prefix `origin` names, best first. `origin` comes from a snapshot,
/// which is untrusted (ADR-0005): a name is only compared with the launchers' own names, and a
/// `Home` rest must be plain components below `linux`'s home.
fn stored_prefix(
    origin: &PrefixOrigin,
    config: &AppConfig,
    env: &Environment,
    steam_roots: &[&PathBuf],
    linux: Option<&LinuxFolders>,
) -> Vec<PathBuf> {
    match origin {
        PrefixOrigin::Proton(id) => (steam_roots.iter())
            .filter_map(|root| proton_prefix(root, u64::from(*id)))
            .collect(),
        PrefixOrigin::Heroic(_) | PrefixOrigin::Lutris(_) | PrefixOrigin::Bottles(_) => {
            (launchers(config, env))
                .filter(|prefix| prefix.origin().as_ref() == Some(origin))
                .map(|prefix| prefix.path.clone())
                .collect()
        }
        PrefixOrigin::Home(_) => (linux.and_then(|folders| origin.home_path(&folders.home)))
            .into_iter()
            .collect(),
    }
}

/// Whether `base` is a Windows folder a prefix holds; `WinAbsolute` restores on Windows only.
fn in_windows_folders(base: Base) -> bool {
    base != Base::WinAbsolute && crossbuild::base_family(base) == Some(Family::Windows)
}

/// Whether `staged` is a snapshot of the game's Windows build: it has a root in the Windows
/// folders. A registry-only snapshot is not searched for, since that would read every prefix's
/// `user.reg` (ADR-0023).
fn windows_build(staged: &StagedGame) -> bool {
    (staged.meta.roots.iter()).any(|(base, _)| in_windows_folders(*base))
}

/// Every prefix this machine knows, without duplicates: the custom games' `wine_prefix`, every
/// Proton prefix of every Steam root, the launcher prefixes not ignored, then the `WinePrefix`
/// roots.
fn known_prefixes(config: &AppConfig, env: &Environment, steam_roots: &[&PathBuf]) -> Vec<PathBuf> {
    let custom = (config.custom_games.iter()).filter_map(|game| game.wine_prefix.clone());
    let proton = steam_roots.iter().flat_map(|root| {
        let mut ids: Vec<PathBuf> = fs::read_dir(root.join("steamapps/compatdata"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path().join("pfx"))
            .collect();
        ids.sort();
        ids
    });
    let launcher = launchers(config, env).map(|prefix| prefix.path.clone());
    let roots = (config.roots.iter())
        .filter(|root| root.kind == RootKind::WinePrefix)
        .map(|root| root.path.clone());
    unique(custom.chain(proton).chain(launcher).chain(roots))
}

/// Most folders [`holds_saves`] looks at per prefix, so a snapshot of many folders stays cheap.
const MAX_FOLDERS: usize = 32;

/// Whether the prefix `ctx` holds `staged`'s saves: the folder of one of its files in the Windows
/// folders, resolved in it as a restore resolves the file (through its root, ASCII case ignored,
/// `<osUserName>` as the prefix's user), is a folder with an entry. The files' folders are
/// checked, not the roots: a generic root such as `My Games` holds other games' folders too.
/// Files directly in a base folder are passed over, and at most [`MAX_FOLDERS`] are checked.
fn holds_saves(staged: &StagedGame, ctx: &Context, vars: &GameVars) -> bool {
    let mut folders: Vec<(Base, String)> = Vec::new();
    for (base, tail) in &staged.files {
        if !in_windows_folders(*base) {
            continue;
        }
        let Some(covering) = covering_root(&staged.meta.roots, *base, tail, vars) else {
            continue;
        };
        let target = covering.tail(&covering.root.1, vars, ctx.user());
        let Some((folder, _)) = target.as_deref().and_then(|target| target.rsplit_once('/')) else {
            continue;
        };
        let folder = (*base, folder.to_owned());
        if !folders.contains(&folder) {
            folders.push(folder);
            if folders.len() == MAX_FOLDERS {
                break;
            }
        }
    }
    folders.iter().any(|(base, folder)| {
        (layout::base_dir(*base, ctx, vars))
            .and_then(|dir| layout::target_path(&dir, folder, true))
            .is_some_and(|path| {
                fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some())
            })
    })
}
