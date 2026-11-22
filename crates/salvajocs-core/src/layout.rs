//! Pure path logic: semantic bases, target contexts, manifest template expansion and the
//! snapshot format (ADR-0004, ADR-0005, ADR-0008, ADR-0018).

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf, Prefix};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Current value of [`SnapshotMeta::format`].
pub const SNAPSHOT_FORMAT: u32 = 1;
/// Directory in a staged game that holds `<Base>/<tail>` files.
pub const FILES_DIR: &str = "files";
/// File name of the serialized [`SnapshotMeta`] in a staged game.
pub const META_FILE: &str = "salvajocs.json";
/// File name of the optional registry export in a staged game (ADR-0006).
pub const REGISTRY_FILE: &str = "registry.json";
/// File name of the serialized [`Hashes`] in a staged game (ADR-0018).
pub const HASHES_FILE: &str = "hashes.json";
/// Current value of [`Hashes::format`].
pub const HASHES_FORMAT: u32 = 1;

/// A semantic location a saved file is stored under, independent of the machine it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Base {
    /// The Windows user's home (`users/<user>` in a prefix).
    WinHome,
    /// The Windows `Documents` folder.
    WinDocuments,
    /// `%APPDATA%` (`AppData/Roaming`).
    WinAppData,
    /// `%LOCALAPPDATA%` (`AppData/Local`).
    WinLocalAppData,
    /// `AppData/LocalLow`.
    WinLocalAppDataLow,
    /// `%PUBLIC%`.
    WinPublic,
    /// `%PROGRAMDATA%`.
    WinProgramData,
    /// `%WINDIR%`.
    WinDir,
    /// The Linux home directory.
    Home,
    /// `$XDG_DATA_HOME`.
    XdgData,
    /// `$XDG_CONFIG_HOME`.
    XdgConfig,
    /// The game's install dir; the tail is relative to it.
    GameDir,
    /// A games root (for example the Steam root); the tail is relative to it.
    StoreRoot,
    /// A custom Unix absolute path; the tail is the path without its leading `/`. Unix only.
    Absolute,
    /// A custom Windows drive-letter path; the first tail component is the drive letter without
    /// the colon (`C/Games/Foo`), since `C:` is not a valid directory name. Native Windows only.
    WinAbsolute,
}

impl Base {
    /// The directory name used under `files/`; equal to the serialized name.
    pub fn dir_name(self) -> &'static str {
        match self {
            Base::WinHome => "WinHome",
            Base::WinDocuments => "WinDocuments",
            Base::WinAppData => "WinAppData",
            Base::WinLocalAppData => "WinLocalAppData",
            Base::WinLocalAppDataLow => "WinLocalAppDataLow",
            Base::WinPublic => "WinPublic",
            Base::WinProgramData => "WinProgramData",
            Base::WinDir => "WinDir",
            Base::Home => "Home",
            Base::XdgData => "XdgData",
            Base::XdgConfig => "XdgConfig",
            Base::GameDir => "GameDir",
            Base::StoreRoot => "StoreRoot",
            Base::Absolute => "Absolute",
            Base::WinAbsolute => "WinAbsolute",
        }
    }
}

/// The folders of a Windows user, native or inside a Wine prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct WinFolders {
    /// The user name, for `<osUserName>`: the Wine user in a prefix.
    pub user: String,
    /// User home.
    pub home: PathBuf,
    /// `Documents`.
    pub documents: PathBuf,
    /// `%APPDATA%`.
    pub app_data: PathBuf,
    /// `%LOCALAPPDATA%`.
    pub local_app_data: PathBuf,
    /// `<home>/AppData/LocalLow`.
    pub local_app_data_low: PathBuf,
    /// `%PUBLIC%`.
    pub public: PathBuf,
    /// `%PROGRAMDATA%`.
    pub program_data: PathBuf,
    /// `%WINDIR%`.
    pub windows: PathBuf,
}

