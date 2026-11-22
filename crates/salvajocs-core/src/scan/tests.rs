use std::time::Instant;

use super::*;
use crate::config::{ListSort, PasswordMode, ThemeChoice};
use crate::launcher::{Launcher, LauncherGame};
use crate::layout::{LinuxFolders, glob_pattern};
use crate::manifest::{CustomFile, CustomGame, Integration, parse_yaml};

/// Creates a valid prefix at `root/pfx` with a `steamuser` home and returns its path.
fn make_prefix(root: &Path) -> PathBuf {
    let prefix = root.join("pfx");
    fs::create_dir_all(prefix.join("drive_c/users/steamuser")).unwrap();
    fs::write(
        prefix.join("user.reg"),
        "WINE REGISTRY Version 2\n\n#arch=win64\n",
    )
    .unwrap();
    prefix
}

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn config(roots: Vec<Root>, custom_games: Vec<CustomGame>) -> AppConfig {
    AppConfig {
        repository: String::new(),
        repository_options: Vec::new(),
        password: PasswordMode::None,
        roots,
        custom_games,
        check_updates: true,
        ignored_prefixes: Vec::new(),
        theme: ThemeChoice::System,
        backup_sort: ListSort::default(),
        restore_sort: ListSort::default(),
        custom_games_repository: None,
        custom_games_unsent: Vec::new(),
        retention_unsent: None,
    }
}

fn wine_root(path: &Path) -> Root {
    Root {
        path: path.to_path_buf(),
        kind: RootKind::WinePrefix,
    }
}

fn linux_env(home: &Path) -> Environment {
    Environment {
        native: Some(Context::Linux(LinuxFolders {
            user: "me".into(),
            home: home.to_path_buf(),
            xdg_data: home.join(".local/share"),
            xdg_config: home.join(".config"),
        })),
        steam_roots: Vec::new(),
        launcher_prefixes: Vec::new(),
        launcher_problems: Vec::new(),
    }
}

fn no_native_env(steam_roots: Vec<PathBuf>) -> Environment {
    Environment {
        native: None,
        steam_roots,
        launcher_prefixes: Vec::new(),
        launcher_problems: Vec::new(),
    }
}

fn game(manifest: &Manifest, config: &AppConfig, env: &Environment) -> GameScan {
    let mut report = scan(manifest, config, env, |_, _| {});
    assert_eq!(report.games.len(), 1, "{report:?}");
    assert_eq!(report.problems, []);
    report.games.remove(0)
}

fn tails(scan: &GameScan) -> Vec<(Base, &str)> {
    scan.files
        .iter()
        .map(|file| (file.base, file.tail.as_str()))
        .collect()
}

#[test]
fn proton_game_files_get_win_bases() {
    let steam = tempfile::tempdir().unwrap();
    let pfx = make_prefix(&steam.path().join("steamapps/compatdata/620"));
    let home = pfx.join("drive_c/users/steamuser");
    write(&home.join("AppData/Roaming/Foo/save.dat"), "12345");
    write(&home.join("Documents/My Games/Foo/a.sav"), "1");
    write(&home.join("AppData/LocalLow/Co/Foo/b.sav"), "1");
    let manifest = parse_yaml(
        "Foo:\n  steam: {id: 620}\n  files:\n    <winAppData>/Foo: {}\n    \
         <winDocuments>/My Games/Foo: {}\n    <home>/AppData/LocalLow/Co/Foo: {}\n",
    )
    .unwrap();
    let env = no_native_env(vec![steam.path().to_path_buf()]);
    let found = game(&manifest, &config(vec![], vec![]), &env);
    assert_eq!(found.title, "Foo");
    let mut got = tails(&found);
    got.sort();
    assert_eq!(
        got,
        [
            (Base::WinDocuments, "My Games/Foo/a.sav"),
            (Base::WinAppData, "Foo/save.dat"),
            (Base::WinLocalAppDataLow, "Co/Foo/b.sav"),
        ]
    );
    let save = found
        .files
        .iter()
        .find(|f| f.tail == "Foo/save.dat")
        .unwrap();
    assert_eq!(save.real, home.join("AppData/Roaming/Foo/save.dat"));
    assert_eq!(save.size, 5);
    assert_eq!(
        save.modified,
        Some(fs::metadata(&save.real).unwrap().modified().unwrap())
    );
}

#[test]
fn native_linux_game_files_get_linux_bases() {
    let home = tempfile::tempdir().unwrap();
    write(&home.path().join(".config/foo/settings.ini"), "a");
    write(&home.path().join(".local/share/foo/save1.dat"), "bb");
    write(&home.path().join("winonly/x.dat"), "c");
    let manifest = parse_yaml(
        "Foo:\n  files:\n    <xdgConfig>/foo: {}\n    <home>/.local/share/foo: {}\n    \
         <home>/winonly: {when: [{os: windows}]}\n    <winAppData>/Foo: {}\n",
    )
    .unwrap();
    let found = game(&manifest, &config(vec![], vec![]), &linux_env(home.path()));
    let mut got = tails(&found);
    got.sort();
    assert_eq!(
        got,
        [
            (Base::XdgData, "foo/save1.dat"),
            (Base::XdgConfig, "foo/settings.ini"),
        ]
    );
}

