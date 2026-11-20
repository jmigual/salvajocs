//! Wine prefixes of the Heroic, Lutris and Bottles launchers, and the title normalization used
//! to match a launcher's game to a manifest entry.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::manifest::Manifest;
use crate::prefix::{PrefixError, enclosing_prefix, validate_prefix};
use crate::scan::{ProblemKind, ScanProblem};

/// A launcher that keeps games in Wine prefixes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Launcher {
    /// Heroic Games Launcher (GOG, Epic, Amazon and sideloaded games).
    Heroic,
    /// Lutris.
    Lutris,
    /// Bottles.
    Bottles,
}

/// What a launcher knows about the game in a prefix, to match it to a manifest entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LauncherGame {
    /// The title the launcher shows.
    pub title: Option<String>,
    /// The GOG product id.
    pub gog_id: Option<u64>,
    /// The Steam app id.
    pub steam_id: Option<u32>,
    /// The Lutris game slug.
    pub lutris_slug: Option<String>,
}

/// A valid Wine prefix a launcher uses.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LauncherPrefix {
    /// The launcher that uses the prefix.
    pub launcher: Launcher,
    /// The prefix; it passes [`validate_prefix`].
    pub path: PathBuf,
    /// The game run in the prefix; `None` when the launcher does not tie a prefix to one game.
    pub game: Option<LauncherGame>,
    /// The launcher's own key for the prefix, the same on every machine: the Heroic app name,
    /// the Lutris game slug or the bottle's directory name (ADR-0023). Empty when unknown, and
    /// then never recorded or matched.
    pub name: String,
}

impl LauncherPrefix {
    /// The prefix at `path` that `launcher` uses, for `game` when it ties one to it, with an
    /// empty `name`.
    pub fn new(launcher: Launcher, path: PathBuf, game: Option<LauncherGame>) -> Self {
        Self {
            launcher,
            path,
            game,
            name: String::new(),
        }
    }
}

/// Finds the prefixes of the native and Flatpak installs of each launcher under the given
/// directories (`dirs::home_dir`, `dirs::config_dir` and `dirs::data_dir` on a real system).
/// Paths that are not valid prefixes are skipped; launcher files that cannot be read or parsed
/// are returned as problems. Missing launchers are not problems.
pub fn detect(
    home: &Path,
    config_dir: &Path,
    data_dir: &Path,
) -> (Vec<LauncherPrefix>, Vec<ScanProblem>) {
    let flatpak = home.join(".var/app");
    let mut found = Found::default();
    for dir in [
        config_dir.join("heroic"),
        flatpak.join("com.heroicgameslauncher.hgl/config/heroic"),
    ] {
        found.heroic(home, &dir);
    }
    let lutris_flatpak = flatpak.join("net.lutris.Lutris");
    for (config, data) in [
        (config_dir.to_path_buf(), data_dir.to_path_buf()),
        (lutris_flatpak.join("config"), lutris_flatpak.join("data")),
    ] {
        // Lutris moved its config to the data dir and uses the old one only while it exists.
        let config = config.join("lutris");
        let dir = if config.is_dir() {
            config
        } else {
            data.join("lutris")
        };
        found.lutris(home, &dir);
    }
    let bottles_data = flatpak.join("com.usebottles.bottles/data");
    for (dir, var_data) in [
        (data_dir.join("bottles/bottles"), None),
        (
            bottles_data.join("bottles/bottles"),
            Some(bottles_data.as_path()),
        ),
    ] {
        found.bottles(&dir, var_data);
    }
    (found.prefixes, found.problems)
}

/// A Heroic store library: GOG and sideloaded apps are under `games`, Epic and Amazon ones
/// under `library`.
#[derive(Deserialize)]
struct HeroicLibrary {
    #[serde(default)]
    games: Vec<HeroicApp>,
    #[serde(default)]
    library: Vec<HeroicApp>,
}

#[derive(Deserialize)]
struct HeroicApp {
    app_name: String,
    title: String,
    /// Empty or missing when the app is not installed.
    #[serde(default)]
    install: Option<HeroicInstall>,
}

#[derive(Deserialize)]
struct HeroicInstall {
    #[serde(default)]
    platform: Option<String>,
}

/// An entry of Heroic's GOG `gog_store/installed.json`.
#[derive(Deserialize)]
struct GogInstall {
    #[serde(rename = "appName")]
    app_name: String,
    platform: String,
}

#[derive(Deserialize)]
struct GogInstalled {
    installed: Vec<GogInstall>,
}

