use super::*;
use salvajocs_core::layout::{LinuxFolders, Root, RootKind};
use salvajocs_core::manifest::GameEntry;

fn game(name: &str) -> CustomGame {
    CustomGame {
        name: name.into(),
        files: vec![CustomFile {
            linux: Some("<xdgData>/Foo/*.sav".into()),
            windows: Some(r"C:\Games\Foo".into()),
        }],
        entry: GameEntry::default(),
        integration: Integration::Extend,
        wine_prefix: None,
        pairs: vec![Pair {
            linux: "<xdgData>/Foo".into(),
            windows: "<winAppData>/Foo".into(),
        }],
        skips: Vec::new(),
    }
}

#[test]
fn a_game_changed_since_the_form_opened_is_not_overwritten() {
    let mut games = vec![game("Game")];
    let form = Form::edit(0, &games[0]);
    assert_eq!(form.build(&games), Ok(games[0].clone()));

    games[0].pairs.push(Pair {
        linux: "<xdgData>/Bar".into(),
        windows: "<winAppData>/Bar".into(),
    });
    assert_eq!(form.build(&games), Err(FormError::Changed));
    games.clear();
    assert_eq!(form.build(&games), Err(FormError::Changed));
}

#[cfg(unix)]
#[test]
fn an_untouched_wine_prefix_keeps_its_exact_path() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let mut stored = game("Game");
    stored.wine_prefix = Some(PathBuf::from(OsStr::from_bytes(b"/prefixes/caf\xe9")));
    let mut form = Form::edit(0, &stored);
    assert_eq!(form.build(&[stored.clone()]), Ok(stored.clone()));

    form.wine_prefix = "/prefixes/other".into();
    let built = form.build(&[stored]).unwrap();
    assert_eq!(built.wine_prefix, Some(PathBuf::from("/prefixes/other")));
}

#[test]
fn an_invalid_file_row_is_not_built_and_a_blank_one_is_left_out() {
    let mut form = Form::new();
    form.name = "Game".into();
    form.files = vec![
        FileRow::default(),
        FileRow {
            linux: " <xdgData>/game ".into(),
            ..FileRow::default()
        },
    ];
    let built = form.build(&[]).unwrap();
    assert_eq!(
        built.files,
        [CustomFile {
            linux: Some("<xdgData>/game".into()),
            windows: None,
        }]
    );

    form.files[1].windows = "relative/game".into();
    assert!(matches!(
        form.build(&[]),
        Err(FormError::File(FileError::Side { .. }))
    ));
}

fn linux_env(home: &Path) -> Environment {
    Environment {
        native: Some(Context::Linux(LinuxFolders {
            user: "me".into(),
            home: home.into(),
            xdg_data: home.join(".local/share"),
            xdg_config: home.join(".config"),
        })),
        steam_roots: Vec::new(),
    }
}

fn no_install_dirs() -> GameVars {
    GameVars::new("Game", Vec::new(), &[], None)
}

#[test]
fn a_linux_pick_becomes_a_linux_template() {
    let home = Path::new("/home/me");
    let picked = picked_template(
        &home.join(".local/share/my-game"),
        Family::Linux,
        &linux_env(home),
        &no_install_dirs(),
    );
    assert_eq!(picked.unwrap(), "<xdgData>/my-game");
}

#[test]
fn an_install_folder_makes_a_pick_under_it_base_relative() {
    let games = Path::new("/games");
    let roots = [Root {
        path: games.into(),
        kind: RootKind::Other,
    }];
    let vars = GameVars::new("Game", vec!["Install Dir".into()], &roots, None);
    let env = linux_env(Path::new("/home/me"));
    let picked = picked_template(&games.join("Install Dir/saves"), Family::Linux, &env, &vars);
    assert_eq!(picked.unwrap(), "<base>/saves");
    let picked = picked_template(
        &games.join("Install Dir/saves"),
        Family::Linux,
        &env,
        &no_install_dirs(),
    );
    assert_eq!(picked.unwrap(), "/games/Install Dir/saves");
}

#[cfg(not(windows))]
#[test]
fn a_windows_pick_inside_a_prefix_becomes_a_windows_template() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = tmp.path().join("pfx");
    std::fs::create_dir_all(prefix.join("drive_c/users/steamuser")).unwrap();
    std::fs::write(prefix.join("user.reg"), "WINE REGISTRY Version 2\n").unwrap();
    let picked = picked_template(
        &prefix.join("drive_c/users/steamuser/AppData/Roaming/Foo"),
        Family::Windows,
        &linux_env(Path::new("/home/me")),
        &no_install_dirs(),
    );
    assert_eq!(picked.unwrap(), "<winAppData>/Foo");
}

#[cfg(not(windows))]
#[test]
fn a_windows_pick_outside_a_prefix_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let picked = picked_template(
        tmp.path(),
        Family::Windows,
        &linux_env(Path::new("/home/me")),
        &no_install_dirs(),
    );
    assert!(picked.is_err());
}

#[test]
fn every_leading_placeholder_has_a_meaning() {
    for windows in [false, true] {
        for placeholder in layout::leading_placeholders(windows) {
            assert!(
                !placeholder_meaning(placeholder, windows).is_empty(),
                "{placeholder}"
            );
        }
    }
}

#[test]
fn the_placeholders_link_follows_the_running_versions_tag() {
    assert!(PLACEHOLDERS_DOCS.contains(&format!("/blob/v{}/", env!("CARGO_PKG_VERSION"))));
    assert!(PLACEHOLDERS_DOCS.ends_with("/docs/guide/custom-games.md#placeholders"));
}

#[test]
fn home_means_the_windows_user_folder_only_on_the_windows_side() {
    assert_ne!(
        placeholder_meaning("<home>", true),
        placeholder_meaning("<home>", false)
    );
}
