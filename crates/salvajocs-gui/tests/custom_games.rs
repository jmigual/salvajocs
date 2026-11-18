//! Headless checks of the Custom games tab.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::{
    env, harness, harness_sized, init_repo, local_config, pick, replace_text, repo_dir,
    run_settled, run_until, run_until_ready, save_config, shows,
};
use eframe::egui::Vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::config::AppConfig;
use salvajocs_core::crossbuild::{self, Choice};
use salvajocs_core::layout::{Base, Context, LinuxFolders};
use salvajocs_core::manifest::{
    self, CustomFile, CustomGame, FileRule, GameEntry, InstallDir, Integration, Manifest, Os, Pair,
    RegistryRule, Steam, Tag, When,
};
use salvajocs_core::scan::Environment;
use salvajocs_gui::tabs::Tab;
use salvajocs_gui::{App, Env};
use tempfile::TempDir;

/// An app with the repository open and the Custom games tab in front, starting from `games`
/// and a cached manifest that holds the titles `manifest_titles`.
fn open_tab(
    games: Vec<CustomGame>,
    manifest_titles: &[&str],
) -> (Harness<'static, App>, Env, TempDir) {
    open_tab_with(games, manifest_titles, |_, _| {})
}

/// Like `open_tab`, after `setup` adjusted the environment, given the temp dir.
fn open_tab_with(
    games: Vec<CustomGame>,
    manifest_titles: &[&str],
    setup: impl FnOnce(&mut Env, &Path),
) -> (Harness<'static, App>, Env, TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let mut env = env(&tmp);
    setup(&mut env, tmp.path());
    let manifest = Manifest {
        games: manifest_titles
            .iter()
            .map(|title| (title.to_string(), GameEntry::default()))
            .collect(),
    };
    std::fs::create_dir_all(&env.cache_dir).unwrap();
    std::fs::write(
        env.cache_dir.join("manifest.json"),
        manifest::to_json(&manifest).unwrap(),
    )
    .unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut config = local_config(&repository);
    config.custom_games = games;
    save_config(&env, &config);
    let mut harness = harness(env.clone());
    run_until_ready(&mut harness);
    harness.state_mut().set_tab(Tab::CustomGames);
    run_settled(&mut harness);
    (harness, env, tmp)
}

/// Waits until the saved config's custom games satisfy `ready`; saving is a background job.
fn run_until_saved(
    harness: &mut Harness<'static, App>,
    env: &Env,
    ready: impl Fn(&[CustomGame]) -> bool,
) -> Vec<CustomGame> {
    let config_dir = env.config_dir.clone();
    run_until(harness, "the config to be saved", move |_| {
        AppConfig::load(&config_dir)
            .ok()
            .flatten()
            .is_some_and(|config| ready(&config.custom_games))
    });
    AppConfig::load(&env.config_dir)
        .unwrap()
        .unwrap()
        .custom_games
}

/// The first node labelled exactly `label`, for buttons that repeat once per game.
fn first<'a>(harness: &'a Harness<'static, App>, label: &'a str) -> egui_kittest::Node<'a> {
    harness.query_all_by_label(label).next().unwrap()
}

/// A game whose kept lines carry rules the form cannot show: an OS condition and a tag.
fn kept_rules_game() -> CustomGame {
    let when = When {
        os: Some(Os::Linux),
        store: None,
    };
    CustomGame {
        name: "Game".into(),
        files: Vec::new(),
        entry: GameEntry {
            files: BTreeMap::from([(
                "<home>/.game/config".to_owned(),
                FileRule {
                    tags: vec![Tag::Config],
                    when: vec![when.clone()],
                },
            )]),
            registry: BTreeMap::from([(
                "HKEY_CURRENT_USER/Software/Game".to_owned(),
                RegistryRule {
                    tags: vec![Tag::Config],
                    when: vec![when],
                },
            )]),
            steam: Some(Steam { id: Some(42) }),
            ..GameEntry::default()
        },
        integration: Integration::Override,
        wine_prefix: None,
        pairs: vec![pair("<xdgData>/game", "<winAppData>/game")],
        skips: Vec::new(),
    }
}