#[test]
fn same_file_from_two_templates_listed_once() {
    let home = tempfile::tempdir().unwrap();
    write(&home.path().join(".config/foo/save.dat"), "a");
    let manifest = parse_yaml(
        "Foo:\n  files:\n    <home>/.config/foo/save.dat: {}\n    <xdgConfig>/foo/*.dat: {}\n",
    )
    .unwrap();
    let found = game(&manifest, &config(vec![], vec![]), &linux_env(home.path()));
    assert_eq!(tails(&found), [(Base::XdgConfig, "foo/save.dat")]);
}

#[test]
fn game_specific_prefix_beats_wine_prefix_root() {
    let steam = tempfile::tempdir().unwrap();
    let own = make_prefix(&steam.path().join("steamapps/compatdata/620"));
    let other = make_prefix(&steam.path().join("elsewhere"));
    let relative = "drive_c/users/steamuser/AppData/Roaming/Foo/save.dat";
    write(&own.join(relative), "own");
    write(&other.join(relative), "other");
    let manifest =
        parse_yaml("Foo:\n  steam: {id: 620}\n  files:\n    <winAppData>/Foo: {}\n").unwrap();
    let env = no_native_env(vec![steam.path().to_path_buf()]);
    // The root is configured first, so only the game-specific rule can put `own` ahead.
    let config = config(vec![wine_root(&other)], vec![]);
    let found = game(&manifest, &config, &env);
    assert_eq!(found.files.len(), 1);
    assert_eq!(found.files[0].real, own.join(relative));
}

/// A launcher prefix at `path`, of the game titled `title` or, when `None`, of no game.
fn launcher_prefix(path: &Path, title: Option<&str>) -> LauncherPrefix {
    LauncherPrefix {
        launcher: Launcher::Heroic,
        path: path.to_path_buf(),
        game: title.map(|title| LauncherGame {
            title: Some(title.to_owned()),
            ..LauncherGame::default()
        }),
        name: String::new(),
    }
}

/// An environment without a native context that found `prefixes`.
fn launcher_env(prefixes: Vec<LauncherPrefix>) -> Environment {
    Environment {
        launcher_prefixes: prefixes,
        ..no_native_env(vec![])
    }
}

const FOO_SAVE: &str = "drive_c/users/steamuser/AppData/Roaming/Foo/save.dat";
const FOO: &str = "Foo:\n  files:\n    <winAppData>/Foo: {}\n";

#[test]
fn launcher_prefix_of_the_game_beats_wine_prefix_root() {
    let dir = tempfile::tempdir().unwrap();
    let own = make_prefix(&dir.path().join("own"));
    let root = make_prefix(&dir.path().join("root"));
    write(&own.join(FOO_SAVE), "own");
    write(&root.join(FOO_SAVE), "root");
    let manifest = parse_yaml(FOO).unwrap();
    let env = launcher_env(vec![launcher_prefix(&own, Some("Foo™"))]);
    let found = game(&manifest, &config(vec![wine_root(&root)], vec![]), &env);
    assert_eq!(found.files.len(), 1);
    assert_eq!(found.files[0].real, own.join(FOO_SAVE));
}

#[test]
fn proton_prefix_beats_launcher_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let steam = dir.path().join("steam");
    let proton = make_prefix(&steam.join("steamapps/compatdata/620"));
    let own = make_prefix(&dir.path().join("own"));
    write(&proton.join(FOO_SAVE), "proton");
    write(&own.join(FOO_SAVE), "own");
    let manifest = parse_yaml(&format!("{FOO}  steam: {{id: 620}}\n")).unwrap();
    let env = Environment {
        launcher_prefixes: vec![launcher_prefix(&own, Some("Foo"))],
        ..no_native_env(vec![steam])
    };
    let found = game(&manifest, &config(vec![], vec![]), &env);
    assert_eq!(found.files.len(), 1);
    assert_eq!(found.files[0].real, proton.join(FOO_SAVE));
}

#[test]
fn custom_wine_prefix_beats_launcher_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let custom = make_prefix(&dir.path().join("custom"));
    let own = make_prefix(&dir.path().join("own"));
    write(&custom.join(FOO_SAVE), "custom");
    write(&own.join(FOO_SAVE), "own");
    let mut game_config = custom_game("Foo", "<winAppData>/Foo", Integration::Extend);
    game_config.wine_prefix = Some(custom.clone());
    let manifest = parse_yaml(FOO).unwrap();
    let env = launcher_env(vec![launcher_prefix(&own, Some("Foo"))]);
    let found = game(&manifest, &config(vec![], vec![game_config]), &env);
    assert_eq!(found.files.len(), 1);
    assert_eq!(found.files[0].real, custom.join(FOO_SAVE));
}

