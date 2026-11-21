//! Headless checks of the Custom games tab.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::{
    back_up_unsent, env, harness, harness_sized, init_repo, local_config, replace_text, repo_dir,
    run_settled, run_until, run_until_ready, save_config, shows,
};
use eframe::egui::{Key, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use rustic_backend::BackendOptions;
use rustic_core::{ConfigOptions, Credentials, Repository, RepositoryOptions};
use salvajocs_core::config::AppConfig;
use salvajocs_core::crossbuild::{self, Choice};
use salvajocs_core::custom_games::{self, Stored};
use salvajocs_core::layout::Base;
#[cfg(not(windows))]
use salvajocs_core::layout::{Context, LinuxFolders};
use salvajocs_core::manifest::{
    self, CustomFile, CustomGame, FileRule, GameEntry, InstallDir, Integration, Manifest, Os, Pair,
    RegistryRule, Steam, Tag, When,
};
#[cfg(not(windows))]
use salvajocs_core::scan::Environment;
use salvajocs_core::slug::slug;
use salvajocs_core::storage::{RepoLocation, SnapshotInfo, Storage};
use salvajocs_gui::tabs::Tab;
use salvajocs_gui::{App, Env};
use tempfile::TempDir;

#[cfg(not(windows))]
use common::pick;

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
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let harness = start(env.clone(), &repository, games, manifest_titles);
    (harness, env, tmp)
}

/// The app of `env` on `repository` with the Custom games tab in front, starting from `games`
/// and a cached manifest that holds the titles `manifest_titles`. A second PC is another `env`
/// on the same repository.
fn start(
    env: Env,
    repository: &str,
    games: Vec<CustomGame>,
    manifest_titles: &[&str],
) -> Harness<'static, App> {
    let mut manifest = Manifest::default();
    manifest.games = manifest_titles
        .iter()
        .map(|title| (title.to_string(), GameEntry::default()))
        .collect();
    std::fs::create_dir_all(&env.cache_dir).unwrap();
    std::fs::write(
        env.cache_dir
            .join(salvajocs_core::manifest::fetch::JSON_FILE),
        manifest::to_json(&manifest).unwrap(),
    )
    .unwrap();
    let mut config = local_config(repository);
    config.custom_games = games;
    save_config(&env, &config);
    let mut harness = harness(env);
    run_until_ready(&mut harness);
    harness.state_mut().set_tab(Tab::CustomGames);
    // The status bar's spinner keeps the frames coming until the custom games are read.
    run_settled(&mut harness);
    harness
}

/// The repository of `tmp` as another PC opens it.
fn other_pc(tmp: &TempDir) -> Storage {
    let location = RepoLocation::new(repo_dir(tmp), Default::default());
    Storage::open(&location, "").unwrap()
}

/// Saves `game` (`None`: removes it) as `name` in the repository of `tmp`, as another PC does.
fn save_elsewhere(
    tmp: &TempDir,
    name: &str,
    based_on: Option<&CustomGame>,
    game: Option<&CustomGame>,
) {
    let stage = tmp.path().join("elsewhere");
    custom_games::save(&mut other_pc(tmp), name, based_on, game, &stage).unwrap();
}

/// The custom games the repository of `tmp` holds, by name.
fn in_repository(tmp: &TempDir) -> BTreeMap<String, Stored> {
    let listed = custom_games::load_all(&mut other_pc(tmp)).unwrap();
    (listed.into_iter())
        .map(|(name, stored)| (name, stored.unwrap()))
        .collect()
}

/// `game` as the repository holds it: without its Wine prefix.
fn shared(game: &CustomGame) -> Stored {
    let mut game = game.clone();
    game.wine_prefix = None;
    Stored::Game(Box::new(game))
}

/// The settings snapshots in the repository of `tmp`.
fn settings_snapshots(tmp: &TempDir) -> Vec<SnapshotInfo> {
    other_pc(tmp).snapshots("salvajocs-settings").unwrap()
}

/// Makes the upload of the custom game `name` fail: its file is staged where a directory is.
fn block_upload(env: &Env, name: &str) -> std::path::PathBuf {
    let dir = env.cache_dir.join("stage").join("settings");
    let staged = dir
        .join("custom-games")
        .join(slug(name))
        .join("custom-game.json");
    if staged.is_file() {
        std::fs::remove_file(&staged).unwrap();
    }
    std::fs::create_dir_all(&staged).unwrap();
    staged
}

