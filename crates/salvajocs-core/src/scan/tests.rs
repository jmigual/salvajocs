use std::time::Instant;

use super::*;
use crate::config::PasswordMode;
use crate::layout::LinuxFolders;
use crate::manifest::{CustomGame, Integration, parse_yaml};

/// Creates a valid prefix at `root/pfx` with a `steamuser` home and returns its path.
fn make_prefix(root: &Path) -> PathBuf {
    let prefix = root.join("pfx");
    fs::create_dir_all(prefix.join("drive_c/users/steamuser")).unwrap();
    fs::write(prefix.join("user.reg"), "").unwrap();
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
            home: home.to_path_buf(),
            xdg_data: home.join(".local/share"),
            xdg_config: home.join(".config"),
        })),
        user: "me".into(),
        steam_roots: Vec::new(),
    }
}

fn no_native_env(steam_roots: Vec<PathBuf>) -> Environment {
    Environment {
        native: None,
        user: String::new(),
        steam_roots,
    }
}

fn game(manifest: &Manifest, config: &AppConfig, env: &Environment) -> GameScan {
    let mut scans = scan(manifest, config, env);
    assert_eq!(scans.len(), 1, "{scans:?}");
    scans.remove(0)
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

#[test]
fn roots_record_glob_free_prefixes() {
    let home = tempfile::tempdir().unwrap();
    write(&home.path().join(".config/foo/slot1/save.dat"), "a");
    write(&home.path().join(".local/share/bar/one.sav"), "a");
    write(&home.path().join(".config/none/x.dat"), "a");
    let manifest = parse_yaml(
        "Foo:\n  files:\n    <xdgConfig>/foo/*/save.dat: {}\n    <home>/.local/share/bar: {}\n    \
         <xdgConfig>/missing/*/x: {}\n",
    )
    .unwrap();
    let found = game(&manifest, &config(vec![], vec![]), &linux_env(home.path()));
    let mut roots = found.roots;
    roots.sort();
    assert_eq!(
        roots,
        [
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
        entry: parse_yaml("Mine:\n  files:\n    <winAppData>/Mine: {}\n")
            .unwrap()
            .games
            .remove("Mine")
            .unwrap(),
        integration: Integration::Override,
        wine_prefix: Some(pfx),
        pairs: Vec::new(),
    };
    let found = game(
        &Manifest::default(),
        &config(vec![], vec![custom]),
        &no_native_env(vec![]),
    );
    assert_eq!(found.title, "Mine");
    assert_eq!(tails(&found), [(Base::WinAppData, "Mine/s.dat")]);
}

/// Times a full-manifest scan against one prefix root (ADR-0008). Run with
/// `SALVAJOCS_MANIFEST_YAML=<manifest.yaml> SALVAJOCS_PREFIX=<pfx> cargo test --release -p
/// salvajocs-core scan::tests::time_full_manifest_scan -- --ignored --nocapture`.
#[test]
#[ignore = "needs the real manifest and a prefix"]
fn time_full_manifest_scan() {
    let yaml = fs::read_to_string(std::env::var("SALVAJOCS_MANIFEST_YAML").unwrap()).unwrap();
    let prefix = PathBuf::from(std::env::var("SALVAJOCS_PREFIX").unwrap());
    let manifest = parse_yaml(&yaml).unwrap();
    let config = config(vec![wine_root(&prefix)], vec![]);
    let started = Instant::now();
    let scans = scan(&manifest, &config, &no_native_env(vec![]));
    println!(
        "{} games, {} files in {:?}",
        scans.len(),
        scans.iter().map(|s| s.files.len()).sum::<usize>(),
        started.elapsed()
    );
}