#[test]
fn wine_prefix_root_beats_shared_launcher_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let root = make_prefix(&dir.path().join("root"));
    let shared = make_prefix(&dir.path().join("shared"));
    write(&root.join(FOO_SAVE), "root");
    write(&shared.join(FOO_SAVE), "shared");
    let bar_save = "drive_c/users/steamuser/AppData/Roaming/Bar/save.dat";
    write(&shared.join(bar_save), "bar");
    let manifest = parse_yaml(&format!(
        "{FOO}Bar:\n  files:\n    <winAppData>/Bar: {{}}\n"
    ));
    // An unmapped prefix is scanned for every game, after the configured roots.
    let env = launcher_env(vec![launcher_prefix(&shared, Some("Unknown"))]);
    let report = scan(
        &manifest.unwrap(),
        &config(vec![wine_root(&root)], vec![]),
        &env,
        |_, _| {},
    );
    assert_eq!(report.problems, []);
    let real: Vec<&Path> = (report.games.iter())
        .flat_map(|game| game.files.iter().map(|file| file.real.as_path()))
        .collect();
    assert_eq!(real, [shared.join(bar_save), root.join(FOO_SAVE)]);
}

#[test]
fn launcher_prefix_of_another_game_is_not_scanned() {
    let dir = tempfile::tempdir().unwrap();
    let bar = make_prefix(dir.path());
    write(&bar.join(FOO_SAVE), "foo");
    let manifest = parse_yaml(&format!("{FOO}Bar: {{}}\n")).unwrap();
    let env = launcher_env(vec![launcher_prefix(&bar, Some("Bar"))]);
    let report = scan(&manifest, &config(vec![], vec![]), &env, |_, _| {});
    assert_eq!(report.games, []);
    assert_eq!(report.problems, []);
}

#[test]
fn ignored_launcher_prefix_is_not_scanned() {
    let dir = tempfile::tempdir().unwrap();
    let own = make_prefix(dir.path());
    write(&own.join(FOO_SAVE), "own");
    let manifest = parse_yaml(FOO).unwrap();
    let env = launcher_env(vec![launcher_prefix(&own, Some("Foo"))]);
    let mut config = config(vec![], vec![]);
    config.ignored_prefixes = vec![own];
    let report = scan(&manifest, &config, &env, |_, _| {});
    assert_eq!(report.games, []);
}

/// A prefix of `launcher` at `path`, which it names `name`, of the game titled `title`.
fn named_prefix(
    launcher: Launcher,
    path: &Path,
    title: Option<&str>,
    name: &str,
) -> LauncherPrefix {
    let mut prefix = launcher_prefix(path, title);
    prefix.launcher = launcher;
    prefix.name = name.to_owned();
    prefix
}

#[test]
fn a_shared_bottle_is_recorded_by_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let bottle = make_prefix(&dir.path().join("Drive"));
    write(&bottle.join(FOO_SAVE), "foo");
    let manifest = parse_yaml(FOO).unwrap();
    let bottles = named_prefix(Launcher::Bottles, &bottle, None, "Generic-Game");
    let found = game(
        &manifest,
        &config(vec![], vec![]),
        &launcher_env(vec![bottles]),
    );
    let origin = PrefixOrigin::Bottles("Generic-Game".into());
    assert_eq!(found.prefix_origin, Some(origin));
}

#[test]
fn heroics_shared_default_prefix_is_recorded_by_this_games_app() {
    let dir = tempfile::tempdir().unwrap();
    let shared = make_prefix(dir.path());
    write(&shared.join(FOO_SAVE), "foo");
    let manifest = parse_yaml(&format!("{FOO}Bar: {{}}\n")).unwrap();
    let env = launcher_env(vec![
        named_prefix(Launcher::Heroic, &shared, Some("Bar"), "BarApp"),
        named_prefix(Launcher::Heroic, &shared, Some("Foo"), "FooApp"),
    ]);
    let found = game(&manifest, &config(vec![], vec![]), &env);
    assert_eq!(
        found.prefix_origin,
        Some(PrefixOrigin::Heroic("FooApp".into()))
    );
}

#[test]
fn a_proton_prefix_is_recorded_by_its_app_id() {
    let steam = tempfile::tempdir().unwrap();
    let pfx = make_prefix(&steam.path().join("steamapps/compatdata/620"));
    write(&pfx.join(FOO_SAVE), "foo");
    let manifest = parse_yaml(&format!("{FOO}  steam: {{id: 620}}\n")).unwrap();
    let env = no_native_env(vec![steam.path().to_path_buf()]);
    let found = game(&manifest, &config(vec![], vec![]), &env);
    assert_eq!(found.prefix_origin, Some(PrefixOrigin::Proton(620)));
}