/// An entry of legendary's (Epic) `installed.json`, a map keyed by app name.
#[derive(Deserialize)]
struct LegendaryInstall {
    platform: String,
}

/// An entry of nile's (Amazon) `installed.json`, a list.
#[derive(Deserialize)]
struct NileInstall {
    id: String,
}

/// What Heroic's library and install files say about an app.
#[derive(Default)]
struct HeroicAppInfo {
    game: LauncherGame,
    /// The installed platform, lowercase: `windows`, `linux`, `mac`, ...
    platform: Option<String>,
}

/// A Lutris `games/<id>.yml`. Installers write every field; a game added by hand has only
/// `game` and runner sections.
#[derive(Default, Deserialize)]
#[serde(default)]
struct LutrisGame {
    name: Option<String>,
    game_slug: Option<String>,
    service: Option<String>,
    /// The GOG product id or Steam app id when `service` is `gog` or `steam`.
    service_id: Option<String>,
    game: LutrisGameSection,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct LutrisGameSection {
    prefix: Option<String>,
    exe: Option<String>,
}

/// A bottle's `placeholder.yml`, left where a bottle kept elsewhere would be.
#[derive(Deserialize)]
struct BottlePlaceholder {
    #[serde(rename = "Path")]
    path: String,
}

/// The part of a `bottle.yml` that says where a bottle kept elsewhere is.
#[derive(Deserialize)]
struct BottleConfig {
    #[serde(rename = "Path", default)]
    path: String,
    #[serde(rename = "Custom_Path", default)]
    custom_path: bool,
}

/// What [`detect`] has found so far.
#[derive(Default)]
struct Found {
    prefixes: Vec<LauncherPrefix>,
    problems: Vec<ScanProblem>,
}

impl Found {
    /// Reads Heroic's per-game `GamesConfig/<app>.json`. Apps installed for Linux are skipped. A
    /// config stripped of its defaults has no `winePrefix`; Heroic then uses the default from
    /// `config.json`, else its built-in one, but only Windows apps run in it.
    fn heroic(&mut self, home: &Path, dir: &Path) {
        let config: Option<Value> = self.json(&dir.join("config.json"));
        let default_prefix = config
            .as_ref()
            .and_then(|c| c.pointer("/defaultSettings/winePrefix"))
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .map_or_else(
                || home.join("Games/Heroic/Prefixes/shared"),
                |p| expand_home(home, p),
            );
        let apps = self.heroic_apps(dir);
        for file in self.entries(&dir.join("GamesConfig")) {
            if file.extension() != Some(OsStr::new("json")) {
                continue;
            }
            let Some(app) = file.file_stem().and_then(OsStr::to_str) else {
                continue;
            };
            let Some(config) = self.json::<Value>(&file) else {
                continue;
            };
            let info = apps.get(app);
            let platform = info.and_then(|i| i.platform.as_deref());
            if platform == Some("linux") {
                continue;
            }
            let wine_prefix = config
                .get(app)
                .and_then(|game| game.get("winePrefix"))
                .and_then(Value::as_str);
            let prefix = match wine_prefix {
                // Heroic keeps an empty prefix as it is, so the game has none.
                Some("") => continue,
                Some(prefix) => expand_home(home, prefix),
                None if matches!(platform, Some("windows" | "win32")) => default_prefix.clone(),
                None => continue,
            };
            // A Proton prefix keeps the Wine prefix in `pfx`. The type is not checked because a
            // stripped config may lack `wineVersion` too.
            let pfx = prefix.join("pfx");
            let game = info.map(|i| i.game.clone()).unwrap_or_default();
            self.push(Launcher::Heroic, app, [prefix, pfx], Some(game));
        }
    }