/// What the restore mapping dialog saves for `title`.
fn dialog_game(title: &str) -> CustomGame {
    let mut config = local_config("unused");
    crossbuild::apply_choices(
        &mut config,
        title,
        &[(
            (Base::XdgData, "Foo".to_owned()),
            Choice::Template("<winAppData>/Foo".to_owned()),
        )],
    );
    config.custom_games.remove(0)
}

/// What the restore mapping dialog saves when the user skips a root of `title`.
fn skipped_game(title: &str) -> CustomGame {
    let mut config = local_config("unused");
    crossbuild::apply_choices(
        &mut config,
        title,
        &[((Base::XdgData, "Foo".to_owned()), Choice::Skip)],
    );
    config.custom_games.remove(0)
}

/// This machine as a native Linux system whose home is `home`.
fn linux_environment(home: &Path) -> Environment {
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

fn disabled(harness: &Harness<'static, App>, label: &str) -> bool {
    harness.get_by_label(label).accesskit_node().is_disabled()
}

fn pair(linux: &str, windows: &str) -> Pair {
    Pair {
        linux: linux.into(),
        windows: windows.into(),
    }
}

/// A game name wider than the window does not push its Remove button off screen.
#[test]
fn custom_games_long_name_keeps_its_remove_button_on_screen() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut config = local_config(&repository);
    config.custom_games = vec![dialog_game(&"A Very Long Game Title ".repeat(6))];
    save_config(&env, &config);
    let width = 400.0;
    let mut harness = harness_sized(env, Vec2::new(width, 900.0));
    run_until_ready(&mut harness);
    harness.state_mut().set_tab(Tab::CustomGames);
    run_settled(&mut harness);

    let remove = harness
        .get_by_label("Remove")
        .accesskit_node()
        .bounding_box();
    let remove = remove.expect("the Remove button has bounds");
    assert!(
        remove.x1 <= f64::from(width),
        "Remove ends at {}",
        remove.x1
    );
}

#[test]
fn custom_games_without_games_says_what_they_are_for() {
    let (harness, _env, _tmp) = open_tab(Vec::new(), &[]);
    assert!(shows(&harness, "No custom games."));
    assert!(harness.query_by_label("Add game").is_some());
}