#[test]
fn a_wine_prefix_root_below_home_is_recorded_from_home() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let pfx = make_prefix(&home.join("Games/wine"));
    write(&pfx.join(FOO_SAVE), "foo");
    let manifest = parse_yaml(FOO).unwrap();
    let config = config(vec![wine_root(&pfx)], vec![]);
    let found = game(&manifest, &config, &linux_env(&home));
    let origin = PrefixOrigin::Home("<home>/Games/wine/pfx".into());
    assert_eq!(found.prefix_origin, Some(origin));
}

#[test]
fn a_prefix_outside_home_is_not_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let pfx = make_prefix(&dir.path().join("elsewhere"));
    write(&pfx.join(FOO_SAVE), "foo");
    let manifest = parse_yaml(FOO).unwrap();
    let config = config(vec![wine_root(&pfx)], vec![]);
    let found = game(&manifest, &config, &linux_env(&dir.path().join("home")));
    assert_eq!(found.prefixes, [pfx]);
    assert_eq!(found.prefix_origin, None);
}

#[test]
fn a_native_game_records_no_prefix() {
    let home = tempfile::tempdir().unwrap();
    write(&home.path().join(".local/share/foo/save.dat"), "a");
    let manifest = parse_yaml("Foo:\n  files:\n    <xdgData>/foo: {}\n").unwrap();
    let found = game(&manifest, &config(vec![], vec![]), &linux_env(home.path()));
    assert_eq!(found.prefix_origin, None);
}

#[test]
fn an_ignored_bottle_used_as_a_wine_prefix_root_is_recorded_from_home() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let bottle = make_prefix(&home.join("Bottle"));
    write(&bottle.join(FOO_SAVE), "foo");
    let manifest = parse_yaml(FOO).unwrap();
    let mut config = config(vec![wine_root(&bottle)], vec![]);
    config.ignored_prefixes = vec![bottle.clone()];
    let env = Environment {
        launcher_prefixes: vec![named_prefix(Launcher::Bottles, &bottle, None, "Bottle")],
        ..linux_env(&home)
    };
    let found = game(&manifest, &config, &env);
    let origin = PrefixOrigin::Home("<home>/Bottle/pfx".into());
    assert_eq!(found.prefix_origin, Some(origin));
}

#[test]
fn progress_counts_every_game_with_the_custom_ones() {
    let manifest = parse_yaml("Bar: {}\nFoo: {}\n").unwrap();
    let custom = custom_game("Baz", "<home>/baz", Integration::Override);
    let mut calls = Vec::new();
    scan(
        &manifest,
        &config(vec![], vec![custom]),
        &no_native_env(vec![]),
        |done, total| calls.push((done, total)),
    );
    assert_eq!(calls, [(1, 3), (2, 3), (3, 3)]);
}

#[test]
fn launcher_problems_are_reported_once() {
    let problem = ScanProblem {
        path: PathBuf::from("/launcher/games/foo.yml"),
        kind: ProblemKind::Unreadable("bad".into()),
    };
    let env = Environment {
        launcher_problems: vec![problem.clone(), problem.clone()],
        ..no_native_env(vec![])
    };
    let report = scan(
        &Manifest::default(),
        &config(vec![], vec![]),
        &env,
        |_, _| {},
    );
    assert_eq!(report.problems, [problem]);
}

#[test]
fn prefix_scan_matches_case_insensitively() {
    let dir = tempfile::tempdir().unwrap();
    let pfx = make_prefix(dir.path());
    let save = pfx.join("drive_c/users/steamuser/Documents/my games/foo/Save.dat");
    write(&save, "x");
    let manifest = parse_yaml("Foo:\n  files:\n    <winDocuments>/My Games/Foo: {}\n").unwrap();
    let config = config(vec![wine_root(&pfx)], vec![]);
    let found = game(&manifest, &config, &no_native_env(vec![]));
    assert_eq!(found.files.len(), 1);
    assert_eq!(found.files[0].base, Base::WinDocuments);
    assert_eq!(found.files[0].tail, "my games/foo/Save.dat");
    assert_eq!(found.files[0].real, save);
}

/// A native Windows environment whose folders are those of `prefix`'s `me` user.
fn windows_env(prefix: &Path) -> Environment {
    Environment {
        native: Some(Context::WindowsNative(WinFolders::for_prefix(prefix, "me"))),
        ..no_native_env(vec![])
    }
}