    /// The titles and installed platforms of Heroic's apps by app name. A GOG app name is the
    /// product id.
    fn heroic_apps(&mut self, dir: &Path) -> HashMap<String, HeroicAppInfo> {
        let mut apps = HashMap::<String, HeroicAppInfo>::new();
        let gog = self
            .json(&dir.join("store_cache/gog_library.json"))
            .or_else(|| self.json(&dir.join("gog_store/library.json")));
        let others = [
            "store_cache/legendary_library.json",
            "store_cache/nile_library.json",
            "sideload_apps/library.json",
        ]
        .map(|file| self.json(&dir.join(file)));
        for (is_gog, library) in [(true, gog)].into_iter().chain(others.map(|l| (false, l))) {
            let Some(HeroicLibrary {
                games: entries,
                library,
            }) = library
            else {
                continue;
            };
            for app in entries.into_iter().chain(library) {
                let info = apps.entry(app.app_name.clone()).or_default();
                info.game = LauncherGame {
                    gog_id: is_gog.then(|| app.app_name.parse().ok()).flatten(),
                    title: Some(app.title),
                    ..LauncherGame::default()
                };
                info.platform = app.install.and_then(|i| i.platform);
            }
        }
        let mut installed = Vec::new();
        if let Some(gog) = self.json::<GogInstalled>(&dir.join("gog_store/installed.json")) {
            installed.extend(gog.installed.into_iter().map(|g| (g.app_name, g.platform)));
        }
        let legendary = "legendaryConfig/legendary/installed.json";
        if let Some(epic) = self.json::<HashMap<String, LegendaryInstall>>(&dir.join(legendary)) {
            installed.extend(epic.into_iter().map(|(app, i)| (app, i.platform)));
        }
        if let Some(nile) =
            self.json::<Vec<NileInstall>>(&dir.join("nile_config/nile/installed.json"))
        {
            // Amazon Games only has Windows games.
            installed.extend(nile.into_iter().map(|n| (n.id, "windows".to_owned())));
        }
        for (app, platform) in installed {
            apps.entry(app).or_default().platform = Some(platform);
        }
        for info in apps.values_mut() {
            if let Some(platform) = &mut info.platform {
                platform.make_ascii_lowercase();
            }
        }
        apps
    }

    /// Reads Lutris' `games/*.yml`. The prefix is `game.prefix`, else the prefix around
    /// `game.exe`; a relative path depends on Lutris' game directory, which only its database has.
    fn lutris(&mut self, home: &Path, dir: &Path) {
        let absolute = |path: &str| Some(expand_home(home, path)).filter(|p| p.is_absolute());
        for file in self.entries(&dir.join("games")) {
            if !matches!(
                file.extension().and_then(OsStr::to_str),
                Some("yml" | "yaml")
            ) {
                continue;
            }
            let Some(stem) = file.file_stem().and_then(OsStr::to_str) else {
                continue;
            };
            let Some(spec) = self.yaml::<LutrisGame>(&file) else {
                continue;
            };
            // Lutris treats an empty prefix as unset.
            let prefix = match spec.game.prefix.filter(|p| !p.is_empty()) {
                Some(prefix) => absolute(&prefix),
                None => spec
                    .game
                    .exe
                    .as_deref()
                    .and_then(absolute)
                    .and_then(|exe| enclosing_prefix(&exe)),
            };
            let Some(prefix) = prefix else {
                continue;
            };
            let service_id = |service| {
                spec.service_id
                    .as_deref()
                    .filter(|_| spec.service.as_deref() == Some(service))
            };
            let slug = spec.game_slug.unwrap_or_else(|| file_slug(stem).to_owned());
            let game = LauncherGame {
                gog_id: service_id("gog").and_then(|id| id.parse().ok()),
                steam_id: service_id("steam").and_then(|id| id.parse().ok()),
                title: spec.name,
                lutris_slug: Some(slug.clone()),
            };
            self.push(Launcher::Lutris, &slug, [prefix], Some(game));
        }
    }

    /// Lists Bottles' bottles; each is a prefix and is not tied to one game. A bottle kept
    /// elsewhere has its path in `placeholder.yml`, or in its `bottle.yml` with `Custom_Path`.
    /// `var_data` is where the Flatpak's `/var/data` is on the host.
    fn bottles(&mut self, dir: &Path, var_data: Option<&Path>) {
        let host_path = |path: &str| match (var_data, Path::new(path).strip_prefix("/var/data")) {
            (Some(var_data), Ok(rest)) => var_data.join(rest),
            _ => PathBuf::from(path),
        };
        for bottle in self.entries(dir) {
            match fs::metadata(&bottle) {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => continue,
                // A dangling link, like a missing launcher, is not a problem.
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => {
                    self.unreadable(&bottle, e.to_string());
                    continue;
                }
            }
            // The bottle's name, not the last part of a placeholder's or `Custom_Path`'s target.
            let name = (bottle.file_name().and_then(OsStr::to_str))
                .unwrap_or_default()
                .to_owned();
            let placeholder = self
                .yaml::<BottlePlaceholder>(&bottle.join("placeholder.yml"))
                .filter(|placeholder| Path::new(&placeholder.path).is_absolute());
            let path = if let Some(placeholder) = placeholder {
                host_path(&placeholder.path)
            } else if self.is_prefix(&bottle) {
                bottle
            } else {
                match self.yaml::<BottleConfig>(&bottle.join("bottle.yml")) {
                    Some(config) if config.custom_path && Path::new(&config.path).is_absolute() => {
                        host_path(&config.path)
                    }
                    _ => continue,
                }
            };
            self.push(Launcher::Bottles, &name, [path], None);
        }
    }