#[test]
fn custom_games_add_game_with_a_linux_only_file_saves_the_entry_to_the_config() {
    let (mut harness, env, _tmp) = open_tab(Vec::new(), &[]);

    harness.get_by_label("Add game").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Name", "My Game");
    harness.get_by_label("Add file").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Linux path 1", "<xdgData>/mygame/*.sav");
    harness.get_by_label("Add registry key").click();
    run_settled(&mut harness);
    replace_text(
        &mut harness,
        "Registry key 1",
        "HKEY_CURRENT_USER/Software/My Game",
    );
    harness.get_by_label("Add install folder").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Install folder 1", "My Game Dir");
    replace_text(&mut harness, "Wine prefix", "/prefixes/mygame");
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);

    let saved = run_until_saved(&mut harness, &env, |games| games.len() == 1);
    assert_eq!(
        saved,
        [CustomGame {
            name: "My Game".into(),
            files: vec![CustomFile {
                linux: Some("<xdgData>/mygame/*.sav".into()),
                windows: None,
            }],
            entry: GameEntry {
                registry: BTreeMap::from([(
                    "HKEY_CURRENT_USER/Software/My Game".to_owned(),
                    RegistryRule::default(),
                )]),
                install_dir: BTreeMap::from([("My Game Dir".to_owned(), InstallDir {})]),
                ..GameEntry::default()
            },
            integration: Integration::Extend,
            wine_prefix: Some("/prefixes/mygame".into()),
            pairs: Vec::new(),
            skips: Vec::new(),
        }]
    );
    assert!(shows(&harness, "My Game (custom game)"));
    assert!(
        !shows(&harness, "Save game"),
        "the form closes after saving"
    );

    harness.get_by_label("Edit").click();
    run_settled(&mut harness);
    for (label, value) in [
        ("Linux path 1", "<xdgData>/mygame/*.sav"),
        ("Registry key 1", "HKEY_CURRENT_USER/Software/My Game"),
        ("Install folder 1", "My Game Dir"),
    ] {
        assert_eq!(
            harness.get_by_label(label).value().as_deref(),
            Some(value),
            "{label}"
        );
    }
    assert!(
        !shows(&harness, "Saved restore mappings"),
        "no mappings, no section"
    );
}

#[test]
fn custom_games_install_folder_that_is_a_path_shows_its_error_and_cannot_be_saved() {
    let (mut harness, _env, _tmp) = open_tab(Vec::new(), &[]);

    harness.get_by_label("Add game").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Name", "My Game");
    harness.get_by_label("Add install folder").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Install folder 1", "Games/My Game");
    assert!(shows(&harness, "An install folder is one folder name"));
    assert!(disabled(&harness, "Save game"));
    replace_text(&mut harness, "Install folder 1", "My Game");
    assert!(!disabled(&harness, "Save game"));
}

#[test]
fn custom_games_install_folder_makes_a_browsed_folder_base_relative() {
    let (mut harness, _env, _tmp) = open_tab_with(Vec::new(), &[], |env, tmp| {
        let steam = tmp.join("steam");
        let saves = steam.join("steamapps/common/Install Dir/saves");
        std::fs::create_dir_all(&saves).unwrap();
        let mut environment = linux_environment(&tmp.join("home"));
        environment.steam_roots = vec![steam];
        env.environment = Some(environment);
        pick(env, saves);
    });

    harness.get_by_label("Add game").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Name", "My Game");
    harness.get_by_label("Add install folder").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Install folder 1", "Install Dir");
    harness.get_by_label("Add file").click();
    run_settled(&mut harness);
    harness.get_by_label("Browse Linux path 1").click();
    run_until(&mut harness, "the picked folder", |h| {
        h.get_by_label("Linux path 1").value().as_deref() == Some("<base>/saves")
    });
}

#[test]
fn custom_games_invalid_file_row_shows_its_error_and_cannot_be_saved() {
    let (mut harness, _env, _tmp) = open_tab(Vec::new(), &[]);

    harness.get_by_label("Add game").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Name", "My Game");
    for _ in 0..2 {
        harness.get_by_label("Add file").click();
        run_settled(&mut harness);
    }
    assert!(!disabled(&harness, "Save game"), "a blank row is unused");
    replace_text(&mut harness, "Linux path 1", "<xdgData>/kept");
    replace_text(&mut harness, "Windows path 2", "<xdgData>/mygame");
    assert!(shows(&harness, "not a Windows path"));
    assert!(disabled(&harness, "Save game"));

    harness.get_by_label("Remove file 2").click();
    run_settled(&mut harness);
    assert!(!disabled(&harness, "Save game"));
    assert_eq!(
        harness.get_by_label("Linux path 1").value().as_deref(),
        Some("<xdgData>/kept")
    );
    assert!(harness.query_by_label("Linux path 2").is_none());
}

#[test]
fn custom_games_browse_fills_the_linux_path_from_the_picked_folder() {
    let (mut harness, _env, _tmp) = open_tab_with(Vec::new(), &[], |env, tmp| {
        let home = tmp.join("home");
        env.environment = Some(linux_environment(&home));
        pick(env, home.join(".local/share/my-game"));
    });

    harness.get_by_label("Add game").click();
    run_settled(&mut harness);
    harness.get_by_label("Add file").click();
    run_settled(&mut harness);
    harness.get_by_label("Browse Linux path 1").click();
    run_until(&mut harness, "the picked folder", |h| {
        h.get_by_label("Linux path 1").value().as_deref() == Some("<xdgData>/my-game")
    });
}

#[cfg(not(windows))]
#[test]
fn custom_games_browse_for_a_windows_path_outside_a_prefix_shows_an_error() {
    let (mut harness, _env, _tmp) = open_tab_with(Vec::new(), &[], |env, tmp| {
        env.environment = Some(linux_environment(&tmp.join("home")));
        pick(env, tmp.join("home/Games"));
    });

    harness.get_by_label("Add game").click();
    run_settled(&mut harness);
    harness.get_by_label("Add file").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Windows path 1", "<winAppData>/Kept");
    harness.get_by_label("Browse Windows path 1").click();
    run_until(&mut harness, "the pick to fail", |h| {
        shows(h, "Pick a folder inside a Wine prefix")
    });
    assert_eq!(
        harness.get_by_label("Windows path 1").value().as_deref(),
        Some("<winAppData>/Kept")
    );
}

#[test]
fn custom_games_mapping_that_does_not_classify_shows_error_and_is_not_saved() {
    let game = dialog_game("My Game");
    let (mut harness, env, _tmp) = open_tab(vec![game.clone()], &[]);

    harness.get_by_label("Edit").click();
    run_settled(&mut harness);
    // A Windows template on the Linux side.
    replace_text(&mut harness, "Linux 1", "<winAppData>/mygame");
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);

    assert!(shows(&harness, "not a placeholder template"));
    assert!(shows(&harness, "Save game"), "the form stays open");
    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.custom_games, [game]);
}

#[test]
fn custom_games_empty_and_duplicate_names_are_rejected() {
    let existing = CustomGame {
        name: "Taken".into(),
        files: Vec::new(),
        entry: GameEntry::default(),
        integration: Integration::Override,
        wine_prefix: None,
        pairs: Vec::new(),
        skips: Vec::new(),
    };
    let (mut harness, _env, _tmp) = open_tab(vec![existing], &[]);

    harness.get_by_label("Add game").click();
    run_settled(&mut harness);
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);
    assert!(shows(&harness, "needs a name"));

    replace_text(&mut harness, "Name", "Taken");
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);
    assert!(shows(&harness, "already a custom game named Taken"));
}

#[test]
fn custom_games_edit_keeps_what_the_form_does_not_show_and_remove_deletes() {
    let game = kept_rules_game();
    let other = dialog_game("Other");
    let (mut harness, env, _tmp) = open_tab(vec![game.clone(), other.clone()], &[]);

    first(&harness, "Edit").click();
    run_settled(&mut harness);
    assert!(shows(&harness, "Edit custom game"));
    replace_text(&mut harness, "Name", "Renamed");
    harness.get_by_label("Remove mapping").click();
    run_settled(&mut harness);
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);

    let saved = run_until_saved(&mut harness, &env, |games| {
        games.first().is_some_and(|game| game.name == "Renamed")
    });
    let renamed = CustomGame {
        name: "Renamed".into(),
        files: Vec::new(),
        pairs: Vec::new(),
        skips: Vec::new(),
        ..game
    };
    assert_eq!(saved, [renamed.clone(), other.clone()]);

    // Removing one game leaves the other as it was.
    first(&harness, "Remove").click();
    run_settled(&mut harness);
    assert_eq!(
        run_until_saved(&mut harness, &env, |games| games.len() == 1),
        [other]
    );
}

#[test]
fn custom_games_removing_an_earlier_game_keeps_the_open_form_on_its_game() {
    let (mut harness, env, _tmp) = open_tab(vec![kept_rules_game(), dialog_game("Other")], &[]);

    harness.query_all_by_label("Edit").nth(1).unwrap().click();
    run_settled(&mut harness);
    first(&harness, "Remove").click();
    run_settled(&mut harness);
    assert!(shows(&harness, "Edit custom game"), "the form stays open");
    replace_text(&mut harness, "Name", "Moved");
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);

    let saved = run_until_saved(&mut harness, &env, |games| {
        games.first().is_some_and(|game| game.name == "Moved")
    });
    assert_eq!(
        saved,
        [CustomGame {
            name: "Moved".into(),
            files: Vec::new(),
            ..dialog_game("Other")
        }]
    );
}

#[test]
fn custom_games_pair_saved_by_the_restore_dialog_can_be_edited_and_deleted() {
    // What the mapping dialog leaves behind for a manifest game the user never added.
    let (mut harness, env, _tmp) = open_tab(vec![dialog_game("Manifest Game")], &["Manifest Game"]);

    assert!(shows(&harness, "Manifest Game (extends the manifest game)"));
    harness.get_by_label("Edit").click();
    run_settled(&mut harness);
    assert_eq!(
        harness.get_by_label("Linux 1").value().as_deref(),
        Some("<xdgData>/Foo")
    );
    replace_text(&mut harness, "Windows 1", "<winLocalAppData>/Foo");
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);

    let saved = run_until_saved(&mut harness, &env, |games| {
        games
            .first()
            .is_some_and(|game| game.pairs == [pair("<xdgData>/Foo", "<winLocalAppData>/Foo")])
    });
    assert_eq!(saved[0].integration, Integration::Extend);

    harness.get_by_label("Edit").click();
    run_settled(&mut harness);
    harness.get_by_label("Remove mapping").click();
    run_settled(&mut harness);
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);
    run_until_saved(&mut harness, &env, |games| {
        games.first().is_some_and(|game| game.pairs.is_empty())
    });

    harness.get_by_label("Remove").click();
    run_settled(&mut harness);
    run_until_saved(&mut harness, &env, |games| games.is_empty());
    assert!(!shows(&harness, "Manifest Game"));
}

#[test]
fn custom_games_skip_saved_by_the_restore_dialog_can_be_edited_and_deleted() {
    let (mut harness, env, _tmp) =
        open_tab(vec![skipped_game("Manifest Game")], &["Manifest Game"]);

    harness.get_by_label("Edit").click();
    run_settled(&mut harness);
    assert_eq!(
        harness.get_by_label("Skipped folder 1").value().as_deref(),
        Some("<xdgData>/Foo")
    );
    replace_text(&mut harness, "Skipped folder 1", "<winAppData>/Foo");
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);
    let saved = run_until_saved(&mut harness, &env, |games| {
        games
            .first()
            .is_some_and(|game| game.skips == ["<winAppData>/Foo"])
    });
    assert_eq!(saved[0].integration, Integration::Extend);

    // A skip that is not a placeholder template is refused inline.
    harness.get_by_label("Edit").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Skipped folder 1", "/opt/foo");
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);
    assert!(shows(&harness, "not a placeholder template"));
    assert!(shows(&harness, "Save game"), "the form stays open");

    harness.get_by_label("Remove skipped folder").click();
    run_settled(&mut harness);
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);
    run_until_saved(&mut harness, &env, |games| {
        games.first().is_some_and(|game| game.skips.is_empty())
    });
}

#[test]
fn custom_games_placeholder_help_lists_the_placeholders_of_each_side_and_the_guide() {
    let (mut harness, _env, _tmp) = open_tab(Vec::new(), &[]);
    harness.get_by_label("Add game").click();
    run_settled(&mut harness);
    harness.get_by_label("Path placeholders").hover();
    run_until(&mut harness, "the placeholder help", |h| {
        shows(h, "<winAppData>")
    });
    assert!(shows(&harness, "<xdgData>"));
    assert!(shows(&harness, "<storeUserId>"));
    assert!(shows(&harness, "Placeholders in the guide"));
}