#[test]
fn native_windows_scan_matches_wildcards_case_insensitively() {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("drive_c/users/me/Documents");
    write(&docs.join("Ace/SG_PC-1.sav"), "x");
    let manifest = parse_yaml("Foo:\n  files:\n    <winDocuments>/Ace/SG_PC-*.SAV: {}\n").unwrap();
    let found = game(&manifest, &config(vec![], vec![]), &windows_env(dir.path()));
    assert_eq!(tails(&found), [(Base::WinDocuments, "Ace/SG_PC-1.sav")]);
}

#[test]
fn roots_record_glob_free_prefixes() {
    let home = tempfile::tempdir().unwrap();
    write(&home.path().join(".config/foo/slot1/save.dat"), "a");
    write(&home.path().join(".local/share/bar/one.sav"), "a");
    write(&home.path().join(".local/share/Foo/saves/a"), "a");
    write(&home.path().join(".config/none/x.dat"), "a");
    let manifest = parse_yaml(
        "Foo:\n  files:\n    <xdgConfig>/foo/*/save.dat: {}\n    <home>/.local/share/bar: {}\n    \
         <xdgData>/<game>/saves: {}\n    <xdgConfig>/missing/*/x: {}\n",
    )
    .unwrap();
    let found = game(&manifest, &config(vec![], vec![]), &linux_env(home.path()));
    let mut roots = found.roots;
    roots.sort();
    assert_eq!(
        roots,
        [
            (Base::XdgData, "<game>/saves".to_owned()),
            (Base::XdgData, "bar".to_owned()),
            (Base::XdgConfig, "foo".to_owned())
        ]
    );
}

#[test]
fn registry_keys_are_hkcu_in_backslash_form() {
    let home = tempfile::tempdir().unwrap();
    write(&home.path().join(".config/foo/a"), "a");
    let manifest = parse_yaml(
        "Foo:\n  files:\n    <xdgConfig>/foo: {}\n  registry:\n    HKEY_CURRENT_USER/Software/Foo: {}\n    \
         HKEY_LOCAL_MACHINE/Software/Foo: {}\n    HKEY_CURRENT_USER: {}\n",
    )
    .unwrap();
    let found = game(&manifest, &config(vec![], vec![]), &linux_env(home.path()));
    assert_eq!(found.registry_keys, [r"HKEY_CURRENT_USER\Software\Foo"]);
}

#[test]
fn custom_game_with_wine_prefix_is_scanned() {
    let dir = tempfile::tempdir().unwrap();
    let pfx = make_prefix(dir.path());
    write(
        &pfx.join("drive_c/users/steamuser/AppData/Roaming/Mine/s.dat"),
        "a",
    );
    let custom = CustomGame {
        name: "Mine".into(),
        files: Vec::new(),
        entry: parse_yaml("Mine:\n  files:\n    <winAppData>/Mine: {}\n")
            .unwrap()
            .games
            .remove("Mine")
            .unwrap(),
        integration: Integration::Override,
        wine_prefix: Some(pfx),
        pairs: Vec::new(),
        skips: Vec::new(),
    };
    let found = game(
        &Manifest::default(),
        &config(vec![], vec![custom]),
        &no_native_env(vec![]),
    );
    assert_eq!(found.title, "Mine");
    assert_eq!(tails(&found), [(Base::WinAppData, "Mine/s.dat")]);
}

/// Writes a `user.reg` holding one valueful key per entry of `keys` (`Software\Foo` form).
fn write_user_reg(prefix: &Path, keys: &[&str]) {
    let mut text = String::from("WINE REGISTRY Version 2\n\n#arch=win64\n");
    for key in keys {
        let key = key.replace('\\', "\\\\");
        text.push_str(&format!(
            "\n[{key}] 1700000000\n#time=1d9\n\"Level\"=dword:00000005\n"
        ));
    }
    fs::write(prefix.join("user.reg"), text).unwrap();
}

fn custom_game(name: &str, files: &str, integration: Integration) -> CustomGame {
    let yaml = format!("{name}:\n  files:\n    {files}: {{}}\n");
    CustomGame {
        name: name.into(),
        files: Vec::new(),
        entry: parse_yaml(&yaml).unwrap().games.remove(name).unwrap(),
        integration,
        wine_prefix: None,
        pairs: Vec::new(),
        skips: Vec::new(),
    }
}

const REGISTRY_ONLY: &str = "Foo:\n  registry:\n    HKEY_CURRENT_USER/Software/Foo: {}\n";

#[test]
fn registry_only_game_is_offered() {
    let dir = tempfile::tempdir().unwrap();
    let pfx = make_prefix(dir.path());
    write_user_reg(&pfx, &["Software\\Foo\\Sub"]);
    let manifest = parse_yaml(REGISTRY_ONLY).unwrap();
    let config = config(vec![wine_root(&pfx)], vec![]);
    let found = game(&manifest, &config, &no_native_env(vec![]));
    assert!(found.files.is_empty());
    assert_eq!(found.registry_keys, [r"HKEY_CURRENT_USER\Software\Foo"]);
    assert_eq!(found.registry_source, Some(RegistrySource::WinePrefix(pfx)));
}

