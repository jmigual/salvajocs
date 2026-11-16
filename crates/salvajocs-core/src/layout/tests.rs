use std::fs;

use super::*;

fn prefix_ctx() -> Context {
    let prefix = PathBuf::from("/pfx");
    let folders = WinFolders::for_prefix(&prefix, "steamuser");
    Context::WinePrefix { prefix, folders }
}

fn native_win_ctx() -> Context {
    Context::WindowsNative(WinFolders::for_prefix(Path::new("/fake"), "me"))
}

fn linux_ctx() -> Context {
    Context::Linux(LinuxFolders {
        user: "me".into(),
        home: "/home/me".into(),
        xdg_data: "/home/me/.local/share".into(),
        xdg_config: "/home/me/.config".into(),
    })
}

fn no_roots_vars() -> GameVars {
    GameVars::new("Game", vec![], &[], None)
}

fn patterns(expanded: Vec<(PathBuf, String)>) -> Vec<String> {
    expanded.into_iter().map(|(_, pattern)| pattern).collect()
}

fn classified(template: &str, ctx: &Context) -> (Base, String) {
    classify(template, ctx).unwrap()
}

#[test]
fn home_under_windows_context_is_win_home() {
    for ctx in [prefix_ctx(), native_win_ctx()] {
        assert_eq!(
            classified("<home>/Saved Games/Foo", &ctx),
            (Base::WinHome, "Saved Games/Foo".into())
        );
    }
}

#[test]
fn home_appdata_locallow_normalizes_to_win_local_app_data_low() {
    let ctx = prefix_ctx();
    assert_eq!(
        classified("<home>/AppData/LocalLow/Co/Game", &ctx),
        (Base::WinLocalAppDataLow, "Co/Game".into())
    );
    assert_eq!(
        classified("<home>/AppData/Local/Foo", &ctx),
        (Base::WinLocalAppData, "Foo".into())
    );
    assert_eq!(
        classified("<home>/AppData/Roaming/Foo", &ctx),
        (Base::WinAppData, "Foo".into())
    );
    // Only whole components normalize.
    assert_eq!(
        classified("<home>/AppData/Locale/Foo", &ctx),
        (Base::WinHome, "AppData/Locale/Foo".into())
    );
}

#[test]
fn home_dot_config_normalizes_to_xdg_config() {
    let ctx = linux_ctx();
    assert_eq!(
        classified("<home>/.config/unity3d/Co/Game", &ctx),
        (Base::XdgConfig, "unity3d/Co/Game".into())
    );
    assert_eq!(
        classified("<home>/.local/share/Foo", &ctx),
        (Base::XdgData, "Foo".into())
    );
    assert_eq!(
        classified("<home>/.configs/Foo", &ctx),
        (Base::Home, ".configs/Foo".into())
    );
    assert_eq!(
        classified("<xdgConfig>/Foo", &ctx),
        (Base::XdgConfig, "Foo".into())
    );
}

#[test]
fn leading_placeholders_pick_their_base() {
    let ctx = prefix_ctx();
    assert_eq!(
        classified("<winAppData>/Foo", &ctx),
        (Base::WinAppData, "Foo".into())
    );
    assert_eq!(
        classified("<winDocuments>", &ctx),
        (Base::WinDocuments, "".into())
    );
    assert_eq!(
        classified("<base>/saves", &ctx),
        (Base::GameDir, "saves".into())
    );
    assert_eq!(
        classified("<root>/x", &linux_ctx()),
        (Base::StoreRoot, "x".into())
    );
}

#[cfg(unix)]
#[test]
fn classify_unix_absolute_template() {
    assert_eq!(
        classified("/opt/game/save.dat", &linux_ctx()),
        (Base::Absolute, "opt/game/save.dat".into())
    );
    // A Windows path is not absolute on a Unix host.
    assert_eq!(classify("C:\\Games\\Foo", &linux_ctx()), None);
    assert_eq!(classify("C:/Games/Foo", &prefix_ctx()), None);
}

#[test]
fn classify_rejects_what_the_context_cannot_resolve() {
    assert_eq!(classify("<winAppData>/Foo", &linux_ctx()), None);
    assert_eq!(classify("<xdgData>/Foo", &prefix_ctx()), None);
    assert_eq!(classify("relative/Foo", &linux_ctx()), None);
    assert_eq!(classify("<game>/Foo", &linux_ctx()), None);
    assert_eq!(classify("<homeless>/Foo", &linux_ctx()), None);
}