/// Makes the repository of `tmp` append-only, so forgetting a snapshot fails.
fn set_append_only(tmp: &TempDir) {
    let backends = (BackendOptions::default().repository(repo_dir(tmp)))
        .to_backends()
        .unwrap();
    let mut repo = Repository::new(&RepositoryOptions::default(), &backends)
        .unwrap()
        .open(&Credentials::password(""))
        .unwrap();
    repo.apply_config(&ConfigOptions::default().set_append_only(true))
        .unwrap();
}

/// Waits until the saved config marks its custom games as this repository's.
fn run_until_read(harness: &mut Harness<'static, App>, env: &Env) {
    let config_dir = env.config_dir.clone();
    run_until(harness, "the custom games to be read", move |_| {
        AppConfig::load(&config_dir)
            .ok()
            .flatten()
            .is_some_and(|config| config.custom_games_repository.is_some())
    });
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
    let mut when = When::default();
    when.os = Some(Os::Linux);
    let mut file = FileRule::default();
    file.tags = vec![Tag::Config];
    file.when = vec![when.clone()];
    let mut key = RegistryRule::default();
    key.tags = vec![Tag::Config];
    key.when = vec![when];
    let mut steam = Steam::default();
    steam.id = Some(42);
    let mut entry = GameEntry::default();
    entry.files = BTreeMap::from([("<home>/.game/config".to_owned(), file)]);
    entry.registry = BTreeMap::from([("HKEY_CURRENT_USER/Software/Game".to_owned(), key)]);
    entry.steam = Some(steam);
    let mut game = CustomGame::new("Game".into(), entry, Integration::Override);
    game.pairs = vec![pair("<xdgData>/game", "<winAppData>/game")];
    game
}

/// What the restore mapping dialog saves for `title`.
fn dialog_game(title: &str) -> CustomGame {
    let mut config = local_config("unused");
    crossbuild::apply_choices(
        &mut config.custom_games,
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
        &mut config.custom_games,
        title,
        &[((Base::XdgData, "Foo".to_owned()), Choice::Skip)],
    );
    config.custom_games.remove(0)
}

/// This machine as a native Linux system whose home is `home`.
#[cfg(not(windows))]
fn linux_environment(home: &Path) -> Environment {
    let mut environment = Environment::default();
    environment.native = Some(Context::Linux(LinuxFolders::new(
        "me".into(),
        home.into(),
        home.join(".local/share"),
        home.join(".config"),
    )));
    environment
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
    let mut entry = GameEntry::default();
    entry.registry = BTreeMap::from([(
        "HKEY_CURRENT_USER/Software/My Game".to_owned(),
        RegistryRule::default(),
    )]);
    entry.install_dir = BTreeMap::from([("My Game Dir".to_owned(), InstallDir::default())]);
    let mut game = CustomGame::new("My Game".into(), entry, Integration::Extend);
    game.files = vec![CustomFile {
        linux: Some("<xdgData>/mygame/*.sav".into()),
        windows: None,
    }];
    game.wine_prefix = Some("/prefixes/mygame".into());
    assert_eq!(saved, [game]);
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

#[cfg(not(windows))]
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

#[cfg(not(windows))]
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
    let existing = CustomGame::new("Taken".into(), GameEntry::default(), Integration::Override);
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
    let (mut harness, env, tmp) = open_tab(vec![game.clone(), other.clone()], &[]);
    back_up(&mut harness, "Saved 2 custom games to the repository");

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
    let mut renamed = game;
    renamed.name = "Renamed".into();
    renamed.files = Vec::new();
    renamed.pairs = Vec::new();
    renamed.skips = Vec::new();
    assert_eq!(saved, [renamed.clone(), other.clone()]);
    back_up(&mut harness, "Saved 2 custom games to the repository");
    // The repository marks the old name as removed, so another PC lists only the new one.
    let repository = in_repository(&tmp);
    assert_eq!(repository["Game"], Stored::Removed);
    assert_eq!(repository["Renamed"], shared(&renamed));
    let other_pc = tempfile::tempdir().unwrap();
    let second = start(common::env(&other_pc), &repo_dir(&tmp), Vec::new(), &[]);
    assert!(shows(&second, "Renamed ("));
    assert!(!shows(&second, "Game ("));

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
    let mut moved = dialog_game("Other");
    moved.name = "Moved".into();
    moved.files = Vec::new();
    assert_eq!(saved, [moved]);
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
    assert!(!shows(&harness, "Manifest Game ("));
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
fn custom_games_placeholder_help_opens_from_the_keyboard_and_lists_each_side_once() {
    let (mut harness, _env, _tmp) = open_tab(Vec::new(), &[]);
    harness.get_by_label("Add game").click();
    run_settled(&mut harness);
    harness.get_by_label("Path placeholders").focus();
    harness.step();
    harness.key_press(Key::Enter);
    run_until(&mut harness, "the placeholder help", |h| {
        shows(h, "<winAppData>")
    });
    let count = |h: &Harness<'static, App>, text: &str| h.query_all_by_label_contains(text).count();
    // Only the Windows block lists the Windows folders and only the Linux block the XDG ones.
    assert_eq!(count(&harness, "<winAppData>"), 1);
    assert_eq!(count(&harness, "<xdgData>"), 1);
    assert!(shows(&harness, "<storeUserId>"));
    assert!(shows(&harness, "Placeholders in the guide"));
}

#[test]
fn custom_games_added_on_another_pc_show_after_the_repository_opens() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let game = dialog_game("Elsewhere");
    save_elsewhere(&tmp, "Elsewhere", None, Some(&game));

    let mut harness = start(env.clone(), &repository, Vec::new(), &[]);

    assert!(shows(&harness, "Elsewhere (custom game)"));
    assert_eq!(
        run_until_saved(&mut harness, &env, |games| games.len() == 1),
        [game]
    );
}