#[test]
fn prefixes_are_those_of_windows_files_then_the_registry() {
    let steam = tempfile::tempdir().unwrap();
    // The game's own Proton prefix, looked in first, holds none of its files.
    make_prefix(&steam.path().join("steamapps/compatdata/620"));
    let other = make_prefix(&steam.path().join("other"));
    let third = make_prefix(&steam.path().join("third"));
    // Found while looking in the own prefix, but not inside it.
    write(&steam.path().join("Foo.cfg"), "root");
    write(
        &other.join("drive_c/users/steamuser/AppData/Roaming/Foo/a.dat"),
        "a",
    );
    write_user_reg(&third, &["Software\\Foo"]);
    let manifest = parse_yaml(
        "Foo:\n  steam: {id: 620}\n  files:\n    <winAppData>/Foo: {}\n    <root>/Foo.cfg: {}\n  \
         registry:\n    HKEY_CURRENT_USER/Software/Foo: {}\n",
    )
    .unwrap();
    let env = no_native_env(vec![steam.path().to_path_buf()]);
    let config = config(vec![wine_root(&other), wine_root(&third)], vec![]);

    let found = game(&manifest, &config, &env);

    assert_eq!(found.files.len(), 2);
    assert_eq!(found.prefixes, [other, third]);
}

#[test]
fn registry_source_is_first_context_with_keys() {
    let steam = tempfile::tempdir().unwrap();
    let own = make_prefix(&steam.path().join("steamapps/compatdata/620"));
    let first = make_prefix(&steam.path().join("first"));
    let second = make_prefix(&steam.path().join("second"));
    write_user_reg(&first, &["Software\\Foo"]);
    write_user_reg(&second, &["Software\\Foo"]);
    let manifest = parse_yaml(
        "Foo:\n  steam: {id: 620}\n  registry:\n    HKEY_CURRENT_USER/Software/Foo: {}\n",
    )
    .unwrap();
    let env = no_native_env(vec![steam.path().to_path_buf()]);
    let config = config(vec![wine_root(&first), wine_root(&second)], vec![]);
    // The game's own prefix has no key, so the first root that has one is the source.
    let found = game(&manifest, &config, &env);
    assert_eq!(
        found.registry_source,
        Some(RegistrySource::WinePrefix(first))
    );
    write_user_reg(&own, &["Software\\Foo"]);
    let found = game(&manifest, &config, &env);
    assert_eq!(found.registry_source, Some(RegistrySource::WinePrefix(own)));
}

#[test]
fn user_reg_parsed_once_per_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let pfx = make_prefix(dir.path());
    write_user_reg(&pfx, &["Software\\Foo"]);
    let cache = Cache::default();
    assert_eq!(
        cache.prefix_has_keys(&pfx, &[r"HKEY_CURRENT_USER\Software\Foo".into()]),
        Ok(true)
    );
    assert_eq!(
        cache.prefix_has_keys(&pfx, &[r"HKEY_CURRENT_USER\Software\Bar".into()]),
        Ok(false)
    );
    assert_eq!(cache.user_reg_loads.get(), 1);
    assert_eq!(cache.problems.into_inner(), []);
}

#[test]
fn a_glob_is_skipped_only_when_its_first_part_matches_no_entry() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("Documents");
    write(&base.join("My Games/Foo/save"), "x");
    fs::create_dir(dir.path().join("Empty")).unwrap();
    let cache = Cache::default();
    let may_match = |base: &Path, rest: &str, folded: bool| {
        cache.may_match(base, &glob_pattern(base, rest, folded), false)
    };
    assert!(may_match(&base, "my games/foo", true));
    assert!(!may_match(&base, "Other Games/Foo", true));
    assert!(may_match(&base, "*/Foo", false));
    assert!(may_match(&base, "**/Foo", false));
    assert!(may_match(&base, "", false));
    assert!(may_match(&base, "My Games/Bar", false));
    assert!(!may_match(&base, "Other Games", false));
    assert!(!may_match(&dir.path().join("Empty"), "*", false));
    assert!(may_match(&dir.path().join("Empty"), ".*/save", true));
    assert!(!may_match(&dir.path().join("Missing"), "foo/*", true));
    assert!(!may_match(&base.join("My Games/Foo/save"), "y*", true));
    let upper = glob_pattern(&base, "MY*/Foo", false);
    assert!(!cache.may_match(&base, &upper, false));
    assert!(cache.may_match(&base, &upper, true));
}