impl WinFolders {
    /// The current Windows user's Known Folders, so a redirected Documents folder is honoured.
    /// `None` if one of them cannot be determined.
    #[cfg(windows)]
    pub fn native() -> Option<Self> {
        let home = dirs::home_dir()?;
        let env = |name: &str| std::env::var_os(name).map(PathBuf::from);
        Some(Self {
            documents: dirs::document_dir()?,
            app_data: dirs::config_dir()?,
            local_app_data: dirs::data_local_dir()?,
            local_app_data_low: home.join("AppData/LocalLow"),
            public: env("PUBLIC")?,
            program_data: env("PROGRAMDATA")?,
            windows: env("WINDIR")?,
            home,
            user: os_user(),
        })
    }

    /// The folders of `user` inside the Wine prefix at `prefix`.
    pub fn for_prefix(prefix: &Path, user: &str) -> Self {
        let drive_c = prefix.join("drive_c");
        let home = drive_c.join("users").join(user);
        Self {
            documents: home.join("Documents"),
            app_data: home.join("AppData/Roaming"),
            local_app_data: home.join("AppData/Local"),
            local_app_data_low: home.join("AppData/LocalLow"),
            public: drive_c.join("users/Public"),
            program_data: drive_c.join("ProgramData"),
            windows: drive_c.join("windows"),
            home,
            user: user.to_owned(),
        }
    }
}

/// The folders of a native Linux user.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LinuxFolders {
    /// The user name, for `<osUserName>`.
    pub user: String,
    /// Home directory.
    pub home: PathBuf,
    /// `$XDG_DATA_HOME`.
    pub xdg_data: PathBuf,
    /// `$XDG_CONFIG_HOME`.
    pub xdg_config: PathBuf,
}

impl LinuxFolders {
    /// The folders of `user` from their paths.
    pub fn new(user: String, home: PathBuf, xdg_data: PathBuf, xdg_config: PathBuf) -> Self {
        Self {
            user,
            home,
            xdg_data,
            xdg_config,
        }
    }

    /// The current user's folders, honouring `XDG_*`. `None` if one cannot be determined.
    #[cfg(unix)]
    pub fn native() -> Option<Self> {
        Some(Self {
            user: os_user(),
            home: dirs::home_dir()?,
            xdg_data: dirs::data_dir()?,
            xdg_config: dirs::config_dir()?,
        })
    }
}

/// The OS user name from `USER` or `USERNAME`; empty when neither is set.
fn os_user() -> String {
    ["USER", "USERNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .unwrap_or_default()
}

/// What a configured root is used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RootKind {
    /// A Steam root: games live in `<root>/steamapps/common/<installDir>`.
    Steam,
    /// Any other games dir: games live in `<root>/<installDir>`.
    Other,
    /// A Wine prefix scanned for its Windows folders (ADR-0007); holds no `<root>`/`<base>`.
    WinePrefix,
}

/// A configured or detected root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Root {
    /// Root directory.
    pub path: PathBuf,
    /// What the root is for.
    pub kind: RootKind,
}

impl Root {
    /// A root at `path` of `kind`.
    pub fn new(path: PathBuf, kind: RootKind) -> Self {
        Self { path, kind }
    }
}

/// Where a set of bases resolves to. Plain data, so tests can build any of them.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Context {
    /// A native Windows user.
    WindowsNative(WinFolders),
    /// A Wine/Proton prefix. Keeps the prefix path for `user.reg` and case-insensitive matching.
    WinePrefix {
        /// The prefix directory.
        prefix: PathBuf,
        /// The prefix user's folders.
        folders: WinFolders,
    },
    /// A native Linux user.
    Linux(LinuxFolders),
}

impl Context {
    /// Whether paths in this context must match case-insensitively (a prefix is case-sensitive
    /// on disk but Windows programs are not; native Windows already ignores case).
    pub fn case_insensitive(&self) -> bool {
        matches!(self, Context::WinePrefix { .. })
    }

    pub(crate) fn win(&self) -> Option<&WinFolders> {
        match self {
            Context::WindowsNative(folders) | Context::WinePrefix { folders, .. } => Some(folders),
            Context::Linux(_) => None,
        }
    }

    /// The context's user name, for `<osUserName>`.
    pub fn user(&self) -> &str {
        match self {
            Context::WindowsNative(folders) | Context::WinePrefix { folders, .. } => &folders.user,
            Context::Linux(folders) => &folders.user,
        }
    }

