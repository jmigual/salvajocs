//! Finds every game's save files on this machine and where its registry keys live (ADR-0004,
//! ADR-0006, ADR-0007, ADR-0008).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::AppConfig;
#[cfg(not(windows))]
use crate::layout::LinuxFolders;
use crate::layout::{
    Base, Context, GameVars, Root, RootKind, WinFolders, classify, expand, glob_free_prefix,
    tail_of,
};
use crate::manifest::{GameEntry, Manifest, Os, When, merge_custom};
use crate::prefix::{PrefixError, proton_prefix, steam_roots, wine_user};
use crate::registry::HKCU;
use crate::registry::wine::RegFile;

/// What the scan knows about this machine besides the config. Plain data, so tests can build any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Environment {
    /// The native context: `WindowsNative` on Windows, `Linux` elsewhere. `None` when its
    /// folders cannot be determined.
    pub native: Option<Context>,
    /// Detected Steam roots; Proton prefixes are looked up under them.
    pub steam_roots: Vec<PathBuf>,
}

impl Environment {
    /// Detects the native context and the default Steam roots of this machine.
    pub fn detect() -> Self {
        #[cfg(windows)]
        let native = WinFolders::native().map(Context::WindowsNative);
        #[cfg(not(windows))]
        let native = LinuxFolders::native().map(Context::Linux);
        Self {
            native,
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

/// A game with a save file or a registry key on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameScan {
    /// The game's title.
    pub title: String,
    /// The files found, best prefix first.
    pub files: Vec<FoundFile>,
    /// The glob-free prefix of each template that matched, as `(Base, rest)`, in the template's
    /// own text: placeholders other than the leading one stay in `rest` and its case is the
    /// template's, not the disk's. Matching a file's tail to a root must therefore ignore case
    /// for `Win*` bases (ADR-0004).
    pub roots: Vec<(Base, String)>,
    /// The game's applicable `HKEY_CURRENT_USER\...` keys, from the manifest.
    pub registry_keys: Vec<String>,
    /// The first context, in priority order, whose registry holds any of `registry_keys`.
    pub registry_source: Option<RegistrySource>,
    /// Paths that could not be read; `files` may be incomplete.
    pub unreadable: Vec<PathBuf>,
}

/// Where a game's registry keys were found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistrySource {
    /// The live registry of native Windows.
    Windows,
    /// The `user.reg` of this Wine prefix.
    WinePrefix(PathBuf),
}

/// Why a path could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    /// A prefix has several non-system users and none is `steamuser`.
    AmbiguousPrefix(Vec<String>),
    /// A prefix has no user directory: the game has never run in it.
    NoUser,
    /// A configured prefix is not a Wine prefix.
    NotAPrefix,
    /// A read failed; holds the error text.
    Unreadable(String),
}

/// Something the scan could not do, reported instead of dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanProblem {
    /// The prefix, directory or file concerned.
    pub path: PathBuf,
    /// What went wrong.
    pub kind: ProblemKind,
}

impl From<PrefixError> for ScanProblem {
    fn from(err: PrefixError) -> Self {
        let (path, kind) = match err {
            PrefixError::NotAPrefix(path) => (path, ProblemKind::NotAPrefix),
            PrefixError::NoUser(path) => (path, ProblemKind::NoUser),
            PrefixError::Ambiguous { prefix, users } => {
                (prefix, ProblemKind::AmbiguousPrefix(users))
            }
            PrefixError::Io { path, source } => (path, ProblemKind::Unreadable(source.to_string())),
        };
        Self { path, kind }
    }
}

/// The result of [`scan`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanReport {
    /// Games with a file or a registry key on this machine, sorted by title.
    pub games: Vec<GameScan>,
    /// Everything that could not be resolved or read, without duplicates.
    pub problems: Vec<ScanProblem>,
}

/// What one scan resolves once and reuses for every game: prefixes, their `user.reg` and the
/// problems met on the way.
#[derive(Default)]
struct Cache {
    /// Resolved prefixes by path; `None` when unusable (reported once).
    prefixes: RefCell<HashMap<PathBuf, Option<Context>>>,
    /// Parsed `user.reg` by prefix path: `Ok(None)` when missing, `Err` when unreadable (reported once).
    registries: RefCell<HashMap<PathBuf, Result<Option<RegFile>, ()>>>,
    /// How many `user.reg` files were read.
    user_reg_loads: Cell<usize>,
    problems: RefCell<Vec<ScanProblem>>,
}

impl Cache {
    fn report(&self, path: PathBuf, kind: ProblemKind) {
        let problem = ScanProblem { path, kind };
        let mut problems = self.problems.borrow_mut();
        if !problems.contains(&problem) {
            problems.push(problem);
        }
    }