#[cfg(unix)]
#[test]
fn a_glob_below_an_unreadable_dir_still_runs() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let may_match =
        Cache::default().may_match(&locked, &glob_pattern(&locked, "*.dat", true), false);
    let readable = fs::read_dir(&locked).is_ok();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        return; // running as root: permissions do not apply
    }
    assert!(may_match, "glob reports the unreadable dir");
}

#[test]
fn ambiguous_wine_prefix_root_reported() {
    let dir = tempfile::tempdir().unwrap();
    let pfx = make_prefix(dir.path());
    fs::remove_dir(pfx.join("drive_c/users/steamuser")).unwrap();
    for user in ["alice", "bob"] {
        fs::create_dir(pfx.join("drive_c/users").join(user)).unwrap();
    }
    let config = config(vec![wine_root(&pfx)], vec![]);
    let report = scan(
        &Manifest::default(),
        &config,
        &no_native_env(vec![]),
        |_, _| {},
    );
    assert_eq!(report.games, []);
    assert_eq!(
        report.problems,
        [ScanProblem {
            path: pfx,
            kind: ProblemKind::AmbiguousPrefix(vec!["alice".into(), "bob".into()]),
        }]
    );
}

#[cfg(unix)]
#[test]
fn unreadable_dir_reported_not_dropped() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().unwrap();
    let locked = home.path().join(".config/foo/locked");
    write(&home.path().join(".config/foo/ok/a"), "a");
    fs::create_dir_all(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let manifest = parse_yaml(
        "Foo:\n  files:\n    <xdgConfig>/foo/ok: {}\n    <xdgConfig>/foo/locked/*.dat: {}\n",
    )
    .unwrap();
    let report = scan(
        &manifest,
        &config(vec![], vec![]),
        &linux_env(home.path()),
        |_, _| {},
    );
    let readable = fs::read_dir(&locked).is_ok();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        return; // running as root: permissions do not apply
    }
    assert_eq!(report.games.len(), 1);
    assert_eq!(report.games[0].unreadable, std::slice::from_ref(&locked));
    assert_eq!(report.games[0].files.len(), 1);
    assert!(matches!(
        report.problems.as_slice(),
        [ScanProblem { path, kind: ProblemKind::Unreadable(_) }] if *path == locked
    ));
}

#[test]
fn unreadable_user_reg_stops_the_registry_walk() {
    let dir = tempfile::tempdir().unwrap();
    let broken = make_prefix(&dir.path().join("a"));
    let good = make_prefix(&dir.path().join("b"));
    write(
        &broken.join("drive_c/users/steamuser/AppData/Roaming/Foo/s.dat"),
        "a",
    );
    fs::write(broken.join("user.reg"), "not a registry").unwrap();
    write_user_reg(&good, &["Software\\Foo"]);
    let manifest = parse_yaml(
        "Foo:\n  files:\n    <winAppData>/Foo: {}\n  registry:\n    \
         HKEY_CURRENT_USER/Software/Foo: {}\n",
    )
    .unwrap();
    let config = config(vec![wine_root(&broken), wine_root(&good)], vec![]);
    let report = scan(&manifest, &config, &no_native_env(vec![]), |_, _| {});
    let [found] = report.games.as_slice() else {
        panic!("{report:?}");
    };
    // The lower-priority prefix must not stand in for the one that could not be read.
    assert_eq!(found.registry_source, None);
    assert_eq!(found.unreadable, [broken.join("user.reg")]);
    assert_eq!(found.files.len(), 1);
    assert!(matches!(
        report.problems.as_slice(),
        [ScanProblem {
            kind: ProblemKind::Unreadable(_),
            ..
        }]
    ));
}

#[test]
fn os_user_name_comes_from_the_context() {
    let dir = tempfile::tempdir().unwrap();
    let pfx = make_prefix(dir.path());
    let home = tempfile::tempdir().unwrap();
    let prefix_home = pfx.join("drive_c/users/steamuser");
    write(&prefix_home.join("Documents/steamuser/s.dat"), "a");
    write(&prefix_home.join("Documents/me/s.dat"), "a");
    write(&home.path().join(".local/share/me/s.dat"), "a");
    write(&home.path().join(".local/share/steamuser/s.dat"), "a");
    let manifest = parse_yaml(
        "Foo:\n  files:\n    <winDocuments>/<osUserName>: {}\n    <xdgData>/<osUserName>: {}\n",
    )
    .unwrap();
    let config = config(vec![wine_root(&pfx)], vec![]);
    let found = game(&manifest, &config, &linux_env(home.path()));
    let mut got = tails(&found);
    got.sort();
    assert_eq!(
        got,
        [
            (Base::WinDocuments, "steamuser/s.dat"),
            (Base::XdgData, "me/s.dat")
        ]
    );
}

