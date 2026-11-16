//! Finds every game's save files on this machine and lists its registry keys (ADR-0004,
//! ADR-0007, ADR-0008).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::AppConfig;
#[cfg(not(windows))]
use crate::layout::LinuxFolders;
use crate::layout::{
    Base, Context, GameVars, Root, RootKind, WinFolders, classify, expand, glob_free_prefix,
    tail_of,
};
use crate::manifest::{GameEntry, Manifest, Os, When, merge_custom};
use crate::prefix::{proton_prefix, steam_roots, validate_prefix, wine_user};
use crate::registry::HKCU;

/// What the scan knows about this machine besides the config. Plain data, so tests can build any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Environment {
    /// The native context: `WindowsNative` on Windows, `Linux` elsewhere. `None` when its
    /// folders cannot be determined.
    pub native: Option<Context>,
    /// The OS user name of the native context, for `<osUserName>`.
    pub user: String,
    /// Detected Steam roots; Proton prefixes are looked up under them.
    pub steam_roots: Vec<PathBuf>,
}

impl Environment {
    /// Detects the native context, the user and the default Steam roots of this machine.
    pub fn detect() -> Self {
        #[cfg(windows)]
        let native = WinFolders::native().map(Context::WindowsNative);
        #[cfg(not(windows))]
        let native = LinuxFolders::native().map(Context::Linux);
        let user = ["USER", "USERNAME"]
            .iter()
            .find_map(|name| std::env::var(name).ok())
            .unwrap_or_default();
        Self {
            native,
            user,
            steam_roots: steam_roots(),
        }
    }
}

/// One file a game's save template matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundFile {
    /// The semantic base the file is staged under.
    pub base: Base,
    /// The path below the base, `/`-separated, in the case it has on disk.
    pub tail: String,
    /// The file on disk.
    pub real: PathBuf,
    /// Size in bytes.
    pub size: u64,
}

/// A game with at least one save file on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameScan {
    /// The game's title.
    pub title: String,
    /// The files found, best prefix first.
    pub files: Vec<FoundFile>,
    /// The glob-free prefix of each template that matched, as `(Base, rest)`; placeholders other
    /// than the leading one stay in `rest` (ADR-0004).
    pub roots: Vec<(Base, String)>,
    /// The game's applicable `HKEY_CURRENT_USER\...` keys, not checked against any registry.
    pub registry_keys: Vec<String>,
}

/// Everything shared by all games of one scan.
struct Shared<'a> {
    env: &'a Environment,
    /// Steam roots and configured roots, deduplicated.
    roots: Vec<Root>,
    /// `WinePrefix` roots, in configured order.
    wine_roots: Vec<PathBuf>,
}

/// The context of the Wine prefix at `prefix` and its user; `None` when it is not a usable prefix.
fn prefix_context(prefix: &Path) -> Option<(Context, String)> {
    validate_prefix(prefix).ok()?;
    let user = wine_user(prefix).ok()?;
    let folders = WinFolders::for_prefix(prefix, &user);
    let prefix = prefix.to_path_buf();
    Some((Context::WinePrefix { prefix, folders }, user))
}

/// Whether a rule with these constraints applies on `os`. `when.store` never excludes: the store
/// of a root is unknown here, and a template that does not exist matches nothing.
fn applies(when: &[When], os: Os) -> bool {
    when.is_empty()
        || when
            .iter()
            .any(|w| w.os.is_none_or(|rule_os| rule_os == os))
}

/// The OS a rule's `when.os` is compared with in `ctx`.
fn context_os(ctx: &Context) -> Os {
    match ctx {
        Context::Linux(_) => Os::Linux,
        Context::WindowsNative(_) | Context::WinePrefix { .. } => Os::Windows,
    }
}

/// A manifest registry key in `HKEY_CURRENT_USER\a\b` form; `None` for other hives and the
/// bare root (ADR-0006).
fn hkcu_key(key: &str) -> Option<String> {
    let key = key.replace('/', "\\");
    let below = key.strip_prefix(HKCU)?.strip_prefix('\\')?;
    (!below.is_empty()).then_some(key)
}

/// Pushes every file at or below `path`. Symlinked directories below `path` are not followed,
/// since a prefix's `Documents` is often a link into the real home.
fn collect_files(path: &Path, out: &mut Vec<(PathBuf, u64)>) {
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.is_file() {
        out.push((path.to_path_buf(), meta.len()));
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_files(&entry.path(), out);
        } else if let Ok(meta) = fs::metadata(entry.path())
            && meta.is_file()
        {
            out.push((entry.path(), meta.len()));
        }
    }
}

/// The real files one glob pattern matches; a matched directory contributes everything below it
/// (ADR-0008). A malformed pattern or an unreadable entry matches nothing.
fn matches(pattern: &str) -> Vec<(PathBuf, u64)> {
    let mut files = Vec::new();
    if let Ok(paths) = glob::glob(pattern) {
        for path in paths.flatten() {
            collect_files(&path, &mut files);
        }
    }
    files
}