#[test]
fn custom_games_save_stays_on_this_pc_until_back_up() {
    let (mut harness, env, tmp) = open_tab(Vec::new(), &[]);
    assert!(shows(
        &harness,
        "Changes stay on this PC until the next Back up"
    ));

    harness.get_by_label("Add game").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Name", "My Game");
    harness.get_by_label("Add file").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Linux path 1", "<xdgData>/mygame");
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);

    let saved = run_until_saved(&mut harness, &env, |games| games.len() == 1);
    assert!(!shows(&harness, "Save game"), "the form closes once saved");
    assert!(shows(&harness, "Not backed up yet"));
    assert!(in_repository(&tmp).is_empty());

    back_up(&mut harness, "Saved 1 custom game to the repository");
    assert_eq!(
        in_repository(&tmp),
        BTreeMap::from([("My Game".to_owned(), shared(&saved[0]))])
    );
    assert!(!shows(&harness, "Not backed up yet"));
}

#[test]
fn custom_games_remove_on_one_pc_removes_it_on_the_other() {
    let (mut first, first_env, tmp) = open_tab(vec![dialog_game("Shared")], &[]);
    run_until_read(&mut first, &first_env);
    back_up(&mut first, "Saved 1 custom game to the repository");
    let other = tempfile::tempdir().unwrap();
    let mut second = start(env(&other), &repo_dir(&tmp), Vec::new(), &[]);
    assert!(shows(&second, "Shared (custom game)"));

    first.get_by_label("Remove").click();
    run_until(&mut first, "the removal", |h| {
        !shows(h, "Shared (custom game)")
    });
    assert!(shows(
        &first,
        "Removed here; other PCs keep them until the next Back up: Shared"
    ));
    assert!(matches!(in_repository(&tmp)["Shared"], Stored::Game(_)));
    back_up(&mut first, "Saved 1 custom game to the repository");
    assert_eq!(in_repository(&tmp)["Shared"], Stored::Removed);
    assert!(!shows(&first, "Removed here"));

    // The other PC reads the games again when asked.
    second.get_by_label("Read repository").click();
    run_until(&mut second, "the removal on the other PC", |h| {
        shows(h, "No custom games.")
    });
}