#[test]
fn prefix_and_native_windows_map_same_base_to_same_tail() {
    let prefix = tempfile::tempdir().unwrap();
    let in_prefix = Context::WinePrefix {
        prefix: prefix.path().into(),
        folders: WinFolders::for_prefix(prefix.path(), "steamuser"),
    };
    let native_dir = tempfile::tempdir().unwrap();
    let native_folders = WinFolders::for_prefix(native_dir.path(), "me");
    let native = Context::WindowsNative(native_folders.clone());
    let vars = no_roots_vars();

    // The prefix has the save on disk under a differently cased directory.
    let saved = prefix
        .path()
        .join("drive_c/users/steamuser/AppData/Roaming/foo");
    fs::create_dir_all(&saved).unwrap();
    fs::write(saved.join("slot1.sav"), b"x").unwrap();

    let template = "<home>/AppData/Roaming/Foo/slot1.sav";
    let (dir, pattern) = expand(template, &in_prefix, &vars).remove(0);
    let found: Vec<_> = glob::glob(&pattern)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(found.len(), 1);
    let tail = tail_of(&found[0], &dir).unwrap();
    assert_eq!(tail, "foo/slot1.sav");

    let (base, _) = classified(template, &native);
    let target = target_path(&base_dir(base, &native, &vars).unwrap(), &tail, true).unwrap();
    assert_eq!(base, Base::WinAppData);
    assert_eq!(target, native_folders.app_data.join("foo/slot1.sav"));
}

#[test]
fn linux_base_has_no_target_in_windows_context() {
    let vars = no_roots_vars();
    for base in [Base::Home, Base::XdgData, Base::XdgConfig] {
        assert_eq!(base_dir(base, &prefix_ctx(), &vars), None);
        assert_eq!(base_dir(base, &native_win_ctx(), &vars), None);
    }
    assert_eq!(base_dir(Base::WinAppData, &linux_ctx(), &vars), None);
    assert_eq!(
        base_dir(Base::Home, &linux_ctx(), &vars),
        Some("/home/me".into())
    );
}

#[test]
fn case_insensitive_only_in_prefix() {
    assert!(prefix_ctx().case_insensitive());
    assert!(!native_win_ctx().case_insensitive());
    assert!(!linux_ctx().case_insensitive());
}

#[test]
fn game_dir_resolves_through_roots_and_install_dirs() {
    let steam = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    fs::create_dir_all(steam.path().join("steamapps/common/Real Dir")).unwrap();
    let roots = [
        Root {
            path: other.path().into(),
            kind: RootKind::Other,
        },
        Root {
            path: steam.path().into(),
            kind: RootKind::Steam,
        },
        Root {
            path: "/pfx".into(),
            kind: RootKind::WinePrefix,
        },
    ];
    let vars = GameVars::new(
        "Title",
        vec!["Missing".into(), "Real Dir".into()],
        &roots,
        None,
    );
    let found = base_dir(Base::GameDir, &linux_ctx(), &vars);
    assert_eq!(found, Some(steam.path().join("steamapps/common/Real Dir")));
    // `<root>` is a Steam root only, even when an Other root is listed first.
    assert_eq!(
        base_dir(Base::StoreRoot, &linux_ctx(), &vars),
        Some(steam.path().into())
    );

    let not_installed = GameVars::new("Title", vec![], &roots, None);
    assert_eq!(base_dir(Base::GameDir, &linux_ctx(), &not_installed), None);
}

#[test]
fn install_dirs_that_are_not_plain_names_are_dropped() {
    let root = tempfile::tempdir().unwrap();
    let roots = [Root {
        path: root.path().into(),
        kind: RootKind::Other,
    }];
    let dirs = ["/etc", "../x", "a/b", "..", "", "ok"]
        .map(String::from)
        .to_vec();
    let vars = GameVars::new("Title", dirs, &roots, None);
    assert_eq!(vars.base, [root.path().join("ok")]);
    assert_eq!(vars.game, ["ok"]);
}

#[test]
fn install_dir_falls_back_to_title() {
    let root = tempfile::tempdir().unwrap();
    let roots = [Root {
        path: root.path().into(),
        kind: RootKind::Other,
    }];
    let vars = GameVars::new("Title", vec![], &roots, None);
    assert_eq!(vars.base, [root.path().join("Title")]);
}

