use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use salvajocs_core::config::PasswordMode;
use salvajocs_core::layout::Base;
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::manifest::{GameEntry, Integration};
use salvajocs_core::storage::{Compression, RepoLocation, Storage};

use super::*;
use crate::env::{Env, MemorySecrets};
use crate::state::Session;

fn game(name: &str, prefix: Option<&str>) -> CustomGame {
    let mut game = CustomGame::new(name.into(), GameEntry::default(), Integration::Extend);
    game.wine_prefix = prefix.map(Into::into);
    game
}

#[test]
fn a_put_replaces_the_named_game_in_place_or_appends() {
    let mut games = vec![game("A", Some("/a")), game("B", None)];
    // A rename takes the old game's place.
    let put = |at: &str, game: CustomGame| Update::Put {
        at: at.into(),
        game: Box::new(game),
    };
    apply(&mut games, put("A", game("Renamed", Some("/form"))));
    apply(&mut games, put("C", game("C", None)));
    assert_eq!(
        games,
        [
            game("Renamed", Some("/form")),
            game("B", None),
            game("C", None)
        ]
    );
    apply(&mut games, Update::Drop("B".into()));
    assert_eq!(games, [game("Renamed", Some("/form")), game("C", None)]);
}

/// A state with a new repository in `dir` open.
fn state(dir: &Path) -> State {
    fn offline(_: &Path) -> Result<Freshness, FetchError> {
        Ok(Freshness::NotModified)
    }
    let mut state = State::new(Env {
        config_dir: dir.join("config"),
        cache_dir: dir.join("cache"),
        secrets: Arc::new(MemorySecrets::new()),
        refresh_manifest: offline,
        environment: None,
        wineserver_running: salvajocs_core::prefix::wineserver_running,
        check_update: None,
        installed_by_installer: || false,
        run_installer: |_| panic!("tests never start an installer"),
        appimage: None,
        restart_appimage: |_| panic!("tests never restart the app"),
        rclone_remotes: || Err(std::io::ErrorKind::NotFound.into()),
        pick_folder: Arc::new(|| None),
        open_folder: Arc::new(|_| Ok(())),
    });
    let repository = dir.join("repo").to_str().unwrap().to_owned();
    let location = RepoLocation::new(repository.clone(), Default::default());
    let storage = Storage::init(&location, "", Compression::Default).unwrap();
    state.session = Some(Session {
        compression: storage.compression(),
        storage: Arc::new(Mutex::new(storage)),
        space: Default::default(),
    });
    let mut config = AppConfig::new(repository, PasswordMode::Ask);
    config.custom_games_repository = Some(config.secret_scope());
    state.config = Some(config);
    state
}

fn poll_until_idle(state: &mut State, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while state.busy() {
        assert!(Instant::now() < deadline, "the jobs never ended");
        state.poll(ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn config(state: &State) -> &AppConfig {
    state.config.as_ref().unwrap()
}

/// Saves `game` to the repository of `state` as another PC would.
fn save_elsewhere(state: &State, dir: &Path, game: &CustomGame) {
    let storage = state.session.as_ref().unwrap().storage.clone();
    let mut storage = storage.lock().unwrap();
    let stage = dir.join("elsewhere");
    custom_games::save(&mut storage, &game.name, None, Some(game), &stage).unwrap();
}

#[test]
fn edits_mark_the_names_they_change_with_the_earliest_base() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    let old = game("Old", Some("/old"));
    state.config.as_mut().unwrap().custom_games = vec![old.clone()];

    // The Wine prefix alone is never stored, so it leaves nothing unsent.
    let moved = game("Old", Some("/moved"));
    let edit = |based_on: &CustomGame, game: Option<CustomGame>| Edit {
        based_on: Some(based_on.clone()),
        game,
    };
    state.edit_custom_game(&ctx, edit(&old, Some(moved.clone())));
    assert_eq!(config(&state).custom_games, std::slice::from_ref(&moved));
    assert!(config(&state).custom_games_unsent.is_empty());

    let mut renamed = moved.clone();
    renamed.name = "New".into();
    state.edit_custom_game(&ctx, edit(&moved, Some(renamed.clone())));
    let mut edited = renamed.clone();
    edited.integration = Integration::Override;
    state.edit_custom_game(&ctx, edit(&renamed, Some(edited.clone())));
    assert_eq!(config(&state).custom_games, [edited.clone()]);
    assert_eq!(
        config(&state).custom_games_unsent,
        [
            Unsent::new("Old".into(), Some(moved)),
            Unsent::new("New".into(), None)
        ]
    );

    state.edit_custom_game(&ctx, edit(&edited, None));
    assert!(config(&state).custom_games.is_empty());
    assert_eq!(config(&state).custom_games_unsent.len(), 2);
    poll_until_idle(&mut state, &ctx);
    let saved = AppConfig::load(&tmp.path().join("config"))
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.custom_games_unsent,
        config(&state).custom_games_unsent
    );
}

#[test]
fn mapping_choices_mark_the_game_only_when_they_change_it() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    let choice = (
        (Base::XdgData, "Foo".to_owned()),
        Choice::Template("<winAppData>/Foo".to_owned()),
    );
    state.apply_mapping_choices(&ctx, "Mapped", std::slice::from_ref(&choice));
    assert_eq!(config(&state).custom_games.len(), 1);
    let marked = [Unsent::new("Mapped".into(), None)];
    assert_eq!(config(&state).custom_games_unsent, marked);

    state.config.as_mut().unwrap().custom_games_unsent.clear();
    state.apply_mapping_choices(&ctx, "Mapped", &[choice]);
    assert!(
        config(&state).custom_games_unsent.is_empty(),
        "the same choice"
    );
    poll_until_idle(&mut state, &ctx);
}

#[test]
fn an_edit_made_while_a_read_runs_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    save_elsewhere(&state, tmp.path(), &game("Theirs", None));
    let storage = state.session.as_ref().unwrap().storage.clone();

    let parked = storage.lock().unwrap();
    state.read_custom_games(&ctx);
    let added = Edit {
        based_on: None,
        game: Some(game("Mine", None)),
    };
    state.edit_custom_game(&ctx, added);
    drop(parked);
    poll_until_idle(&mut state, &ctx);

    assert_eq!(
        config(&state).custom_games,
        [game("Mine", None), game("Theirs", None)]
    );
}

#[test]
fn a_read_that_started_before_a_back_up_ended_is_read_again() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());

    state.read_custom_games(&ctx);
    let deadline = Instant::now() + Duration::from_secs(60);
    while (state.custom_games.reading.as_mut()).is_some_and(|reading| reading.job.is_running()) {
        assert!(Instant::now() < deadline, "the read never ended");
        std::thread::sleep(Duration::from_millis(5));
    }
    // A Back up saves a game after the read listed the repository, and ends before it is taken.
    save_elsewhere(&state, tmp.path(), &game("Sent", None));
    state.custom_games_sent(&ctx, &[], None);
    poll_until_idle(&mut state, &ctx);

    assert_eq!(config(&state).custom_games, [game("Sent", None)]);
}