#[test]
fn custom_games_first_open_writes_nothing_and_back_up_uploads_them_once() {
    let (mut first, env, tmp) = open_tab(vec![dialog_game("Mine")], &[]);
    run_until_read(&mut first, &env);
    assert!(settings_snapshots(&tmp).is_empty());
    assert!(shows(&first, "Not backed up yet"));

    back_up(&mut first, "Saved 1 custom game to the repository");
    assert_eq!(
        in_repository(&tmp),
        BTreeMap::from([("Mine".to_owned(), shared(&dialog_game("Mine")))])
    );
    let written = settings_snapshots(&tmp);
    drop(first);

    // The next start mirrors the repository: nothing is unsent.
    let mut again = harness(env.clone());
    run_until_ready(&mut again);
    again.state_mut().set_tab(Tab::CustomGames);
    run_settled(&mut again);
    assert!(shows(&again, "Mine (custom game)"));
    assert!(!shows(&again, "Not backed up yet"));
    assert_eq!(settings_snapshots(&tmp), written);
}

/// Starts this PC with `mine` on a repository where another PC holds `theirs` (`None`: a game
/// it removed) of the same name, presses Back up and waits for the question.
fn first_open_with_another_version(
    mine: &CustomGame,
    theirs: Option<&CustomGame>,
) -> (Harness<'static, App>, Env, TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let base = dialog_game(&mine.name);
    save_elsewhere(&tmp, &mine.name, None, Some(theirs.unwrap_or(&base)));
    if theirs.is_none() {
        save_elsewhere(&tmp, &mine.name, Some(&base), None);
    }
    let mut harness = start(env.clone(), &repository, vec![mine.clone()], &[]);
    run_until_read(&mut harness, &env);
    let config = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(
        config.custom_games,
        std::slice::from_ref(mine),
        "this PC keeps its version"
    );
    assert!(shows(&harness, "Not backed up yet"));

    harness.state_mut().set_tab(Tab::Backup);
    run_until(&mut harness, "Back up to be enabled", |h| {
        h.query_by_label("Back up")
            .is_some_and(|button| !button.accesskit_node().is_disabled())
    });
    harness.get_by_label("Back up").click();
    run_until(&mut harness, "the question", |h| {
        shows(h, "Changed on another PC")
    });
    (harness, env, tmp)
}

/// Clicks the dialog's own Back up, the enabled one, and waits for the backup to end.
fn answer_and_back_up(harness: &mut Harness<'static, App>, keep: &str) {
    harness.get_by_label(keep).click();
    run_settled(harness);
    let button = harness
        .query_all_by_label("Back up")
        .find(|node| !node.accesskit_node().is_disabled())
        .unwrap();
    button.click();
    run_until(harness, "the backup", |h| {
        !shows(h, "Changed on another PC") && !shows(h, "Saving the custom games")
    });
    run_settled(harness);
}

#[test]
fn custom_games_first_open_keeps_this_pcs_version_and_back_up_asks() {
    let mut theirs = dialog_game("Shared");
    theirs.integration = Integration::Override;
    let mut mine = dialog_game("Shared");
    mine.wine_prefix = Some("/prefixes/shared".into());
    let (mut harness, env, tmp) = first_open_with_another_version(&mine, Some(&theirs));
    assert!(shows(&harness, "Integration: overrides the manifest entry"));
    assert!(shows(&harness, "Integration: extends the manifest entry"));

    answer_and_back_up(&mut harness, "Keep this PC's");
    assert_eq!(in_repository(&tmp)["Shared"], shared(&mine));
    let config = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(config.custom_games, [mine]);
    assert!(config.custom_games_unsent.is_empty());
}

#[test]
fn custom_games_first_open_keeps_a_game_removed_elsewhere_and_back_up_asks() {
    let mut mine = dialog_game("Gone");
    mine.wine_prefix = Some("/prefixes/gone".into());
    let (mut harness, env, tmp) = first_open_with_another_version(&mine, None);
    assert!(shows(&harness, "Removed"));
    let written = settings_snapshots(&tmp);

    answer_and_back_up(&mut harness, "Keep the repository's");
    assert_eq!(in_repository(&tmp)["Gone"], Stored::Removed);
    assert_eq!(settings_snapshots(&tmp), written);
    let config = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert!(config.custom_games.is_empty());
    assert!(config.custom_games_unsent.is_empty());
}

