use super::*;
use crate::manifest::{GameEntry, Gog, Ids, Steam};

/// A fake home with the XDG config and data directories inside it.
struct Home {
    _dir: tempfile::TempDir,
    home: PathBuf,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        Self { _dir: dir, home }
    }

    fn config(&self) -> PathBuf {
        self.home.join(".config")
    }

    fn data(&self) -> PathBuf {
        self.home.join(".local/share")
    }

    /// Writes `content` to `path` below the home, creating its parents.
    fn write(&self, path: &str, content: &str) -> PathBuf {
        let path = self.home.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    /// Creates a valid prefix at `path` below the home.
    fn prefix(&self, path: &str) -> PathBuf {
        let prefix = self.home.join(path);
        fs::create_dir_all(prefix.join("drive_c/users/steamuser")).unwrap();
        fs::write(prefix.join("user.reg"), "").unwrap();
        prefix
    }

    fn detect(&self) -> (Vec<LauncherPrefix>, Vec<ScanProblem>) {
        detect(&self.home, &self.config(), &self.data())
    }

    /// Detects, expecting no problems.
    fn prefixes(&self) -> Vec<LauncherPrefix> {
        let (prefixes, problems) = self.detect();
        assert_eq!(problems, []);
        prefixes
    }
}

fn found(launcher: Launcher, path: PathBuf, game: Option<LauncherGame>) -> LauncherPrefix {
    LauncherPrefix {
        launcher,
        path,
        game,
    }
}

fn titled(title: &str) -> LauncherGame {
    LauncherGame {
        title: Some(title.to_owned()),
        ..LauncherGame::default()
    }
}

fn heroic(path: PathBuf) -> LauncherPrefix {
    found(Launcher::Heroic, path, Some(LauncherGame::default()))
}

const HEROIC: &str = ".config/heroic";

/// Writes a Heroic `GamesConfig` file for `app` with the given wine prefix and type.
fn heroic_config(h: &Home, app: &str, prefix: &str, wine_type: &str) {
    let config = serde_json::json!({
        app: { "winePrefix": prefix, "wineVersion": { "type": wine_type } },
        "version": "v0",
        "explicit": false,
    });
    h.write(
        &format!("{HEROIC}/GamesConfig/{app}.json"),
        &config.to_string(),
    );
}