    fn linux(&self) -> Option<&LinuxFolders> {
        match self {
            Context::Linux(folders) => Some(folders),
            _ => None,
        }
    }
}

/// The per-game values a template needs besides the context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameVars {
    /// `<game>` candidates: the `installDir` names, or the title when there are none.
    pub(crate) game: Vec<String>,
    /// `<base>` candidates, one per root and `<game>`.
    base: Vec<PathBuf>,
    /// `<root>` candidates.
    root: Vec<PathBuf>,
    pub(crate) store_game_id: Option<String>,
}

impl GameVars {
    /// Builds the variables of a game. `install_dirs` falls back to `[title]` when empty;
    /// names that are not a single plain path component (manifest input) are dropped.
    /// `<base>` is `<root>/steamapps/common/<game>` for Steam roots and `<root>/<game>` for
    /// others. `<root>` is a Steam root only; `WinePrefix` roots contribute neither.
    /// `<osUserName>` comes from the context, not from here.
    pub fn new(
        title: &str,
        install_dirs: Vec<String>,
        roots: &[Root],
        store_game_id: Option<String>,
    ) -> Self {
        let mut game = if install_dirs.is_empty() {
            vec![title.to_owned()]
        } else {
            install_dirs
        };
        game.retain(|g| is_plain_component(g));
        let mut base = Vec::new();
        let mut root = Vec::new();
        for r in roots {
            let games_dir = match r.kind {
                RootKind::Steam => {
                    root.push(r.path.clone());
                    r.path.join("steamapps/common")
                }
                RootKind::Other => r.path.clone(),
                RootKind::WinePrefix => continue,
            };
            base.extend(game.iter().map(|g| games_dir.join(g)));
        }
        Self {
            game,
            base,
            root,
            store_game_id,
        }
    }
}

/// The directories a base can resolve to, in priority order.
pub(crate) fn base_dirs(base: Base, ctx: &Context, vars: &GameVars) -> Vec<PathBuf> {
    let fixed = |dir: Option<&PathBuf>| dir.cloned().into_iter().collect();
    match base {
        Base::WinHome => fixed(ctx.win().map(|w| &w.home)),
        Base::WinDocuments => fixed(ctx.win().map(|w| &w.documents)),
        Base::WinAppData => fixed(ctx.win().map(|w| &w.app_data)),
        Base::WinLocalAppData => fixed(ctx.win().map(|w| &w.local_app_data)),
        Base::WinLocalAppDataLow => fixed(ctx.win().map(|w| &w.local_app_data_low)),
        Base::WinPublic => fixed(ctx.win().map(|w| &w.public)),
        Base::WinProgramData => fixed(ctx.win().map(|w| &w.program_data)),
        Base::WinDir => fixed(ctx.win().map(|w| &w.windows)),
        Base::Home => fixed(ctx.linux().map(|l| &l.home)),
        Base::XdgData => fixed(ctx.linux().map(|l| &l.xdg_data)),
        Base::XdgConfig => fixed(ctx.linux().map(|l| &l.xdg_config)),
        Base::GameDir => vars.base.clone(),
        Base::StoreRoot => vars.root.clone(),
        Base::Absolute => cfg!(unix).then(|| PathBuf::from("/")).into_iter().collect(),
        // The drive comes from the tail, see `win_absolute_target`.
        Base::WinAbsolute => Vec::new(),
    }
}

/// The directory `base` resolves to in `ctx`, or `None` when the context has no such base or
/// (for `GameDir`/`StoreRoot`) no candidate exists on disk. `WinAbsolute` has no base dir; it
/// resolves through [`win_absolute_target`].
pub fn base_dir(base: Base, ctx: &Context, vars: &GameVars) -> Option<PathBuf> {
    let mut dirs = base_dirs(base, ctx, vars).into_iter();
    match base {
        Base::GameDir | Base::StoreRoot => dirs.find(|dir| dir.is_dir()),
        _ => dirs.next(),
    }
}

