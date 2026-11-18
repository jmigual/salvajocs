use super::*;

/// A fake home with the XDG config directory inside it.
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
        detect(&self.home, &self.config())
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