/// The contexts a game is evaluated in, best first (ADR-0007): its own prefixes (Proton, then
/// the custom `wine_prefix`), the `WinePrefix` roots, then the native context. Each carries its
/// own `GameVars`, built for that context's user.
fn contexts(
    title: &str,
    entry: &GameEntry,
    wine_prefix: Option<&PathBuf>,
    shared: &Shared,
) -> Vec<(Context, GameVars)> {
    let steam_id = entry.steam.as_ref().and_then(|steam| steam.id);
    let proton = shared
        .roots
        .iter()
        .filter(|root| root.kind == RootKind::Steam)
        .filter_map(|root| proton_prefix(&root.path, u64::from(steam_id?)));
    let mut prefixes: Vec<PathBuf> = Vec::new();
    for prefix in proton
        .chain(wine_prefix.cloned())
        .chain(shared.wine_roots.iter().cloned())
    {
        if !prefixes.contains(&prefix) {
            prefixes.push(prefix);
        }
    }
    let install_dirs: Vec<String> = entry.install_dir.keys().cloned().collect();
    let vars = |user: String| {
        GameVars::new(
            title,
            install_dirs.clone(),
            &shared.roots,
            steam_id.map(|id| id.to_string()),
            user,
        )
    };
    let mut out: Vec<_> = prefixes
        .iter()
        .filter_map(|prefix| prefix_context(prefix))
        .map(|(ctx, user)| (ctx, vars(user)))
        .collect();
    if let Some(native) = &shared.env.native {
        out.push((native.clone(), vars(shared.env.user.clone())));
    }
    out
}

/// Scans one game; `None` when none of its templates matched a file.
fn scan_game(
    title: &str,
    entry: &GameEntry,
    wine_prefix: Option<&PathBuf>,
    shared: &Shared,
) -> Option<GameScan> {
    let mut files = Vec::new();
    let mut roots: Vec<(Base, String)> = Vec::new();
    let mut seen_real = HashSet::new();
    let mut seen_logical = HashSet::new();
    for (ctx, vars) in contexts(title, entry, wine_prefix, shared) {
        let os = context_os(&ctx);
        for (template, rule) in &entry.files {
            let Some((base, _)) = classify(template, &ctx) else {
                continue;
            };
            if !applies(&rule.when, os) {
                continue;
            }
            let mut matched = false;
            for (base_dir, pattern) in expand(template, &ctx, &vars) {
                for (real, size) in matches(&pattern) {
                    let Some(tail) = tail_of(&real, &base_dir) else {
                        continue;
                    };
                    matched = true;
                    // A prefix is matched case-insensitively, so its logical paths are too.
                    let logical = if ctx.case_insensitive() {
                        tail.to_ascii_lowercase()
                    } else {
                        tail.clone()
                    };
                    if seen_real.contains(&real) || !seen_logical.insert((base, logical)) {
                        continue;
                    }
                    seen_real.insert(real.clone());
                    files.push(FoundFile {
                        base,
                        tail,
                        real,
                        size,
                    });
                }
            }
            if matched
                && let Some(root) = classify(glob_free_prefix(template), &ctx)
                && !roots.contains(&root)
            {
                roots.push(root);
            }
        }
    }
    if files.is_empty() {
        return None;
    }
    let registry_keys = entry
        .registry
        .iter()
        .filter(|(_, rule)| applies(&rule.when, Os::Windows))
        .filter_map(|(key, _)| hkcu_key(key))
        .collect();
    Some(GameScan {
        title: title.to_owned(),
        files,
        roots,
        registry_keys,
    })
}

/// Scans every game of `manifest`, with the config's custom games applied, and returns those with
/// at least one file on this machine, sorted by title. The first prefix wins per logical path
/// (ADR-0007). A game with only registry keys is not listed, since the keys are not checked.
pub fn scan(manifest: &Manifest, config: &AppConfig, env: &Environment) -> Vec<GameScan> {
    let mut roots: Vec<Root> = env
        .steam_roots
        .iter()
        .map(|path| Root {
            path: path.clone(),
            kind: RootKind::Steam,
        })
        .collect();
    for root in &config.roots {
        if !roots.contains(root) {
            roots.push(root.clone());
        }
    }
    let wine_roots = roots
        .iter()
        .filter(|root| root.kind == RootKind::WinePrefix)
        .map(|root| root.path.clone())
        .collect();
    let shared = Shared {
        env,
        roots,
        wine_roots,
    };

    // Only the custom games' titles are copied; the manifest itself stays borrowed.
    let mut custom = Manifest {
        games: config
            .custom_games
            .iter()
            .filter_map(|game| Some((game.name.clone(), manifest.games.get(&game.name)?.clone())))
            .collect(),
    };
    merge_custom(&mut custom, &config.custom_games);
    let wine_prefixes: HashMap<&str, &PathBuf> = config
        .custom_games
        .iter()
        .filter_map(|game| Some((game.name.as_str(), game.wine_prefix.as_ref()?)))
        .collect();
    let mut games: BTreeMap<&str, &GameEntry> = manifest
        .games
        .iter()
        .map(|(title, entry)| (title.as_str(), entry))
        .collect();
    games.extend(
        custom
            .games
            .iter()
            .map(|(title, entry)| (title.as_str(), entry)),
    );

    games
        .into_iter()
        .filter_map(|(title, entry)| {
            scan_game(title, entry, wine_prefixes.get(title).copied(), &shared)
        })
        .collect()
}

#[cfg(test)]
mod tests;
