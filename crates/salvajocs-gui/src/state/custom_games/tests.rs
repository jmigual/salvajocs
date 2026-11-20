use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use salvajocs_core::config::PasswordMode;
use salvajocs_core::layout::Base;
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::manifest::{GameEntry, Integration};
use salvajocs_core::storage::{Compression, RepoLocation};

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
    apply(
        &mut games,
        Update::Put {
            at: "A".into(),
            game: Box::new(game("Renamed", Some("/form"))),
            keep_prefix: false,
        },
    );
    // The repository's copy keeps this PC's prefix.
    apply(
        &mut games,
        Update::Put {
            at: "Renamed".into(),
            game: Box::new(game("Renamed", None)),
            keep_prefix: true,
        },
    );
    apply(
        &mut games,
        Update::Put {
            at: "C".into(),
            game: Box::new(game("C", None)),
            keep_prefix: true,
        },
    );
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
    state.config = Some(AppConfig::new(repository, PasswordMode::Ask));
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

#[test]
fn only_saves_and_syncs_that_upload_count_as_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    let storage = state.session.as_ref().unwrap().storage.clone();
    let config = state.config.as_mut().unwrap();
    config.custom_games_repository = Some(config.secret_scope());

    // Holding the repository keeps each job waiting for it.
    let parked = storage.lock().unwrap();
    state.sync_custom_games(&ctx);
    assert!(state.busy());
    assert!(!state.writing(), "a sync of a mirror only reads");
    let edit = Edit {
        based_on: None,
        game: Some(game("New", None)),
    };
    state.edit_custom_game(&ctx, edit);
    assert!(state.writing(), "a save waits");
    drop(parked);
    poll_until_idle(&mut state, &ctx);

    state.config.as_mut().unwrap().custom_games_repository = None;
    let parked = storage.lock().unwrap();
    state.sync_custom_games(&ctx);
    assert!(state.writing(), "a sync that migrates uploads");
    drop(parked);
    poll_until_idle(&mut state, &ctx);
}

/// The notices left once the job that `queue` starts ends, another repository connected meanwhile.
fn connect_another_meanwhile(queue: impl FnOnce(&mut State, &egui::Context)) -> Vec<String> {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    let storage = state.session.as_ref().unwrap().storage.clone();
    let parked = storage.lock().unwrap();
    queue(&mut state, &ctx);
    assert!(state.custom_games.saving());
    state.config.as_mut().unwrap().repository = tmp.path().join("other").to_str().unwrap().into();
    drop(parked);
    poll_until_idle(&mut state, &ctx);
    state.notices
}

#[test]
fn a_remove_or_choices_for_a_repository_no_longer_used_say_they_were_not_applied() {
    let notices = connect_another_meanwhile(|state, ctx| {
        let edit = Edit {
            based_on: Some(game("Gone", None)),
            game: None,
        };
        state.edit_custom_game(ctx, edit);
    });
    assert!(
        matches!(notices.as_slice(), [notice] if notice.starts_with("Gone was not removed here")),
        "{notices:?}"
    );

    let notices = connect_another_meanwhile(|state, ctx| {
        let choice = (
            (Base::XdgData, "Foo".to_owned()),
            Choice::Template("<winAppData>/Foo".to_owned()),
        );
        state.save_mapping_choices(ctx, "Mapped".into(), vec![choice]);
    });
    let said = |notice: &String| notice.starts_with("The folder choices for Mapped");
    assert!(
        matches!(notices.as_slice(), [notice] if said(notice)),
        "{notices:?}"
    );
}