#[test]
fn custom_games_wine_prefix_stays_on_this_pc() {
    let (mut harness, env, tmp) = open_tab(Vec::new(), &[]);
    let prefix = tmp.path().join("prefixes").join("game");
    harness.get_by_label("Add game").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Name", "Prefixed");
    replace_text(&mut harness, "Wine prefix", prefix.to_str().unwrap());
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);

    let saved = run_until_saved(&mut harness, &env, |games| games.len() == 1);
    assert_eq!(saved[0].wine_prefix.as_deref(), Some(prefix.as_path()));
    back_up(&mut harness, "Saved 1 custom game to the repository");
    assert_eq!(in_repository(&tmp)["Prefixed"], shared(&saved[0]));

    // Another PC gets the game without this PC's prefix.
    let other = tempfile::tempdir().unwrap();
    let other_env = common::env(&other);
    let mut second = start(other_env.clone(), &repo_dir(&tmp), Vec::new(), &[]);
    let theirs = run_until_saved(&mut second, &other_env, |games| games.len() == 1);
    assert_eq!(theirs[0].wine_prefix, None);

    // A new prefix alone stays on this PC: nothing is left to save.
    let moved = tmp.path().join("prefixes").join("moved");
    harness.get_by_label("Edit").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Wine prefix", moved.to_str().unwrap());
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);
    run_until_saved(&mut harness, &env, |games| {
        games[0].wine_prefix.as_deref() == Some(moved.as_path())
    });
    assert!(!shows(&harness, "Not backed up yet"));
}

/// A Wine prefix in `dir` holding a save of the game `name` below `AppData/Roaming/<name>`, and
/// a custom game that finds it there.
fn game_in_prefix(dir: &Path, name: &str) -> CustomGame {
    let prefix = dir.join("pfx");
    let saves = prefix
        .join("drive_c/users/steamuser/AppData/Roaming")
        .join(name);
    std::fs::create_dir_all(&saves).unwrap();
    std::fs::write(saves.join("save.dat"), "abc").unwrap();
    std::fs::write(prefix.join("user.reg"), "WINE REGISTRY Version 2\n").unwrap();
    let yaml = format!("{name}:\n  files:\n    <winAppData>/{name}: {{}}\n");
    let entry = manifest::parse_yaml(&yaml)
        .unwrap()
        .games
        .remove(name)
        .unwrap();
    let mut game = CustomGame::new(name.into(), entry, Integration::Override);
    game.wine_prefix = Some(prefix);
    game
}

#[test]
fn custom_games_without_a_repository_can_be_edited_and_are_scanned() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let game = game_in_prefix(tmp.path(), "Offline");
    // No repository at the configured location.
    let mut harness = start(env.clone(), &repo_dir(&tmp), vec![game], &[]);
    run_until(&mut harness, "the missing repository", |h| {
        shows(h, "No repository at")
    });

    assert!(shows(&harness, "Offline (custom game)"));
    for button in ["Add game", "Edit", "Remove"] {
        assert!(!disabled(&harness, button), "{button}");
    }
    assert!(disabled(&harness, "Read repository"));

    harness.get_by_label("Add game").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Name", "Added offline");
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);
    run_until(&mut harness, "the unsent change to be saved", |_| {
        AppConfig::load(&env.config_dir)
            .ok()
            .flatten()
            .is_some_and(|config| config.custom_games_unsent.len() == 1)
    });
    assert!(shows(&harness, "Not backed up yet"));

    harness.state_mut().set_tab(Tab::Backup);
    run_settled(&mut harness);
    harness.get_by_label("Scan for games").click();
    run_until(&mut harness, "the scan", |h| shows(h, "1 game found"));
}

#[test]
fn custom_games_form_opened_before_a_read_asks_to_reload() {
    let game = dialog_game("Game");
    let (mut harness, env, tmp) = open_tab(vec![game.clone()], &[]);
    back_up(&mut harness, "Saved 1 custom game to the repository");
    harness.get_by_label("Edit").click();
    run_settled(&mut harness);

    // Another PC changes the game, and this PC reads it while the form is open.
    let mut theirs = game.clone();
    theirs.skips = vec!["<xdgData>/Theirs".into()];
    save_elsewhere(&tmp, "Game", Some(&game), Some(&theirs));
    harness.get_by_label("Read repository").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Windows 1", "<winLocalAppData>/Foo");
    harness.get_by_label("Save game").click();
    run_until(&mut harness, "the conflict", |h| {
        shows(h, "This game changed since the form was opened")
    });

    harness.get_by_label("Reload").click();
    run_settled(&mut harness);
    assert_eq!(
        harness.get_by_label("Skipped folder 1").value().as_deref(),
        Some("<xdgData>/Theirs")
    );
    replace_text(&mut harness, "Windows 1", "<winLocalAppData>/Foo");
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);

    let saved = run_until_saved(&mut harness, &env, |games| {
        games[0].pairs == [pair("<xdgData>/Foo", "<winLocalAppData>/Foo")]
    });
    assert_eq!(saved[0].skips, ["<xdgData>/Theirs"]);
    // Based on the version read, so Back up saves it without asking.
    back_up(&mut harness, "Saved 1 custom game to the repository");
    assert_eq!(in_repository(&tmp)["Game"], shared(&saved[0]));
}