#[test]
fn prefixes_differing_in_case_list_a_logical_path_once() {
    let dir = tempfile::tempdir().unwrap();
    let first = make_prefix(&dir.path().join("a"));
    let second = make_prefix(&dir.path().join("b"));
    write(
        &first.join("drive_c/users/steamuser/Documents/My Games/Foo/s.dat"),
        "a",
    );
    write(
        &second.join("drive_c/users/steamuser/Documents/my games/foo/s.dat"),
        "b",
    );
    let manifest = parse_yaml("Foo:\n  files:\n    <winDocuments>/My Games/Foo: {}\n").unwrap();
    let config = config(vec![wine_root(&first), wine_root(&second)], vec![]);
    let found = game(&manifest, &config, &no_native_env(vec![]));
    assert_eq!(tails(&found), [(Base::WinDocuments, "My Games/Foo/s.dat")]);
}

#[test]
fn extend_custom_game_keeps_the_manifest_files() {
    let home = tempfile::tempdir().unwrap();
    write(&home.path().join(".config/foo/a"), "a");
    write(&home.path().join(".local/share/foo/b"), "b");
    let manifest = parse_yaml("Foo:\n  files:\n    <xdgConfig>/foo: {}\n").unwrap();
    let custom = custom_game("Foo", "<xdgData>/foo", Integration::Extend);
    let config = config(vec![], vec![custom]);
    let found = game(&manifest, &config, &linux_env(home.path()));
    let mut got = tails(&found);
    got.sort();
    assert_eq!(got, [(Base::XdgData, "foo/b"), (Base::XdgConfig, "foo/a")]);
}

fn rows_game(name: &str, linux: Option<&str>, windows: Option<&str>) -> CustomGame {
    CustomGame {
        files: vec![CustomFile {
            linux: linux.map(Into::into),
            windows: windows.map(Into::into),
        }],
        ..custom_game(name, "<base>/unused", Integration::Override)
    }
}

#[test]
fn a_linux_row_is_not_scanned_in_a_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let pfx = make_prefix(dir.path());
    write(&pfx.join("drive_c/users/steamuser/Mine/s.dat"), "a");
    let mut custom = rows_game("Mine", Some("<home>/Mine"), None);
    custom.wine_prefix = Some(pfx);
    let report = scan(
        &Manifest::default(),
        &config(vec![], vec![custom]),
        &no_native_env(vec![]),
        |_, _| {},
    );
    assert_eq!(report.games, []);
}

#[test]
fn a_windows_row_is_found_in_a_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let pfx = make_prefix(dir.path());
    write(&pfx.join("drive_c/users/steamuser/Mine/s.dat"), "a");
    let mut custom = rows_game("Mine", Some("<xdgData>/Mine"), Some("<home>/Mine"));
    custom.wine_prefix = Some(pfx);
    let found = game(
        &Manifest::default(),
        &config(vec![], vec![custom]),
        &no_native_env(vec![]),
    );
    assert_eq!(tails(&found), [(Base::WinHome, "Mine/s.dat")]);
}

#[test]
fn a_windows_row_is_not_scanned_on_native_linux() {
    let home = tempfile::tempdir().unwrap();
    write(&home.path().join("Mine/s.dat"), "a");
    let custom = rows_game("Mine", None, Some("<home>/Mine"));
    let report = scan(
        &Manifest::default(),
        &config(vec![], vec![custom]),
        &linux_env(home.path()),
        |_, _| {},
    );
    assert_eq!(report.games, []);
}

/// Times a full-manifest scan against one prefix root (ADR-0008). Run with
/// `SALVAJOCS_MANIFEST_YAML=<manifest.yaml> SALVAJOCS_PREFIX=<pfx> cargo test --release -p
/// salvajocs-core scan::tests::time_full_manifest_scan -- --ignored --nocapture`.
#[test]
#[ignore = "needs the real manifest and a prefix"]
fn time_full_manifest_scan() {
    // CI runs ignored tests too; without the inputs there is nothing to time.
    let (Ok(manifest_path), Ok(prefix)) = (
        std::env::var("SALVAJOCS_MANIFEST_YAML"),
        std::env::var("SALVAJOCS_PREFIX"),
    ) else {
        eprintln!("SALVAJOCS_MANIFEST_YAML and SALVAJOCS_PREFIX are not both set; skipping");
        return;
    };
    let yaml = fs::read_to_string(manifest_path).unwrap();
    let prefix = PathBuf::from(prefix);
    let manifest = parse_yaml(&yaml).unwrap();
    let config = config(vec![wine_root(&prefix)], vec![]);
    let started = Instant::now();
    let report = scan(&manifest, &config, &no_native_env(vec![]), |_, _| {});
    println!(
        "{} games, {} files, {} problems in {:?}",
        report.games.len(),
        report.games.iter().map(|s| s.files.len()).sum::<usize>(),
        report.problems.len(),
        started.elapsed()
    );
}