    /// Keeps the first of `candidates` that is a valid prefix, named `name` by `launcher`.
    fn push(
        &mut self,
        launcher: Launcher,
        name: &str,
        candidates: impl IntoIterator<Item = PathBuf>,
        game: Option<LauncherGame>,
    ) {
        if let Some(path) = candidates.into_iter().find(|p| self.is_prefix(p)) {
            self.prefixes.push(LauncherPrefix {
                launcher,
                path,
                game,
                name: name.to_owned(),
            });
        }
    }

    /// Whether `path` is a valid prefix; a failure to check it is also a problem.
    fn is_prefix(&mut self, path: &Path) -> bool {
        match validate_prefix(path) {
            Ok(()) => true,
            Err(PrefixError::NotAPrefix(_)) => false,
            Err(err) => {
                self.problems.push(err.into());
                false
            }
        }
    }

    fn json<T: DeserializeOwned>(&mut self, path: &Path) -> Option<T> {
        self.load(path, |text| serde_json::from_str(text))
    }

    fn yaml<T: DeserializeOwned>(&mut self, path: &Path) -> Option<T> {
        // Lutris writes a `null:` key into the file of a Steam game.
        let options = serde_saphyr::options! { null_as_string: true };
        self.load(path, |text| {
            serde_saphyr::from_str_with_options(text, options)
        })
    }

    /// Reads and parses a file. A missing file is `None`; any other failure is also a problem.
    fn load<T, E: ToString>(
        &mut self,
        path: &Path,
        parse: impl FnOnce(&str) -> Result<T, E>,
    ) -> Option<T> {
        let parsed = match fs::read_to_string(path) {
            Ok(text) => parse(&text).map_err(|e| e.to_string()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
            Err(e) => Err(e.to_string()),
        };
        parsed.map_err(|e| self.unreadable(path, e)).ok()
    }

    /// The sorted entries of a directory. A missing directory has none.
    fn entries(&mut self, dir: &Path) -> Vec<PathBuf> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => {
                self.unreadable(dir, e.to_string());
                return Vec::new();
            }
        };
        let mut paths = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => paths.push(entry.path()),
                Err(e) => self.unreadable(dir, e.to_string()),
            }
        }
        paths.sort();
        paths
    }

    fn unreadable(&mut self, path: &Path, error: String) {
        self.problems.push(ScanProblem {
            path: path.to_path_buf(),
            kind: ProblemKind::Unreadable(error),
        });
    }
}

/// The games of a manifest by the ids and titles launchers report, to map a launcher's prefix
/// to the game it runs (ADR-0017).
pub struct GameIndex<'a> {
    gog: HashMap<u64, Vec<&'a str>>,
    steam: HashMap<u32, Vec<&'a str>>,
    lutris: HashMap<&'a str, Vec<&'a str>>,
    titles: HashSet<&'a str>,
    normalized: HashMap<String, Vec<&'a str>>,
}

impl<'a> GameIndex<'a> {
    /// Indexes every game of `manifest` by its GOG and Steam ids (with the extra ones), its
    /// Lutris slug, its title and its [`normalize_title`].
    pub fn new(manifest: &'a Manifest) -> Self {
        let mut index = Self {
            gog: HashMap::new(),
            steam: HashMap::new(),
            lutris: HashMap::new(),
            titles: HashSet::new(),
            normalized: HashMap::new(),
        };
        for (title, entry) in &manifest.games {
            let title = title.as_str();
            let ids = entry.id.as_ref();
            let gog = entry.gog.as_ref().and_then(|gog| gog.id);
            let gog_extra = ids
                .into_iter()
                .flat_map(|ids| ids.gog_extra.iter().copied());
            for id in gog.into_iter().chain(gog_extra) {
                index.gog.entry(id).or_default().push(title);
            }
            let steam = entry.steam.as_ref().and_then(|steam| steam.id);
            let steam_extra = ids
                .into_iter()
                .flat_map(|ids| ids.steam_extra.iter().copied());
            for id in steam.into_iter().chain(steam_extra) {
                index.steam.entry(id).or_default().push(title);
            }
            if let Some(slug) = ids.and_then(|ids| ids.lutris.as_deref()) {
                index.lutris.entry(slug).or_default().push(title);
            }
            index.titles.insert(title);
            let normalized = index.normalized.entry(normalize_title(title)).or_default();
            normalized.push(title);
        }
        index
    }