    /// The context of the prefix at `prefix`, for its Wine user; `None` and a problem when
    /// unusable.
    fn prefix(&self, prefix: &Path) -> Option<Context> {
        if let Some(hit) = self.prefixes.borrow().get(prefix) {
            return hit.clone();
        }
        let resolved = match wine_user(prefix) {
            Ok(user) => {
                let folders = WinFolders::for_prefix(prefix, &user);
                let prefix = prefix.to_path_buf();
                Some(Context::WinePrefix { prefix, folders })
            }
            Err(err) => {
                let problem = ScanProblem::from(err);
                self.report(problem.path, problem.kind);
                None
            }
        };
        self.prefixes
            .borrow_mut()
            .insert(prefix.to_path_buf(), resolved.clone());
        resolved
    }

    /// Whether the `user.reg` of `prefix` holds any of `keys`. The file is parsed once per scan;
    /// a missing file holds nothing. A failure is reported and returns the failing path.
    fn prefix_has_keys(&self, prefix: &Path, keys: &[String]) -> Result<bool, PathBuf> {
        let path = prefix.join("user.reg");
        let mut registries = self.registries.borrow_mut();
        let loaded = registries.entry(prefix.to_path_buf()).or_insert_with(|| {
            self.user_reg_loads.set(self.user_reg_loads.get() + 1);
            match fs::read_to_string(&path) {
                Ok(text) => RegFile::parse(&text)
                    .map(Some)
                    .map_err(|err| err.to_string()),
                Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(err) => Err(err.to_string()),
            }
            .map_err(|err| self.report(path.clone(), ProblemKind::Unreadable(err)))
        });
        match loaded {
            Err(()) => Err(path),
            Ok(None) => Ok(false),
            Ok(Some(file)) => match file.export(keys) {
                Ok(export) => Ok(!export.keys.is_empty()),
                Err(err) => {
                    self.report(path.clone(), ProblemKind::Unreadable(err.to_string()));
                    Err(path)
                }
            },
        }
    }

    /// Whether the live Windows registry holds any of `keys`. A failure is reported and returns
    /// the HKCU root as the failing path.
    #[cfg(windows)]
    fn windows_has_keys(&self, keys: &[String]) -> Result<bool, PathBuf> {
        match crate::registry::windows::export(keys) {
            Ok(export) => Ok(!export.keys.is_empty()),
            Err(err) => {
                let path = PathBuf::from(HKCU);
                self.report(path.clone(), ProblemKind::Unreadable(err.to_string()));
                Err(path)
            }
        }
    }

    #[cfg(not(windows))]
    fn windows_has_keys(&self, _keys: &[String]) -> Result<bool, PathBuf> {
        Ok(false)
    }

    /// The registry source of `ctx` when its registry holds any of `keys`; `Err` is the path
    /// whose read failed.
    fn registry_source(
        &self,
        ctx: &Context,
        keys: &[String],
    ) -> Result<Option<RegistrySource>, PathBuf> {
        Ok(match ctx {
            Context::Linux(_) => None,
            Context::WindowsNative(_) => self
                .windows_has_keys(keys)?
                .then_some(RegistrySource::Windows),
            Context::WinePrefix { prefix, .. } => self
                .prefix_has_keys(prefix, keys)?
                .then(|| RegistrySource::WinePrefix(prefix.clone())),
        })
    }
}

/// Everything shared by all games of one scan.
struct Shared<'a> {
    env: &'a Environment,
    /// Steam roots and configured roots, deduplicated.
    roots: Vec<Root>,
    /// `WinePrefix` roots, in configured order.
    wine_roots: Vec<PathBuf>,
    cache: Cache,
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

/// A manifest registry key in `HKEY_CURRENT_USER\a\b` form; `None` for other hives, the bare
/// root and keys with an empty element (ADR-0006).
fn hkcu_key(key: &str) -> Option<String> {
    let key = key.replace('/', "\\");
    let below = key.strip_prefix(HKCU)?.strip_prefix('\\')?;
    (!below.split('\\').any(str::is_empty)).then_some(key)
}

/// A read failure: the path and the error text.
type Issue = (PathBuf, String);

/// Records a read failure of `path`. A path that vanished meanwhile is not one.
fn note(issues: &mut Vec<Issue>, path: &Path, err: &io::Error) {
    if err.kind() != io::ErrorKind::NotFound {
        issues.push((path.to_path_buf(), err.to_string()));
    }
}

/// Pushes every file at or below `path`. Symlinked directories below `path` are not followed,
/// since a prefix's `Documents` is often a link into the real home.
fn collect_files(path: &Path, out: &mut Vec<(PathBuf, u64)>, issues: &mut Vec<Issue>) {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) => return note(issues, path, &err),
    };
    if meta.is_file() {
        out.push((path.to_path_buf(), meta.len()));
        return;
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(err) => return note(issues, path, &err),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                note(issues, path, &err);
                continue;
            }
        };
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => collect_files(&entry.path(), out, issues),
            Ok(_) => match fs::metadata(entry.path()) {
                Ok(meta) if meta.is_file() => out.push((entry.path(), meta.len())),
                Ok(_) => {}
                Err(err) => note(issues, &entry.path(), &err),
            },
            Err(err) => note(issues, &entry.path(), &err),
        }
    }
}