#[test]
fn custom_games_read_repository_keeps_a_game_not_backed_up_yet() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let base = dialog_game("Game");
    save_elsewhere(&tmp, "Game", None, Some(&base));
    let mut mine = base.clone();
    mine.skips = vec!["<xdgData>/Mine".into()];
    let mut config = local_config(&repository);
    config.custom_games = vec![mine.clone()];
    config.custom_games_repository = Some(config.secret_scope());
    config.custom_games_unsent = vec![custom_games::Unsent::new("Game".into(), Some(base.clone()))];
    save_config(&env, &config);
    let mut harness = harness(env.clone());
    run_until_ready(&mut harness);
    harness.state_mut().set_tab(Tab::CustomGames);
    run_settled(&mut harness);

    let mut theirs = base.clone();
    theirs.skips = vec!["<xdgData>/Theirs".into()];
    save_elsewhere(&tmp, "Game", Some(&base), Some(&theirs));
    harness.get_by_label("Read repository").click();
    run_settled(&mut harness);

    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.custom_games, [mine]);
    assert_eq!(saved.custom_games_unsent, config.custom_games_unsent);
    assert!(shows(&harness, "Not backed up yet"));
}

#[test]
fn custom_games_written_by_a_newer_version_cannot_be_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    // A newer version's file, as it would store it.
    let slug = slug("Newer");
    let dir = tmp.path().join("newer");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("custom-game.json"), r#"{ "format": 2 }"#).unwrap();
    let tag = format!("custom-game:{slug}");
    let as_path = format!("/salvajocs-settings/custom-games/{slug}");
    let tags = ["salvajocs-settings", tag.as_str()];
    let snapshot = other_pc(&tmp).backup_dir(&dir, &as_path, &tags, "Newer");
    snapshot.unwrap().unwrap();

    let local = dialog_game("Newer");
    let harness = start(env.clone(), &repository, vec![local.clone()], &[]);

    assert!(shows(&harness, "Newer (custom game)"));
    assert!(shows(&harness, "Written by a newer salvajocs"));
    assert!(disabled(&harness, "Edit"));
    assert!(disabled(&harness, "Remove"));
    assert_eq!(
        AppConfig::load(&env.config_dir)
            .unwrap()
            .unwrap()
            .custom_games,
        [local]
    );
}

#[test]
fn custom_games_rename_whose_new_name_is_not_saved_keeps_the_old_one() {
    let (mut harness, env, tmp) = open_tab(vec![dialog_game("Old")], &[]);
    back_up(&mut harness, "Saved 1 custom game to the repository");
    block_upload(&env, "New");

    harness.get_by_label("Edit").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Name", "New");
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);
    back_up(&mut harness, "New was not saved to the repository");

    harness.state_mut().set_tab(Tab::Backup);
    run_settled(&mut harness);
    assert!(shows(&harness, "Old was not removed from the repository"));
    let repository = in_repository(&tmp);
    assert!(matches!(repository["Old"], Stored::Game(_)));
    assert!(!repository.contains_key("New"));
    let config = AppConfig::load(&env.config_dir).unwrap().unwrap();
    let names: Vec<&str> = (config.custom_games_unsent.iter())
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["Old", "New"], "both stay unsent");
}