#[test]
fn glob_free_prefix_stops_at_first_wildcard_component() {
    assert_eq!(
        glob_free_prefix("<home>/.config/Co/Game/*.sav"),
        "<home>/.config/Co/Game"
    );
    assert_eq!(glob_free_prefix("<home>/a/<storeUserId>/b"), "<home>/a");
    assert_eq!(glob_free_prefix("<base>/saves"), "<base>/saves");
    assert_eq!(glob_free_prefix("<base>/sa?es/x"), "<base>");
    assert_eq!(glob_free_prefix("*/x"), "");
    assert_eq!(glob_free_prefix("<home>/[ab]x"), "<home>");
}

#[test]
fn glob_pattern_matches_lowercase_dir() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("my games/foo")).unwrap();
    fs::write(dir.path().join("my games/foo/Save1.DAT"), b"x").unwrap();

    let count = |ci| {
        let pattern = glob_pattern(dir.path(), "My Games/Foo/*.dat", ci);
        glob::glob(&pattern).unwrap().filter_map(Result::ok).count()
    };
    assert_eq!(count(false), 0);
    assert_eq!(count(true), 1);
}

#[test]
fn glob_pattern_leaves_existing_classes_alone() {
    let pattern = glob_pattern(Path::new("/b"), "ab/[cd]e/*", true);
    assert_eq!(pattern, "/b/[aA][bB]/[cd][eE]/*");
    // A `]` first in a class, or after `!`, is a member.
    assert_eq!(fold_case("[]a]b"), "[]a][bB]");
    assert_eq!(fold_case("[!]a]b"), "[!]a][bB]");
}

#[test]
fn escaped_value_survives_case_folding_in_prefix() {
    let prefix = tempfile::tempdir().unwrap();
    let ctx = Context::WinePrefix {
        prefix: prefix.path().into(),
        folders: WinFolders::for_prefix(prefix.path(), "steamuser"),
    };
    let vars = GameVars::new("Game [GOTY]", vec![], &[], None);
    let saved = prefix.path().join("drive_c/users/steamuser/game [goty]");
    fs::create_dir_all(&saved).unwrap();
    fs::write(saved.join("a.sav"), b"x").unwrap();

    let (_, pattern) = expand("<home>/<game>/a.sav", &ctx, &vars).remove(0);
    let found: Vec<_> = glob::glob(&pattern)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(found, [saved.join("a.sav")]);
}

#[cfg(unix)]
#[test]
fn glob_pattern_escapes_base_dir() {
    let dir = tempfile::tempdir().unwrap();
    let odd = dir.path().join("[Game]*");
    let decoy = dir.path().join("G");
    fs::create_dir_all(&odd).unwrap();
    fs::create_dir_all(&decoy).unwrap();
    fs::write(odd.join("a.sav"), b"x").unwrap();
    fs::write(decoy.join("a.sav"), b"x").unwrap();

    let pattern = glob_pattern(&odd, "*.sav", false);
    let found: Vec<_> = glob::glob(&pattern)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(found, [odd.join("a.sav")]);
}

#[cfg(unix)]
#[test]
fn expand_builds_case_insensitive_patterns_only_in_prefix() {
    let vars = no_roots_vars();
    let template = "<winAppData>/Foo/*.sav";
    assert_eq!(
        patterns(expand(template, &prefix_ctx(), &vars)),
        ["/pfx/drive_c/users/steamuser/AppData/Roaming/[fF][oO][oO]/*.[sS][aA][vV]"]
    );
    assert_eq!(
        patterns(expand(template, &native_win_ctx(), &vars)),
        ["/fake/drive_c/users/me/AppData/Roaming/Foo/*.sav"]
    );
    assert!(expand(template, &linux_ctx(), &vars).is_empty());
}

#[test]
fn expand_substitutes_and_escapes_values() {
    let steam = tempfile::tempdir().unwrap();
    let roots = [Root {
        path: steam.path().into(),
        kind: RootKind::Steam,
    }];
    let vars = GameVars::new(
        "T",
        vec!["Game [GOTY]".into(), "Game".into()],
        &roots,
        Some("42".into()),
    );
    let ctx = linux_ctx();

    assert_eq!(
        patterns(expand(
            "<home>/<game>/<osUserName>/<storeGameId>/<storeUserId>/x",
            &ctx,
            &vars
        )),
        [
            "/home/me/Game [[]GOTY[]]/me/42/*/x",
            "/home/me/Game/me/42/*/x",
        ]
    );
    let base = steam.path().join("steamapps/common");
    let escaped = |dir: &str| glob::Pattern::escape(&base.join(dir).to_string_lossy());
    assert_eq!(
        patterns(expand("<base>/saves", &ctx, &vars)),
        [
            format!("{}/saves", escaped("Game [GOTY]")),
            format!("{}/saves", escaped("Game")),
        ]
    );
    assert!(expand("<home>/<storeGameId>", &ctx, &no_roots_vars()).is_empty());
    assert!(expand("<base>/saves", &ctx, &no_roots_vars()).is_empty());
}

