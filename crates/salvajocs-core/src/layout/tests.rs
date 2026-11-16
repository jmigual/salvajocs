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
        home: "/home/me".into(),
        xdg_data: "/home/me/.local/share".into(),
        xdg_config: "/home/me/.config".into(),
    })
}

fn no_roots_vars() -> GameVars {
    GameVars::new("Game", vec![], &[], None, "me".into())
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
fn absolute_template_is_classified_absolute() {
    assert_eq!(
        classified("/opt/saves/a", &linux_ctx()),
        (Base::Absolute, "opt/saves/a".into())
    );
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
    let folders = WinFolders::for_prefix(prefix.path(), "steamuser");
    let in_prefix = Context::WinePrefix {
        prefix: prefix.path().into(),
        folders,
    };
    let native_dir = tempfile::tempdir().unwrap();
    let native = Context::WindowsNative(WinFolders::for_prefix(native_dir.path(), "me"));
    let vars = no_roots_vars();

    let template = "<home>/AppData/Roaming/Foo/slot1.sav";
    let tails: Vec<_> = [&in_prefix, &native]
        .into_iter()
        .map(|ctx| {
            let (base, rest) = classified(template, ctx);
            let dir = base_dir(base, ctx, &vars).unwrap();
            let real = dir.join(&rest);
            (base, tail_of(&real, &dir).unwrap())
        })
        .collect();
    assert_eq!(tails[0], tails[1]);
    assert_eq!(tails[0], (Base::WinAppData, "Foo/slot1.sav".to_owned()));
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
        "me".into(),
    );
    let found = base_dir(Base::GameDir, &linux_ctx(), &vars);
    assert_eq!(found, Some(steam.path().join("steamapps/common/Real Dir")));
    assert_eq!(
        base_dir(Base::StoreRoot, &linux_ctx(), &vars),
        Some(other.path().into())
    );

    let not_installed = GameVars::new("Title", vec![], &roots, None, "me".into());
    assert_eq!(base_dir(Base::GameDir, &linux_ctx(), &not_installed), None);
}

#[test]
fn install_dir_falls_back_to_title() {
    let root = tempfile::tempdir().unwrap();
    let roots = [Root {
        path: root.path().into(),
        kind: RootKind::Other,
    }];
    let vars = GameVars::new("Title", vec![], &roots, None, "me".into());
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
        expand(template, &prefix_ctx(), &vars),
        ["/pfx/drive_c/users/steamuser/AppData/Roaming/[fF][oO][oO]/*.[sS][aA][vV]"]
    );
    assert_eq!(
        expand(template, &native_win_ctx(), &vars),
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
        "me".into(),
    );
    let ctx = linux_ctx();

    assert_eq!(
        expand(
            "<home>/<game>/<osUserName>/<storeGameId>/<storeUserId>/x",
            &ctx,
            &vars
        ),
        [
            "/home/me/Game [[]GOTY[]]/me/42/*/x",
            "/home/me/Game/me/42/*/x",
        ]
    );
    let base = steam.path().join("steamapps/common");
    let patterns = expand("<base>/saves", &ctx, &vars);
    assert_eq!(
        patterns,
        [
            format!("{}/Game [[]GOTY[]]/saves", base.display()),
            format!("{}/Game/saves", base.display()),
        ]
    );
    assert!(expand("<home>/<storeGameId>", &ctx, &no_roots_vars()).is_empty());
    assert!(expand("<base>/saves", &ctx, &no_roots_vars()).is_empty());
}

#[cfg(unix)]
#[test]
fn expand_absolute_has_single_slash() {
    let patterns = expand("/opt/a/*.sav", &linux_ctx(), &no_roots_vars());
    assert_eq!(patterns, ["/opt/a/*.sav"]);
}

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