#[test]
fn custom_games_remove_in_an_append_only_repository_says_the_old_copy_stays() {
    let (mut harness, _env, tmp) = open_tab(vec![dialog_game("Gone")], &[]);
    back_up(&mut harness, "Saved 1 custom game to the repository");
    set_append_only(&tmp);

    harness.get_by_label("Remove").click();
    run_settled(&mut harness);
    back_up(
        &mut harness,
        "The custom game Gone was saved, but its older copies could not be removed",
    );
    assert_eq!(in_repository(&tmp)["Gone"], Stored::Removed);
    assert_eq!(settings_snapshots(&tmp).len(), 2, "the old copy stays");
}

/// [`back_up_unsent`], then back to the Custom games tab.
fn back_up(harness: &mut Harness<'static, App>, result: &str) {
    back_up_unsent(harness, result);
    harness.state_mut().set_tab(Tab::CustomGames);
    run_settled(harness);
}

#[test]
fn custom_games_only_back_up_writes_them_to_the_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    save_elsewhere(&tmp, "Theirs", None, Some(&dialog_game("Theirs")));
    let written = settings_snapshots(&tmp);
    let unchanged = |harness: &Harness<'static, App>, after: &str| {
        assert!(shows(harness, "Mine (custom game)"), "{after}");
        assert_eq!(settings_snapshots(&tmp), written, "after {after}");
    };

    // This PC's game from before 0.8 meets another PC's in the repository.
    let mut harness = start(env.clone(), &repository, vec![dialog_game("Mine")], &[]);
    assert!(shows(&harness, "Theirs (custom game)"));
    unchanged(&harness, "the open");

    harness.state_mut().set_tab(Tab::Backup);
    run_settled(&mut harness);
    harness.state_mut().set_tab(Tab::CustomGames);
    run_settled(&mut harness);
    unchanged(&harness, "the tab was shown again");

    harness.get_by_label("Read repository").click();
    run_settled(&mut harness);
    unchanged(&harness, "Read repository");

    harness.get_by_label("Add game").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Name", "Added");
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);
    assert!(shows(&harness, "Added (custom game)"));
    unchanged(&harness, "Save game");

    // The rows are Mine, Theirs and Added.
    harness.query_all_by_label("Remove").nth(1).unwrap().click();
    run_settled(&mut harness);
    assert!(!shows(&harness, "Theirs (custom game)"));
    unchanged(&harness, "Remove");

    harness.query_all_by_label("Edit").nth(1).unwrap().click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Name", "Renamed");
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);
    assert!(shows(&harness, "Renamed (custom game)"));
    unchanged(&harness, "the rename");
    assert!(shows(&harness, "Not backed up yet"));
    // Added never reached the repository, so only Theirs is waiting to be removed there.
    let removed = "Removed here; other PCs keep them until the next Back up: ";
    assert!(shows(&harness, &format!("{removed}Theirs")));
    assert!(!shows(&harness, "Theirs, Added"));

    back_up(&mut harness, "Saved 3 custom games to the repository");
    let stored = in_repository(&tmp);
    assert_eq!(stored["Mine"], shared(&dialog_game("Mine")));
    assert_eq!(stored["Theirs"], Stored::Removed);
    assert!(matches!(stored["Renamed"], Stored::Game(_)));
    assert!(!stored.contains_key("Added"), "renamed before the Back up");
    assert!(!shows(&harness, "Not backed up yet"));
    let config = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert!(config.custom_games_unsent.is_empty());
}

#[test]
fn custom_games_a_mirror_keeps_a_game_the_repository_never_got() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    // A config of salvajocs 0.8.0, marked as mirroring this repository, with a game it lacks.
    let mut config = local_config(&repository);
    config.custom_games = vec![dialog_game("Lost")];
    config.custom_games_repository = Some(config.secret_scope());
    save_config(&env, &config);
    let file = env.config_dir.join("config.yaml");
    let yaml = std::fs::read_to_string(&file).unwrap();
    assert!(yaml.contains("custom_games_read_from"));
    let old = yaml.replace("custom_games_read_from", "custom_games_repository");
    std::fs::write(&file, old).unwrap();

    let mut harness = harness(env.clone());
    run_until_ready(&mut harness);
    harness.state_mut().set_tab(Tab::CustomGames);
    run_settled(&mut harness);
    assert!(shows(&harness, "Lost (custom game)"));
    assert!(shows(&harness, "Not backed up yet"));

    back_up(&mut harness, "Saved 1 custom game to the repository");
    assert_eq!(in_repository(&tmp)["Lost"], shared(&dialog_game("Lost")));
}