#[cfg(unix)]
#[test]
fn expand_absolute_has_single_slash() {
    let expanded = expand("/opt/a/*.sav", &linux_ctx(), &no_roots_vars());
    assert_eq!(expanded, [("/".into(), "/opt/a/*.sav".to_owned())]);
}

#[cfg(unix)]
#[test]
fn absolute_host_path_is_not_case_folded_in_prefix() {
    let expanded = expand("/opt/a/*.sav", &prefix_ctx(), &no_roots_vars());
    assert_eq!(expanded, [("/".into(), "/opt/a/*.sav".to_owned())]);
}

#[test]
fn expand_names_the_base_dir_each_pattern_came_from() {
    let one = tempfile::tempdir().unwrap();
    let two = tempfile::tempdir().unwrap();
    let roots = [one.path(), two.path()].map(|path| Root {
        path: path.into(),
        kind: RootKind::Other,
    });
    let vars = GameVars::new("G", vec![], &roots, None);
    let dirs: Vec<_> = expand("<base>/s/*.sav", &linux_ctx(), &vars)
        .into_iter()
        .map(|(dir, _)| dir)
        .collect();
    assert_eq!(dirs, [one.path().join("G"), two.path().join("G")]);
}

// NTFS ignores case, so `my games` and `My Games` cannot both be created there.
#[cfg(unix)]
#[test]
fn target_path_reuses_existing_dir_case_insensitively() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("my games")).unwrap();

    assert_eq!(
        target_path(dir.path(), "My Games/Foo/s.dat", true),
        Some(dir.path().join("my games/Foo/s.dat"))
    );
    assert_eq!(
        target_path(dir.path(), "My Games/Foo/s.dat", false),
        Some(dir.path().join("My Games/Foo/s.dat"))
    );
    // An exact match wins over a case-folded one.
    fs::create_dir_all(dir.path().join("My Games")).unwrap();
    assert_eq!(
        target_path(dir.path(), "My Games", true),
        Some(dir.path().join("My Games"))
    );
    assert_eq!(
        target_path(dir.path(), "", true),
        Some(dir.path().to_path_buf())
    );
}

#[test]
fn target_path_rejects_escaping_tails() {
    let dir = tempfile::tempdir().unwrap();
    for tail in ["../x", "a/../../x", "a/.."] {
        assert_eq!(target_path(dir.path(), tail, false), None, "{tail}");
    }
}

#[test]
fn tail_of_requires_base_prefix() {
    assert_eq!(
        tail_of(Path::new("/a/b/c/d.sav"), Path::new("/a/b")),
        Some("c/d.sav".into())
    );
    assert_eq!(
        tail_of(Path::new("/a/b"), Path::new("/a/b")),
        Some("".into())
    );
    assert_eq!(tail_of(Path::new("/x/c"), Path::new("/a/b")), None);
    assert_eq!(
        tail_of(Path::new("/a/b/../../x/f.sav"), Path::new("/a/b")),
        None
    );
}

#[cfg(unix)]
#[test]
fn target_path_picks_case_variant_deterministically() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["SAVES", "Saves"] {
        fs::create_dir(dir.path().join(name)).unwrap();
    }
    for _ in 0..5 {
        assert_eq!(
            target_path(dir.path(), "saves/a", true),
            Some(dir.path().join("SAVES/a"))
        );
    }
}

#[test]
fn snapshot_meta_round_trips_and_names_bases() {
    let meta = SnapshotMeta {
        format: SNAPSHOT_FORMAT,
        title: "Game".into(),
        roots: vec![(Base::XdgConfig, "unity3d/Co/Game".into())],
    };
    let json = serde_json::to_string(&meta).unwrap();
    assert_eq!(
        json,
        r#"{"format":1,"title":"Game","roots":[["XdgConfig","unity3d/Co/Game"]]}"#
    );
    assert_eq!(serde_json::from_str::<SnapshotMeta>(&json).unwrap(), meta);
    assert_eq!(
        serde_json::to_string(&Base::WinLocalAppDataLow).unwrap(),
        "\"WinLocalAppDataLow\""
    );
    assert_eq!(Base::WinLocalAppDataLow.dir_name(), "WinLocalAppDataLow");
}

