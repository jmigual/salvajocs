//! Headless checks of the Custom games tab.

mod common;

use std::collections::BTreeMap;

use common::{
    env, harness, init_repo, local_config, replace_text, repo_dir, run_settled, run_until,
    run_until_ready, save_config, shows,
};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use salvajocs_core::config::AppConfig;
use salvajocs_core::crossbuild::{self, Choice};
use salvajocs_core::layout::Base;
use salvajocs_core::manifest::{
    self, CustomGame, FileRule, GameEntry, Integration, Manifest, Os, Pair, RegistryRule, Steam,
    Tag, When,
};
use salvajocs_gui::tabs::Tab;
use salvajocs_gui::{App, Env};
use tempfile::TempDir;

/// An app with the repository open and the Custom games tab in front, starting from `games`
/// and a cached manifest that holds the titles `manifest_titles`.
fn open_tab(
    games: Vec<CustomGame>,
    manifest_titles: &[&str],
) -> (Harness<'static, App>, Env, TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
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

fn save_rule() -> FileRule {
    FileRule {
        tags: vec![Tag::Save],
        when: Vec::new(),
    }
}

fn pair(linux: &str, windows: &str) -> Pair {
    Pair {
        linux: linux.into(),
        windows: windows.into(),
    }
}

#[test]
fn custom_games_add_game_with_pair_saves_the_entry_to_the_config() {
    let (mut harness, env, _tmp) = open_tab(Vec::new(), &[]);

    harness.get_by_label("Add game").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Name", "My Game");
    replace_text(&mut harness, "Files", "<winDocuments>/My Game/*.sav");
    replace_text(
        &mut harness,
        "Registry keys",
        "HKEY_CURRENT_USER/Software/My Game",
    );
    replace_text(&mut harness, "Wine prefix", "/prefixes/mygame");
    harness.get_by_label("Add pair").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Linux 1", "<xdgData>/mygame");
    replace_text(&mut harness, "Windows 1", "<winDocuments>/My Game");
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);

    let saved = run_until_saved(&mut harness, &env, |games| games.len() == 1);
    assert_eq!(
        saved,
        [CustomGame {
            name: "My Game".into(),
            entry: GameEntry {
                files: BTreeMap::from([("<winDocuments>/My Game/*.sav".to_owned(), save_rule())]),
                registry: BTreeMap::from([(
                    "HKEY_CURRENT_USER/Software/My Game".to_owned(),
                    RegistryRule::default(),
                )]),
                ..GameEntry::default()
            },
            integration: Integration::Extend,
            wine_prefix: Some("/prefixes/mygame".into()),
            pairs: vec![pair("<xdgData>/mygame", "<winDocuments>/My Game")],
            skips: Vec::new(),
        }]
    );
    assert!(shows(&harness, "My Game (custom game)"));
    assert!(
        !shows(&harness, "Save game"),
        "the form closes after saving"
    );
}

#[test]
fn custom_games_pair_that_does_not_classify_shows_error_and_is_not_saved() {
    let (mut harness, env, _tmp) = open_tab(Vec::new(), &[]);

    harness.get_by_label("Add game").click();
    run_settled(&mut harness);
    replace_text(&mut harness, "Name", "My Game");
    harness.get_by_label("Add pair").click();
    run_settled(&mut harness);
    // A Windows template on the Linux side.
    replace_text(&mut harness, "Linux 1", "<winAppData>/mygame");
    replace_text(&mut harness, "Windows 1", "<winAppData>/mygame");
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);

    assert!(shows(&harness, "not a placeholder template"));
    assert!(shows(&harness, "Save game"), "the form stays open");
    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert!(saved.custom_games.is_empty());
}

#[test]
fn custom_games_empty_and_duplicate_names_are_rejected() {
    let existing = CustomGame {
        name: "Taken".into(),
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
    harness.get_by_label("Remove pair").click();
    run_settled(&mut harness);
    harness.get_by_label("Save game").click();
    run_settled(&mut harness);

    let saved = run_until_saved(&mut harness, &env, |games| {
        games.first().is_some_and(|game| game.name == "Renamed")
    });
    let renamed = CustomGame {
        name: "Renamed".into(),
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
    harness.get_by_label("Remove pair").click();
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