/// Writes a Heroic `GamesConfig` file for `app` stripped of its defaults.
fn heroic_stripped(h: &Home, app: &str) {
    h.write(
        &format!("{HEROIC}/GamesConfig/{app}.json"),
        &format!(r#"{{"{app}": {{}}, "version": "v0", "explicit": false}}"#),
    );
}

/// Writes Heroic's GOG `installed.json` with `(app, platform)` entries.
fn gog_installed(h: &Home, apps: &[(&str, &str)]) {
    let installed: Vec<_> = apps
        .iter()
        .map(|(app, platform)| serde_json::json!({ "appName": app, "platform": platform }))
        .collect();
    h.write(
        &format!("{HEROIC}/gog_store/installed.json"),
        &serde_json::json!({ "installed": installed }).to_string(),
    );
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

#[test]
fn heroic_gog_app_name_is_gog_id() {
    let h = Home::new();
    let gog = h.prefix("Games/Heroic/Prefixes/Foo");
    let epic = h.prefix("Games/Heroic/Prefixes/Bar");
    heroic_config(&h, "1207658924", &display(&gog), "wine");
    heroic_config(&h, "12345", &display(&epic), "wine");
    h.write(
        &format!("{HEROIC}/store_cache/gog_library.json"),
        r#"{"games": [{"app_name": "1207658924", "title": "Foo"}]}"#,
    );
    h.write(
        &format!("{HEROIC}/store_cache/legendary_library.json"),
        r#"{"library": [{"app_name": "12345", "title": "Bar"}]}"#,
    );
    let foo = LauncherGame {
        gog_id: Some(1_207_658_924),
        ..titled("Foo")
    };
    assert_eq!(
        h.prefixes(),
        [
            found(Launcher::Heroic, gog, Some(foo)),
            found(Launcher::Heroic, epic, Some(titled("Bar"))),
        ]
    );
}

#[test]
fn heroic_legacy_gog_library_and_nile_library_titles() {
    let h = Home::new();
    let gog = h.prefix("Games/Heroic/Prefixes/Foo");
    let amazon = h.prefix("Games/Heroic/Prefixes/Bar");
    heroic_config(&h, "1207658924", &display(&gog), "wine");
    heroic_config(&h, "amzn1.adg.product.1", &display(&amazon), "wine");
    h.write(
        &format!("{HEROIC}/gog_store/library.json"),
        r#"{"games": [{"app_name": "1207658924", "title": "Foo"}]}"#,
    );
    h.write(
        &format!("{HEROIC}/store_cache/nile_library.json"),
        r#"{"library": [{"app_name": "amzn1.adg.product.1", "title": "Bar", "install": {}}]}"#,
    );
    let foo = LauncherGame {
        gog_id: Some(1_207_658_924),
        ..titled("Foo")
    };
    assert_eq!(
        h.prefixes(),
        [
            found(Launcher::Heroic, gog, Some(foo)),
            found(Launcher::Heroic, amazon, Some(titled("Bar"))),
        ]
    );
}

#[test]
fn heroic_proton_prefix_is_pfx() {
    let h = Home::new();
    let pfx = h.prefix("Games/Heroic/Prefixes/Foo/pfx");
    heroic_config(&h, "Foo", &display(pfx.parent().unwrap()), "proton");
    assert_eq!(h.prefixes(), [heroic(pfx)]);
}

#[test]
fn heroic_tilde_wine_prefix() {
    let h = Home::new();
    let prefix = h.prefix("Games/Foo");
    heroic_config(&h, "Foo", "~/Games/Foo", "wine");
    assert_eq!(h.prefixes(), [heroic(prefix)]);
}

#[test]
fn heroic_missing_wine_prefix_uses_config_json() {
    let h = Home::new();
    let prefix = h.prefix("Prefixes/default");
    heroic_stripped(&h, "Foo");
    gog_installed(&h, &[("Foo", "windows")]);
    h.write(
        &format!("{HEROIC}/config.json"),
        r#"{"defaultSettings": {"winePrefix": "~/Prefixes/default"}}"#,
    );
    assert_eq!(h.prefixes(), [heroic(prefix)]);
}

#[test]
fn heroic_without_config_json_uses_shared_prefix() {
    let h = Home::new();
    let prefix = h.prefix("Games/Heroic/Prefixes/shared");
    heroic_stripped(&h, "Foo");
    h.write(
        &format!("{HEROIC}/legendaryConfig/legendary/installed.json"),
        r#"{"Foo": {"app_name": "Foo", "platform": "Windows"}}"#,
    );
    assert_eq!(h.prefixes(), [heroic(prefix)]);
}

#[test]
fn heroic_default_prefix_needs_a_windows_install() {
    let h = Home::new();
    let shared = h.prefix("Games/Heroic/Prefixes/shared");
    for app in ["Amazon", "Mac", "Sideload", "Unknown"] {
        heroic_stripped(&h, app);
    }
    h.write(
        &format!("{HEROIC}/nile_config/nile/installed.json"),
        r#"[{"id": "Amazon", "path": "/games/amazon"}]"#,
    );
    gog_installed(&h, &[("Mac", "osx")]);
    h.write(
        &format!("{HEROIC}/sideload_apps/library.json"),
        r#"{"games": [{"app_name": "Sideload", "title": "Side", "install": {"platform": "Windows"}}]}"#,
    );
    assert_eq!(
        h.prefixes(),
        [
            heroic(shared.clone()),
            found(Launcher::Heroic, shared, Some(titled("Side"))),
        ]
    );
}

#[test]
fn heroic_linux_install_is_skipped() {
    let h = Home::new();
    let prefix = h.prefix("Games/Heroic/Prefixes/Foo");
    heroic_config(&h, "Gog", &display(&prefix), "wine");
    heroic_config(&h, "Sideload", &display(&prefix), "wine");
    gog_installed(&h, &[("Gog", "linux")]);
    h.write(
        &format!("{HEROIC}/sideload_apps/library.json"),
        r#"{"games": [{"app_name": "Sideload", "title": "Side", "install": {"platform": "linux"}}]}"#,
    );
    assert_eq!(h.prefixes(), []);
}

#[test]
fn heroic_empty_wine_prefix() {
    let h = Home::new();
    let shared = h.prefix("Games/Heroic/Prefixes/shared");
    heroic_config(&h, "Empty", "", "wine");
    heroic_stripped(&h, "Stripped");
    gog_installed(&h, &[("Empty", "windows"), ("Stripped", "windows")]);
    h.write(
        &format!("{HEROIC}/config.json"),
        r#"{"defaultSettings": {"winePrefix": ""}}"#,
    );
    assert_eq!(h.prefixes(), [heroic(shared)]);
}

#[test]
fn heroic_flatpak_dir() {
    let h = Home::new();
    let prefix = h.prefix("Games/Heroic/Prefixes/Foo");
    let heroic = ".var/app/com.heroicgameslauncher.hgl/config/heroic";
    let config = serde_json::json!({ "Foo": { "winePrefix": prefix } }).to_string();
    h.write(&format!("{heroic}/GamesConfig/Foo.json"), &config);
    h.write(
        &format!("{heroic}/sideload_apps/library.json"),
        r#"{"games": [{"app_name": "Foo", "title": "Foo Game"}]}"#,
    );
    let game = Some(titled("Foo Game"));
    assert_eq!(h.prefixes(), [found(Launcher::Heroic, prefix, game)]);
}

#[test]
fn heroic_reads_only_json_games_configs() {
    let h = Home::new();
    let prefix = h.prefix("Games/Foo");
    heroic_config(&h, "Foo", &display(&prefix), "wine");
    h.write(&format!("{HEROIC}/GamesConfig/Foo-lastPlay.log"), "{");
    assert_eq!(h.prefixes(), [heroic(prefix)]);
}

#[test]
fn not_a_prefix_is_skipped() {
    let h = Home::new();
    let empty = h.home.join("Games/Heroic/Prefixes/Foo");
    fs::create_dir_all(&empty).unwrap();
    heroic_config(&h, "Foo", &display(&empty), "wine");
    assert_eq!(h.prefixes(), []);
}

#[test]
fn malformed_games_config_is_a_problem() {
    let h = Home::new();
    let file = h.write(&format!("{HEROIC}/GamesConfig/Foo.json"), "{");
    let (prefixes, problems) = h.detect();
    assert_eq!(prefixes, []);
    assert!(matches!(
        problems.as_slice(),
        [ScanProblem { path, kind: ProblemKind::Unreadable(_) }] if *path == file
    ));
}

#[cfg(unix)]
#[test]
fn unreadable_prefix_is_a_problem() {
    use std::os::unix::fs::PermissionsExt;

    let h = Home::new();
    let locked = h.home.join("Locked");
    fs::create_dir_all(&locked).unwrap();
    heroic_config(&h, "Foo", &display(&locked.join("Foo")), "wine");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let (prefixes, problems) = h.detect();
    let readable = fs::read_dir(&locked).is_ok();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        return; // running as root: permissions do not apply
    }
    assert_eq!(prefixes, []);
    assert!(matches!(
        problems.as_slice(),
        [
            ScanProblem {
                kind: ProblemKind::Unreadable(_),
                ..
            },
            ..
        ]
    ));
}

#[test]
fn lutris_installer_yml_name_slug_service() {
    let h = Home::new();
    let gog = h.prefix("Games/foo");
    let steam = h.prefix("Games/bar");
    h.write(
        ".config/lutris/games/foo-gog-1700000000.yml",
        "name: Foo\ngame_slug: foo\nservice: gog\nservice_id: '1207658924'\n\
         game:\n  exe: /elsewhere/foo.exe\n  prefix: ~/Games/foo\n\
         script:\n  files:\n  - setup: N/A\nwine:\n  version: lutris\n",
    );
    h.write(
        ".config/lutris/games/bar-1700000001.yml",
        "name: Bar\ngame_slug: bar-game\nservice: steam\nservice_id: 620\n\
         game:\n  prefix: ~/Games/bar\n",
    );
    let bar = LauncherGame {
        steam_id: Some(620),
        lutris_slug: Some("bar-game".to_owned()),
        ..titled("Bar")
    };
    let foo = LauncherGame {
        gog_id: Some(1_207_658_924),
        lutris_slug: Some("foo".to_owned()),
        ..titled("Foo")
    };
    assert_eq!(
        h.prefixes(),
        [
            found(Launcher::Lutris, steam, Some(bar)),
            found(Launcher::Lutris, gog, Some(foo)),
        ]
    );
}

#[test]
fn lutris_manual_yml_stem_slug_and_exe_prefix() {
    let h = Home::new();
    let prefix = h.prefix("Games/foo");
    h.write(
        ".config/lutris/games/foo-bar-1700000000.yml",
        &format!(
            "game:\n  exe: {}/drive_c/Games/Foo/foo.exe\n  prefix: ''\nwine:\n  version: lutris\n",
            prefix.display()
        ),
    );
    h.write(
        ".config/lutris/games/relative-1700000001.yml",
        "game:\n  exe: drive_c/foo.exe\n",
    );
    let game = LauncherGame {
        lutris_slug: Some("foo-bar".to_owned()),
        ..LauncherGame::default()
    };
    assert_eq!(h.prefixes(), [found(Launcher::Lutris, prefix, Some(game))]);
}

/// A hand-added Lutris game file for the prefix at `~/Games/foo`.
const LUTRIS_FOO: &str = "game:\n  prefix: ~/Games/foo\n";

fn lutris_foo(prefix: PathBuf) -> LauncherPrefix {
    let game = LauncherGame {
        lutris_slug: Some("foo".to_owned()),
        ..LauncherGame::default()
    };
    found(Launcher::Lutris, prefix, Some(game))
}

#[test]
fn lutris_reads_only_yml_games() {
    let h = Home::new();
    let prefix = h.prefix("Games/foo");
    h.write(".config/lutris/games/foo-1.yaml", LUTRIS_FOO);
    h.write(".config/lutris/games/foo-2.yml.bak", "game: [");
    assert_eq!(h.prefixes(), [lutris_foo(prefix)]);
}

/// Lutris writes a `null` key into the file of a Steam game.
#[test]
fn lutris_null_key_is_ignored() {
    let h = Home::new();
    let prefix = h.prefix("Games/foo");
    let steam = "system: {}\nnull: {}\ngame:\n  appid: '1000001'\n";
    let steam = h.write(
        ".local/share/lutris/games/steam-1000001-1700000000.yml",
        steam,
    );
    h.write(
        ".local/share/lutris/games/foo-1.yml",
        "system: {}\nnull: {}\ngame:\n  prefix: ~/Games/foo\n",
    );
    h.write(
        ".local/share/lutris/games/null-1.yml",
        "game:\n  prefix: ~\n  exe: null\n",
    );
    let (prefixes, problems) = h.detect();
    assert_eq!(problems, [], "{}", steam.display());
    assert_eq!(prefixes, [lutris_foo(prefix)]);
}

#[test]
fn malformed_lutris_yml_is_a_problem() {
    let h = Home::new();
    let file = h.write(".config/lutris/games/foo-1.yml", "game: [");
    let (prefixes, problems) = h.detect();
    assert_eq!(prefixes, []);
    assert!(matches!(
        problems.as_slice(),
        [ScanProblem { path, kind: ProblemKind::Unreadable(_) }] if *path == file
    ));
}

#[test]
fn lutris_falls_back_to_data_dir() {
    let h = Home::new();
    let prefix = h.prefix("Games/foo");
    h.write(".local/share/lutris/games/foo-1.yml", LUTRIS_FOO);
    assert_eq!(h.prefixes(), [lutris_foo(prefix)]);

    fs::create_dir_all(h.config().join("lutris")).unwrap();
    assert_eq!(h.prefixes(), []);
}

#[test]
fn lutris_flatpak_dirs() {
    for dir in ["config", "data"] {
        let h = Home::new();
        let prefix = h.prefix("Games/foo");
        h.write(
            &format!(".var/app/net.lutris.Lutris/{dir}/lutris/games/foo-1.yml"),
            LUTRIS_FOO,
        );
        assert_eq!(h.prefixes(), [lutris_foo(prefix)], "{dir}");
    }
}

#[test]
fn bottles_native_and_flatpak() {
    let h = Home::new();
    let native = h.prefix(".local/share/bottles/bottles/Foo");
    let flatpak = h.prefix(".var/app/com.usebottles.bottles/data/bottles/bottles/Bar");
    h.write(
        ".local/share/bottles/bottles/Empty/bottle.yml",
        "Name: Empty\nPath: Empty\nCustom_Path: false\n",
    );
    h.write(".local/share/bottles/bottles/stray.txt", "");
    assert_eq!(
        h.prefixes(),
        [
            found(Launcher::Bottles, native, None),
            found(Launcher::Bottles, flatpak, None),
        ]
    );
}

#[test]
fn bottles_placeholder_path() {
    let h = Home::new();
    let placed = h.prefix("Drive/Foo");
    let custom = h.prefix("Drive/Bar");
    h.write(
        ".local/share/bottles/bottles/Foo/placeholder.yml",
        &format!("Path: {}\n", placed.display()),
    );
    h.write(
        ".local/share/bottles/bottles/Bar/bottle.yml",
        &format!(
            "Name: Bar\nPath: {}\nCustom_Path: true\nEnvironment: Gaming\n",
            custom.display()
        ),
    );
    assert_eq!(
        h.prefixes(),
        [
            found(Launcher::Bottles, custom, None),
            found(Launcher::Bottles, placed, None),
        ]
    );
}

#[test]
fn bottles_empty_placeholder_path_falls_back_to_the_bottle() {
    let h = Home::new();
    let bottle = h.prefix(".local/share/bottles/bottles/Foo");
    h.write(
        ".local/share/bottles/bottles/Foo/placeholder.yml",
        "Path:\n",
    );
    assert_eq!(h.prefixes(), [found(Launcher::Bottles, bottle, None)]);
}

#[test]
fn bottles_custom_path_false_is_ignored() {
    let h = Home::new();
    let elsewhere = h.prefix("Drive/Bar");
    h.write(
        ".local/share/bottles/bottles/Bar/bottle.yml",
        &format!(
            "Name: Bar\nPath: {}\nCustom_Path: false\n",
            elsewhere.display()
        ),
    );
    assert_eq!(h.prefixes(), []);
}

#[test]
fn bottles_var_data_rewrite() {
    let h = Home::new();
    let prefix = h.prefix(".var/app/com.usebottles.bottles/data/custom/Foo");
    let placeholder = "Path: /var/data/custom/Foo\n";
    h.write(
        ".var/app/com.usebottles.bottles/data/bottles/bottles/Foo/placeholder.yml",
        placeholder,
    );
    h.write(
        ".local/share/bottles/bottles/Foo/placeholder.yml",
        placeholder,
    );
    assert_eq!(h.prefixes(), [found(Launcher::Bottles, prefix, None)]);
}

#[test]
fn malformed_bottles_placeholder_is_a_problem() {
    let h = Home::new();
    let file = h.write(
        ".local/share/bottles/bottles/Foo/placeholder.yml",
        "Name: Foo\n",
    );
    let (prefixes, problems) = h.detect();
    assert_eq!(prefixes, []);
    assert!(matches!(
        problems.as_slice(),
        [ScanProblem { path, kind: ProblemKind::Unreadable(_) }] if *path == file
    ));
}

// These normalize_title tests cover ludusavi's `can_normalize_title` cases (src/scan/title.rs).

#[test]
fn normalize_title_lowercases() {
    assert_eq!(normalize_title("foo bar"), "foo bar");
    assert_eq!(normalize_title("Foo Bar"), "foo bar");
}

#[test]
fn normalize_title_drops_punctuated_editions() {
    assert_eq!(normalize_title("Foo Bar: Any Arbitrary Edition"), "foo bar");
    assert_eq!(
        normalize_title("Foo Bar - Any Arbitrary Edition"),
        "foo bar"
    );
    assert_eq!(normalize_title("Foo Bar™ Any Arbitrary Edition"), "foo bar");
    assert_eq!(
        normalize_title("Foo Bar® - Any Arbitrary Edition"),
        "foo bar"
    );
}

#[test]
fn normalize_title_drops_game_of_the_year_edition() {
    assert_eq!(
        normalize_title("Foo Bar Game of the Year Edition"),
        "foo bar"
    );
}

#[test]
fn normalize_title_drops_one_word_editions() {
    assert_eq!(normalize_title("Foo Bar Special Edition"), "foo bar");
}

#[test]
fn normalize_title_drops_year_suffix() {
    assert_eq!(normalize_title("Foo Bar (2000)"), "foo bar");
}

#[test]
fn normalize_title_replaces_symbols() {
    assert_eq!(normalize_title("Foo:Bar"), "foo bar");
    assert_eq!(normalize_title("Foo: Bar"), "foo bar");
    assert_eq!(normalize_title("Fo'o Bar"), "foo bar");
    assert_eq!(normalize_title("Foo \"Bar\""), "foo bar");
}

#[test]
fn normalize_title_collapses_spaces() {
    assert_eq!(normalize_title("  Foo  Bar  "), "foo bar");
}

/// salvajocs' own cases, beyond ludusavi's.
#[test]
fn normalize_title_salvajocs_cases() {
    assert_eq!(
        normalize_title("Foo Bar Very Special Edition"),
        "foo bar very"
    );
    assert_eq!(normalize_title("Foo Bar (Remake)"), "foo bar (remake)");
    assert_eq!(normalize_title("Foo’s “Bar”"), "foos bar");
    assert_eq!(normalize_title("\tFoo Bar\n"), "foo bar");
}

/// A manifest of `games`, each a title with its entry.
fn manifest(games: &[(&str, GameEntry)]) -> Manifest {
    Manifest {
        games: games
            .iter()
            .map(|(title, entry)| ((*title).to_owned(), entry.clone()))
            .collect(),
    }
}

fn gog_game(id: u64) -> GameEntry {
    GameEntry {
        gog: Some(Gog { id: Some(id) }),
        ..GameEntry::default()
    }
}

fn steam_game(id: u32) -> GameEntry {
    GameEntry {
        steam: Some(Steam { id: Some(id) }),
        ..GameEntry::default()
    }
}

fn ids_game(ids: Ids) -> GameEntry {
    GameEntry {
        id: Some(ids),
        ..GameEntry::default()
    }
}

fn mapped(manifest: &Manifest, game: LauncherGame) -> Option<&str> {
    GameIndex::new(manifest).title(&found(Launcher::Heroic, PathBuf::new(), Some(game)))
}

#[test]
fn maps_by_gog_id_and_gog_extra() {
    let m = manifest(&[
        ("Foo", gog_game(1)),
        (
            "Bar",
            ids_game(Ids {
                gog_extra: vec![2],
                ..Ids::default()
            }),
        ),
    ]);
    let gog = |id| LauncherGame {
        gog_id: Some(id),
        ..titled("Other")
    };
    assert_eq!(mapped(&m, gog(1)), Some("Foo"));
    assert_eq!(mapped(&m, gog(2)), Some("Bar"));
}

#[test]
fn maps_by_steam_id_and_steam_extra() {
    let m = manifest(&[
        ("Foo", steam_game(10)),
        (
            "Bar",
            ids_game(Ids {
                steam_extra: vec![20],
                ..Ids::default()
            }),
        ),
    ]);
    let steam = |id| LauncherGame {
        steam_id: Some(id),
        ..titled("Other")
    };
    assert_eq!(mapped(&m, steam(10)), Some("Foo"));
    assert_eq!(mapped(&m, steam(20)), Some("Bar"));
}

#[test]
fn maps_by_lutris_slug() {
    let m = manifest(&[(
        "Foo",
        ids_game(Ids {
            lutris: Some("foo-slug".to_owned()),
            ..Ids::default()
        }),
    )]);
    let game = LauncherGame {
        lutris_slug: Some("foo-slug".to_owned()),
        ..titled("Other")
    };
    assert_eq!(mapped(&m, game), Some("Foo"));
}

#[test]
fn an_id_beats_the_title() {
    let m = manifest(&[("Foo", gog_game(1)), ("Bar", GameEntry::default())]);
    let game = LauncherGame {
        gog_id: Some(1),
        ..titled("Bar")
    };
    assert_eq!(mapped(&m, game), Some("Foo"));
}

#[test]
fn ids_of_several_games_fall_through_to_the_title() {
    let m = manifest(&[
        ("Foo", gog_game(1)),
        ("Bar", steam_game(10)),
        ("Baz", GameEntry::default()),
    ]);
    let game = LauncherGame {
        gog_id: Some(1),
        steam_id: Some(10),
        ..titled("Baz")
    };
    assert_eq!(mapped(&m, game), Some("Baz"));
}

#[test]
fn an_exact_title_beats_a_normalized_one() {
    let m = manifest(&[
        ("Foo: Bar", GameEntry::default()),
        ("Foo Bar", GameEntry::default()),
    ]);
    assert_eq!(mapped(&m, titled("Foo: Bar")), Some("Foo: Bar"));
}

#[test]
fn maps_by_normalized_title() {
    let m = manifest(&[("Foo Bar", GameEntry::default())]);
    assert_eq!(mapped(&m, titled("FOO: Bar™")), Some("Foo Bar"));
}

#[test]
fn a_normalized_title_of_several_games_is_ambiguous() {
    let m = manifest(&[
        ("Foo: Bar", GameEntry::default()),
        ("Foo Bar", GameEntry::default()),
    ]);
    assert_eq!(mapped(&m, titled("foo bar")), None);
}

#[test]
fn an_unknown_game_is_not_mapped() {
    let m = manifest(&[("Foo", gog_game(1))]);
    let game = LauncherGame {
        gog_id: Some(2),
        ..titled("Bar")
    };
    assert_eq!(mapped(&m, game), None);
}

#[test]
fn a_prefix_without_a_game_is_not_mapped() {
    let m = manifest(&[("Foo", GameEntry::default())]);
    let bottle = found(Launcher::Bottles, PathBuf::new(), None);
    assert_eq!(GameIndex::new(&m).title(&bottle), None);
}