pub(crate) const WIN_LEADING: [(&str, Base); 8] = [
    ("<home>", Base::WinHome),
    ("<winDocuments>", Base::WinDocuments),
    ("<winAppData>", Base::WinAppData),
    ("<winLocalAppData>", Base::WinLocalAppData),
    ("<winLocalAppDataLow>", Base::WinLocalAppDataLow),
    ("<winPublic>", Base::WinPublic),
    ("<winProgramData>", Base::WinProgramData),
    ("<winDir>", Base::WinDir),
];
pub(crate) const LINUX_LEADING: [(&str, Base); 3] = [
    ("<home>", Base::Home),
    ("<xdgData>", Base::XdgData),
    ("<xdgConfig>", Base::XdgConfig),
];
pub(crate) const ANY_LEADING: [(&str, Base); 2] =
    [("<base>", Base::GameDir), ("<root>", Base::StoreRoot)];

/// The placeholders a Windows (`windows`) or Linux path template may start with: the OS's own
/// named folders, then `<base>` and `<root>`.
pub fn leading_placeholders(windows: bool) -> impl Iterator<Item = &'static str> {
    let own: &[(&str, Base)] = if windows {
        &WIN_LEADING
    } else {
        &LINUX_LEADING
    };
    own.iter()
        .chain(&ANY_LEADING)
        .map(|&(placeholder, _)| placeholder)
}
const WIN_HOME_SUBDIRS: [(&str, Base); 3] = [
    ("AppData/Roaming", Base::WinAppData),
    ("AppData/Local", Base::WinLocalAppData),
    ("AppData/LocalLow", Base::WinLocalAppDataLow),
];
const LINUX_HOME_SUBDIRS: [(&str, Base); 2] = [
    (".config", Base::XdgConfig),
    (".local/share", Base::XdgData),
];

/// The `WinAbsolute` tail of a Windows drive-letter template (`C:\Games\Foo` -> `C/Games/Foo`).
/// `None` for anything else, including UNC paths and drive-relative `C:foo`.
pub(crate) fn drive_absolute_tail(template: &str) -> Option<String> {
    let mut chars = template.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic)?;
    if chars.next() != Some(':') || !matches!(chars.next(), Some('/' | '\\')) {
        return None;
    }
    let below = template[3..].replace('\\', "/");
    Some(format!("{drive}/{below}").trim_end_matches('/').to_owned())
}

/// Splits a `WinAbsolute` tail into its drive root (`C:\`) and the rest. `None` unless the
/// first component is exactly one ASCII letter.
fn split_drive(tail: &str) -> Option<(PathBuf, &str)> {
    let (drive, rest) = tail.split_once('/').unwrap_or((tail, ""));
    let mut chars = drive.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), None) if letter.is_ascii_alphabetic() => {
            Some((PathBuf::from(format!("{letter}:\\")), rest))
        }
        _ => None,
    }
}

/// The path a `WinAbsolute` tail restores to. Only a native Windows context has drives, so any
/// other context, or a tail without a drive letter, gives `None`.
pub fn win_absolute_target(tail: &str, ctx: &Context) -> Option<PathBuf> {
    if !matches!(ctx, Context::WindowsNative(_)) {
        return None;
    }
    let (root, rest) = split_drive(tail)?;
    target_path(&root, rest, false)
}

/// What follows `dir` in `path`, when `dir` ends on a component boundary; `dir` may be empty.
/// `ignore_case` ignores ASCII case in `dir`.
fn below<'a>(path: &'a str, dir: &str, ignore_case: bool) -> Option<&'a str> {
    let after = if ignore_case {
        let head = path.get(..dir.len())?;
        &path[head.eq_ignore_ascii_case(dir).then_some(dir.len())?..]
    } else {
        path.strip_prefix(dir)?
    };
    if after.is_empty() {
        Some(after)
    } else {
        after.strip_prefix('/')
    }
}