    /// The title of the game run in `prefix`: the one game its ids match, else the game of its
    /// exact title, else the one game of its normalized title. `None` when the launcher ties no
    /// game to the prefix, nothing matches, or the normalized title matches several games.
    pub fn title(&self, prefix: &LauncherPrefix) -> Option<&'a str> {
        let game = prefix.game.as_ref()?;
        let gog = game.gog_id.and_then(|id| self.gog.get(&id));
        let steam = game.steam_id.and_then(|id| self.steam.get(&id));
        let lutris = (game.lutris_slug.as_deref()).and_then(|slug| self.lutris.get(slug));
        let by_id: BTreeSet<&'a str> = [gog, steam, lutris]
            .into_iter()
            .flatten()
            .flatten()
            .copied()
            .collect();
        // Ids of several games say nothing about which one runs here, so the title decides.
        if by_id.len() == 1 {
            return by_id.first().copied();
        }
        let title = game.title.as_deref()?;
        if let Some(&exact) = self.titles.get(title) {
            return Some(exact);
        }
        match self
            .normalized
            .get(&normalize_title(title))
            .map(Vec::as_slice)
        {
            Some(&[title]) => Some(title),
            _ => None,
        }
    }
}

/// Expands a leading `~` to `home`.
fn expand_home(home: &Path, path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some("") => home.to_path_buf(),
        Some(rest) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

/// The game slug in a Lutris game file name, `<slug>-<unix time>`.
fn file_slug(stem: &str) -> &str {
    match stem.rsplit_once('-') {
        Some((slug, time)) if !time.is_empty() && time.bytes().all(|b| b.is_ascii_digit()) => slug,
        _ => stem,
    }
}

/// Symbols that separate a title's parts; they become spaces.
const GAP_SYMBOLS: &[char] = &['™', '®', '©', ':', '-'];

/// Quotes that are dropped from a title.
const NO_GAP_SYMBOLS: &[char] = &['\'', '"', '‘', '’', '“', '”'];

/// Normalizes a game title for matching, after ludusavi's `normalize_title`: lowercase, no
/// trailing ` (<year>)`, no `… edition` suffix, `™®©:-` as spaces, no quotes, single spaces.
/// Unlike ludusavi's, the year only matches ASCII digits.
pub fn normalize_title(title: &str) -> String {
    let mut title = title.to_lowercase();
    strip_year(&mut title);
    strip_punctuated_edition(&mut title);
    if let Some(stripped) = title.strip_suffix(" game of the year edition") {
        title.truncate(stripped.len());
    }
    strip_short_edition(&mut title);
    let title: String = title
        .chars()
        .filter(|c| !NO_GAP_SYMBOLS.contains(c))
        .map(|c| if GAP_SYMBOLS.contains(&c) { ' ' } else { c })
        .collect();
    title
        .split(' ')
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

/// Drops a trailing ` (<digits>)`.
fn strip_year(title: &mut String) {
    let Some(inner) = title.strip_suffix(')') else {
        return;
    };
    if let Some(start) = inner.rfind(" (")
        && let digits = &inner[start + 2..]
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
    {
        title.truncate(start);
    }
}

/// Drops `<symbol> <words> edition` from the first `™®©:-` that starts one.
fn strip_punctuated_edition(title: &mut String) {
    let Some(body) = title.strip_suffix(" edition") else {
        return;
    };
    let cut = body.char_indices().find(|&(i, c)| {
        let rest = &body[i + c.len_utf8()..];
        GAP_SYMBOLS.contains(&c) && rest.len() > 1 && rest.starts_with(' ')
    });
    if let Some((i, _)) = cut {
        title.truncate(i);
    }
}

/// Drops a one-word ` <word> edition`; a longer edition name may be part of the title.
fn strip_short_edition(title: &mut String) {
    let Some(body) = title.strip_suffix(" edition") else {
        return;
    };
    if let Some(space) = body.rfind(' ')
        && space + 1 < body.len()
    {
        title.truncate(space);
    }
}

#[cfg(test)]
mod tests;