/// The real files one glob pattern matches; a matched directory contributes everything below it
/// (ADR-0008). A malformed pattern matches nothing.
fn matches(pattern: &str, issues: &mut Vec<Issue>) -> Vec<(PathBuf, u64)> {
    let mut files = Vec::new();
    if let Ok(paths) = glob::glob(pattern) {
        for path in paths {
            match path {
                Ok(path) => collect_files(&path, &mut files, issues),
                Err(err) => note(issues, err.path(), err.error()),
            }
        }
    }
    files
}

/// The contexts a game is evaluated in, best first (ADR-0007): its own prefixes (Proton, then
/// the custom `wine_prefix`), the `WinePrefix` roots, then the native context, each with the
/// game's `GameVars`. The context supplies `<osUserName>`.
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
    let vars = GameVars::new(
        title,
        install_dirs,
        &shared.roots,
        steam_id.map(|id| id.to_string()),
    );
    let mut out: Vec<_> = prefixes
        .iter()
        .filter_map(|prefix| shared.cache.prefix(prefix))
        .map(|ctx| (ctx, vars.clone()))
        .collect();
    if let Some(native) = &shared.env.native {
        out.push((native.clone(), vars));
    }
    out
}

/// Scans one game; `None` when it has neither a file nor a registry source.
fn scan_game(
    title: &str,
    entry: &GameEntry,
    wine_prefix: Option<&PathBuf>,
    shared: &Shared,
) -> Option<GameScan> {
    let mut files = Vec::new();
    let mut roots: Vec<(Base, String)> = Vec::new();
    let mut unreadable: Vec<PathBuf> = Vec::new();
    let mut seen_real = HashSet::new();
    let mut seen_logical = HashSet::new();
    let contexts = contexts(title, entry, wine_prefix, shared);
    for (ctx, vars) in &contexts {
        let os = context_os(ctx);
        for (template, rule) in &entry.files {
            let Some((base, _)) = classify(template, ctx) else {
                continue;
            };
            if !applies(&rule.when, os) {
                continue;
            }
            let mut matched = false;
            let mut issues = Vec::new();
            for (base_dir, pattern) in expand(template, ctx, vars) {
                for (real, size) in matches(&pattern, &mut issues) {
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
            for (path, error) in issues {
                shared
                    .cache
                    .report(path.clone(), ProblemKind::Unreadable(error));
                if !unreadable.contains(&path) {
                    unreadable.push(path);
                }
            }
            if matched
                && let Some(root) = classify(glob_free_prefix(template), ctx)
                && !roots.contains(&root)
            {
                roots.push(root);
            }
        }
    }
    let registry_keys: Vec<String> = entry
        .registry
        .iter()
        .filter(|(_, rule)| applies(&rule.when, Os::Windows))
        .filter_map(|(key, _)| hkcu_key(key))
        .collect();
    let mut registry_source = None;
    if !registry_keys.is_empty() {
        for (ctx, _) in &contexts {
            match shared.cache.registry_source(ctx, &registry_keys) {
                Ok(None) => {}
                Ok(source) => {
                    registry_source = source;
                    break;
                }
                // A lower-priority source must not stand in for one that could not be read.
                Err(path) => {
                    if !unreadable.contains(&path) {
                        unreadable.push(path);
                    }
                    break;
                }
            }
        }
    }
    if files.is_empty() && registry_source.is_none() {
        return None;
    }
    Some(GameScan {
        title: title.to_owned(),
        files,
        roots,
        registry_keys,
        registry_source,
        unreadable,
    })
}

/// The detected Steam roots followed by the config's roots, without duplicates.
pub(crate) fn roots(config: &AppConfig, env: &Environment) -> Vec<Root> {
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
    roots
}

/// Scans every game of `manifest`, with the config's custom games applied. A game is listed when
/// it has a file or a registry key on this machine; the first context in priority order wins per
/// logical path and for the registry (ADR-0007). Prefixes and paths that cannot be used are
/// returned as problems, never dropped.
pub fn scan(manifest: &Manifest, config: &AppConfig, env: &Environment) -> ScanReport {
    let roots = roots(config, env);
    let wine_roots: Vec<PathBuf> = roots
        .iter()
        .filter(|root| root.kind == RootKind::WinePrefix)
        .map(|root| root.path.clone())
        .collect();
    let shared = Shared {
        env,
        roots,
        wine_roots,
        cache: Cache::default(),
    };
    // Resolve the roots up front so a bad one is reported even when no game uses it.
    for prefix in &shared.wine_roots {
        shared.cache.prefix(prefix);
    }

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

    let games = games
        .into_iter()
        .filter_map(|(title, entry)| {
            scan_game(title, entry, wine_prefixes.get(title).copied(), &shared)
        })
        .collect();
    ScanReport {
        games,
        problems: shared.cache.problems.into_inner(),
    }
}

#[cfg(test)]
mod tests;