/// The base and rest a `WinHome` root under `AppData/<Roaming|Local|LocalLow>`, in any ASCII
/// case, is classified as since [`classify`] ignores that case (ADR-0004). Snapshots, saved
/// choices and settings made before hold the `WinHome` form. `None` for any other root.
pub(crate) fn normalized_win_home(base: Base, rest: &str) -> Option<(Base, String)> {
    if base != Base::WinHome {
        return None;
    }
    WIN_HOME_SUBDIRS
        .iter()
        .find_map(|&(dir, base)| below(rest, dir, true).map(|rest| (base, rest.to_owned())))
}

/// Splits a template into its base and the path below it. The base comes from the leading
/// placeholder and the context's family (ADR-0004). `None` for relative paths, unknown leading
/// placeholders and placeholders the context does not have.
pub fn classify(template: &str, ctx: &Context) -> Option<(Base, String)> {
    // The manifest spells Windows folders in any case (`<home>/Appdata/LocalLow`).
    let windows = ctx.win().is_some();
    let (leading, home_subdirs) = if windows {
        (&WIN_LEADING[..], &WIN_HOME_SUBDIRS[..])
    } else {
        (&LINUX_LEADING[..], &LINUX_HOME_SUBDIRS[..])
    };
    let found = leading
        .iter()
        .chain(&ANY_LEADING)
        .find_map(|&(placeholder, base)| {
            below(template.strip_prefix(placeholder)?, "", false).map(|rest| (base, rest))
        });
    let Some((base, rest)) = found else {
        return if cfg!(windows) {
            drive_absolute_tail(template).map(|tail| (Base::WinAbsolute, tail))
        } else if template.starts_with('/') {
            Some((Base::Absolute, template.trim_start_matches('/').to_owned()))
        } else {
            None
        };
    };
    if matches!(base, Base::WinHome | Base::Home)
        && let Some((base, rest)) = home_subdirs
            .iter()
            .find_map(|&(dir, base)| below(rest, dir, windows).map(|rest| (base, rest)))
    {
        return Some((base, rest.to_owned()));
    }
    Some((base, rest.to_owned()))
}

/// The leading components of `template` before the first one containing a glob character or
/// `<storeUserId>` (which expands to `*`).
pub fn glob_free_prefix(template: &str) -> &str {
    let mut end = 0;
    for part in template.split('/') {
        if part.contains(['*', '?', '[']) || part.contains("<storeUserId>") {
            return template[..end].trim_end_matches('/');
        }
        end += part.len() + 1;
    }
    template
}

/// Rewrites every ASCII letter outside a `[...]` class into a two-case class (`My` -> `[mM][yY]`).
fn fold_case(rest: &str) -> String {
    let mut out = String::with_capacity(rest.len() * 4);
    let mut in_class = false;
    // True until the first member of a class; a `]` there is a member, not the end.
    let mut first = false;
    for c in rest.chars() {
        if !in_class {
            if c == '[' {
                in_class = true;
                first = true;
            } else if c.is_ascii_alphabetic() {
                out.extend(['[', c.to_ascii_lowercase(), c.to_ascii_uppercase(), ']']);
                continue;
            }
        } else if c == ']' && !first {
            in_class = false;
        } else if c != '!' || !first {
            first = false;
        }
        out.push(c);
    }
    out
}

/// Builds a glob pattern from a resolved `base_dir` (escaped, always case-sensitive) and the
/// template's `rest`. With `case_insensitive`, every part of `rest` becomes a wildcard part so
/// the `glob` crate lists directories instead of calling the case-sensitive `fs::metadata`
/// (ADR-0008).
pub fn glob_pattern(base_dir: &Path, rest: &str, case_insensitive: bool) -> String {
    let base = glob::Pattern::escape(&base_dir.to_string_lossy());
    let base = base.trim_end_matches('/');
    if rest.is_empty() {
        return base.to_owned();
    }
    let rest = if case_insensitive {
        fold_case(rest)
    } else {
        rest.to_owned()
    };
    format!("{base}/{rest}")
}