#[cfg(windows)]
#[test]
fn native_win_folders_exist() {
    let f = WinFolders::native().unwrap();
    for dir in [
        &f.home,
        &f.documents,
        &f.app_data,
        &f.local_app_data,
        &f.public,
        &f.program_data,
        &f.windows,
    ] {
        assert!(dir.is_dir(), "{}", dir.display());
    }
    assert_eq!(f.local_app_data_low, f.home.join("AppData/LocalLow"));
}

#[test]
fn win_absolute_tail_parsing() {
    assert_eq!(split_drive("C/x"), Some((PathBuf::from("C:\\"), "x")));
    assert_eq!(
        split_drive("d/Games/Foo"),
        Some((PathBuf::from("d:\\"), "Games/Foo"))
    );
    assert_eq!(split_drive("C"), Some((PathBuf::from("C:\\"), "")));
    for tail in ["CD/x", "1/x", ""] {
        assert_eq!(split_drive(tail), None, "{tail:?}");
    }
}

#[test]
fn drive_template_becomes_drive_letter_tail() {
    let tail = drive_absolute_tail;
    assert_eq!(
        tail("C:\\Games\\Foo\\save.dat").as_deref(),
        Some("C/Games/Foo/save.dat")
    );
    assert_eq!(tail("C:/Games/*.sav").as_deref(), Some("C/Games/*.sav"));
    assert_eq!(tail("C:\\").as_deref(), Some("C"));
    for template in [
        "C:",
        "C:Games",
        "\\\\server\\share\\x",
        "//server/share/x",
        "/opt/x",
        "1:/x",
    ] {
        assert_eq!(tail(template), None, "{template}");
    }
}

#[cfg(unix)]
#[test]
fn win_absolute_has_no_target_on_unix() {
    let vars = no_roots_vars();
    for ctx in [linux_ctx(), prefix_ctx(), native_win_ctx()] {
        assert_eq!(base_dir(Base::WinAbsolute, &ctx, &vars), None);
        assert!(expand("C:/Games/*.sav", &ctx, &vars).is_empty());
    }
    for ctx in [linux_ctx(), prefix_ctx()] {
        assert_eq!(win_absolute_target("C/Games/x", &ctx), None);
    }
}

#[cfg(windows)]
mod windows_absolute {
    use super::*;

    fn native() -> Context {
        Context::WindowsNative(WinFolders::native().unwrap())
    }

    #[test]
    fn classify_windows_absolute_template_uses_drive_letter_component() {
        assert_eq!(
            classified("C:\\Games\\Foo\\save.dat", &native()),
            (Base::WinAbsolute, "C/Games/Foo/save.dat".into())
        );
        assert_eq!(
            classified("C:/Games/Foo", &native()),
            (Base::WinAbsolute, "C/Games/Foo".into())
        );
    }

    #[test]
    fn unc_path_is_not_classified() {
        assert_eq!(classify("\\\\server\\share\\x", &native()), None);
        assert_eq!(classify("//server/share/x", &native()), None);
        assert_eq!(classify("/opt/x", &native()), None);
    }

    #[test]
    fn tail_of_windows_absolute_path() {
        assert_eq!(
            tail_of(Path::new("C:\\Games\\Foo\\save.dat"), Path::new("")),
            Some("C/Games/Foo/save.dat".into())
        );
        assert_eq!(
            tail_of(Path::new("\\\\server\\share\\x"), Path::new("")),
            None
        );
    }

    #[test]
    fn win_absolute_target_rebuilds_drive() {
        assert_eq!(
            win_absolute_target("C/Games/Foo/save.dat", &native()),
            Some(PathBuf::from("C:\\Games\\Foo\\save.dat"))
        );
        assert_eq!(win_absolute_target("CD/x", &native()), None);
        assert_eq!(win_absolute_target("C/../x", &native()), None);
    }

    #[test]
    fn expand_win_absolute_keeps_drive_in_tail() {
        let expanded = expand("C:\\Games\\*.sav", &native(), &no_roots_vars());
        assert_eq!(expanded, [(PathBuf::new(), "C:/Games/*.sav".to_owned())]);
    }

    #[test]
    fn absolute_has_no_target_on_windows() {
        assert_eq!(base_dir(Base::Absolute, &native(), &no_roots_vars()), None);
    }
}