/// Replaces the value placeholders in `rest`, one result per `<game>` candidate, with `user`
/// for `<osUserName>`. Substituted values are glob-escaped; `<storeUserId>` becomes `*`.
pub(crate) fn substitute(rest: &str, vars: &GameVars, user: &str) -> Vec<String> {
    let escape = |value: &str| glob::Pattern::escape(value);
    let rest = rest
        .replace("<storeUserId>", "*")
        .replace("<osUserName>", &escape(user));
    let rest = match (&vars.store_game_id, rest.contains("<storeGameId>")) {
        (Some(id), _) => rest.replace("<storeGameId>", &escape(id)),
        (None, true) => return Vec::new(),
        (None, false) => rest,
    };
    if !rest.contains("<game>") {
        return vec![rest];
    }
    vars.game
        .iter()
        .map(|game| rest.replace("<game>", &escape(game)))
        .collect()
}

/// Every `(base dir, glob pattern)` `template` yields in `ctx`: one per candidate base dir and
/// `<game>`. A match's tail is `tail_of(match, base dir)`. Empty when the template does not
/// classify or the context lacks its base.
pub fn expand(template: &str, ctx: &Context, vars: &GameVars) -> Vec<(PathBuf, String)> {
    let Some((base, rest)) = classify(template, ctx) else {
        return Vec::new();
    };
    // Absolute holds a host path, which stays case-sensitive (ADR-0008).
    let case_insensitive = ctx.case_insensitive() && base != Base::Absolute;
    let rests = substitute(&rest, vars, ctx.user());
    if base == Base::WinAbsolute {
        if !matches!(ctx, Context::WindowsNative(_)) {
            return Vec::new();
        }
        // The empty base dir makes `tail_of` keep the drive letter as the first component.
        return rests
            .iter()
            .filter_map(|rest| split_drive(rest))
            .map(|(root, below)| {
                let root = root.to_string_lossy();
                (
                    PathBuf::new(),
                    format!("{}/{below}", root.trim_end_matches('\\')),
                )
            })
            .collect();
    }
    base_dirs(base, ctx, vars)
        .into_iter()
        .flat_map(|dir| {
            rests
                .iter()
                .map(|rest| (dir.clone(), glob_pattern(&dir, rest, case_insensitive)))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The path of `real_path` below `base_dir`, with `/` separators. `None` if it is not below it
/// or contains anything but plain components (such as `..`). With an empty `base_dir` a drive
/// path keeps its drive letter as the first component (the `WinAbsolute` tail).
pub fn tail_of(real_path: &Path, base_dir: &Path) -> Option<String> {
    let parts: Option<Vec<_>> = real_path
        .strip_prefix(base_dir)
        .ok()?
        .components()
        .filter(|c| !matches!(c, Component::RootDir))
        .map(|c| match c {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(letter) => Some(char::from(letter).to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    Some(parts?.join("/"))
}

/// Whether `part` is a single plain path component (no `..`, separators or drive prefix).
pub fn is_plain_component(part: &str) -> bool {
    let mut components = Path::new(part).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    )
}

/// The path `tail` maps to below `base_dir`. With `case_insensitive`, each component reuses an
/// existing entry that differs only by ASCII case, so `My Games` never sits next to `my games`.
/// `None` if the tail (read from a snapshot) would escape `base_dir`.
pub fn target_path(base_dir: &Path, tail: &str, case_insensitive: bool) -> Option<PathBuf> {
    let mut path = base_dir.to_path_buf();
    for part in tail.split('/').filter(|p| !p.is_empty() && *p != ".") {
        if !is_plain_component(part) {
            return None;
        }
        let existing = if case_insensitive && !path.join(part).exists() {
            std::fs::read_dir(&path).ok().and_then(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.file_name())
                    .filter(|name| name.to_string_lossy().eq_ignore_ascii_case(part))
                    .min()
            })
        } else {
            None
        };
        path.push(existing.as_deref().unwrap_or(OsStr::new(part)));
    }
    Some(path)
}

/// Which Wine prefix a game's saves came from, described without any path of the machine, so
/// another machine can find its own copy (ADR-0023).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum PrefixOrigin {
    /// Proton's `steamapps/compatdata/<id>/pfx` of a Steam library, by app id.
    Proton(u32),
    /// A Heroic prefix, by the app name of `GamesConfig/<app>.json`.
    Heroic(String),
    /// A Lutris prefix, by the game slug.
    Lutris(String),
    /// A Bottles bottle, by its directory name in `bottles/bottles`.
    Bottles(String),
    /// A prefix below the home folder, as `<home>/<rest>`, `/`-separated.
    Home(String),
}

impl PrefixOrigin {
    /// `Home` for the prefix at `path` below `home`; `None` when it is not below it, or a part
    /// of the rest is not a plain UTF-8 component.
    pub(crate) fn below_home(path: &Path, home: &Path) -> Option<Self> {
        // Every path is below a home of `/`, which would record an absolute path.
        if !(home.components()).any(|part| matches!(part, Component::Normal(_))) {
            return None;
        }
        let parts = (path.strip_prefix(home).ok()?.components())
            .map(|part| match part {
                Component::Normal(part) => part.to_str().filter(|part| is_plain_component(part)),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        (!parts.is_empty()).then(|| Self::Home(format!("<home>/{}", parts.join("/"))))
    }

    /// Where a `Home` prefix is below `home`; `None` for another kind, or a rest that is not
    /// `<home>/` and plain components, since a snapshot is untrusted (ADR-0005).
    pub(crate) fn home_path(&self, home: &Path) -> Option<PathBuf> {
        let Self::Home(rest) = self else {
            return None;
        };
        let parts: Vec<&str> = rest.strip_prefix("<home>/")?.split('/').collect();
        (parts.iter().all(|part| is_plain_component(part))).then(|| {
            parts
                .iter()
                .fold(home.to_path_buf(), |path, part| path.join(part))
        })
    }
}

/// The `salvajocs.json` of a staged game (ADR-0004).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SnapshotMeta {
    /// Format version, [`SNAPSHOT_FORMAT`].
    pub format: u32,
    /// Game title.
    pub title: String,
    /// The glob-free prefix of each template that produced files, as `(base, rest)`.
    pub roots: Vec<(Base, String)>,
    /// The prefix the Windows-folder files came from; `None` when not from a prefix, or when it
    /// is a kind this version does not know (ADR-0023).
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub prefix: Option<PrefixOrigin>,
}

impl SnapshotMeta {
    /// The meta of format `format` for `title` with its `roots`, without a prefix.
    pub fn new(format: u32, title: String, roots: Vec<(Base, String)>) -> Self {
        Self {
            format,
            title,
            roots,
            prefix: None,
        }
    }
}

/// A prefix that does not parse reads as none, so a kind from a newer version keeps the
/// snapshot's roots readable.
fn lenient<'de, D>(deserializer: D) -> Result<Option<PrefixOrigin>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}

/// The `hashes.json` of a staged game: the SHA-256 of every other file in it (ADR-0018).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Hashes {
    /// Format version, [`HASHES_FORMAT`].
    pub format: u32,
    /// The SHA-256 of `salvajocs.json`, in hex.
    pub meta: String,
    /// The SHA-256 of `registry.json` in hex, `None` when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// Each file below `files/`, by its `<Base>/<tail>` path.
    pub files: BTreeMap<String, FileHash>,
}

/// One file of [`Hashes`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FileHash {
    /// The SHA-256 of the file, in hex.
    pub sha256: String,
    /// Size in bytes.
    pub size: u64,
    /// The staged file's mtime since the epoch, which tells whether the entry still describes
    /// it; `None` when it is unknown or before the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<Duration>,
}

impl Hashes {
    /// Parses a `hashes.json`; `None` when it is not one this version reads, such as a newer
    /// format, so the caller falls back as if there were none.
    pub fn from_json(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes)
            .ok()
            .filter(|hashes: &Self| hashes.format <= HASHES_FORMAT)
    }

    /// Whether both lists describe the same content: the same `salvajocs.json` and
    /// `registry.json`, and the same files with the same sizes and hashes. Mtimes are ignored.
    pub fn same_content(&self, other: &Hashes) -> bool {
        self.meta == other.meta
            && self.registry == other.registry
            && self.files.len() == other.files.len()
            && (self.files.iter().zip(&other.files)).all(|((path, a), (other_path, b))| {
                path == other_path && a.sha256 == b.sha256 && a.size == b.size
            })
    }
}

#[cfg(test)]
mod tests;
