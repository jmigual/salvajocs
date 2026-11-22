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

    assert_eq!(config(&state).custom_games, [game("Mine", None)]);
    assert_eq!(
        state.custom_games.offered("Theirs"),
        Some(&game("Theirs", None))
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

    assert!(state.custom_games.offered("Sent").is_some());
}

/// `state` after a read of its repository, where another PC saved `games`.
fn read_elsewhere(state: &mut State, dir: &Path, ctx: &egui::Context, games: &[CustomGame]) {
    for game in games {
        save_elsewhere(state, dir, game);
    }
    state.read_custom_games(ctx);
    poll_until_idle(state, ctx);
}

#[test]
fn a_read_does_not_use_another_pcs_game_until_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    let theirs = game("Theirs", None);
    read_elsewhere(&mut state, tmp.path(), &ctx, std::slice::from_ref(&theirs));
    assert!(config(&state).custom_games.is_empty());
    assert_eq!(state.custom_games.offered("Theirs"), Some(&theirs));

    state.accept_custom_game(&ctx, &theirs);
    assert_eq!(config(&state).custom_games, std::slice::from_ref(&theirs));
    assert!(config(&state).custom_games_unsent.is_empty());
    assert!(state.custom_games.offered("Theirs").is_none());
    poll_until_idle(&mut state, &ctx);
    let saved = AppConfig::load(&tmp.path().join("config"))
        .unwrap()
        .unwrap();
    assert_eq!(saved.custom_games, [theirs]);
}

#[test]
fn accepting_keeps_this_pcs_wine_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    let mine = game("Game", Some("/prefixes/game"));
    state.config.as_mut().unwrap().custom_games = vec![mine];
    let mut theirs = game("Game", None);
    theirs.integration = Integration::Override;
    read_elsewhere(&mut state, tmp.path(), &ctx, std::slice::from_ref(&theirs));
    assert_eq!(
        config(&state).custom_games,
        [game("Game", Some("/prefixes/game"))]
    );

    state.accept_custom_game(&ctx, &theirs);
    let mut expected = theirs;
    expected.wine_prefix = Some("/prefixes/game".into());
    assert_eq!(config(&state).custom_games, [expected]);
    assert!(config(&state).custom_games_unsent.is_empty());
    poll_until_idle(&mut state, &ctx);
}

/// The copies `state` set aside with **Not now**.
fn not_now(state: &State) -> Vec<CustomGame> {
    let not_now = config(state).custom_games_not_now.as_ref();
    not_now.map_or_else(Vec::new, |not_now| not_now.games.clone())
}

#[test]
fn a_copy_set_aside_stays_offered_without_a_notice_until_another_copy_comes() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    let mine = game("Game", None);
    state.config.as_mut().unwrap().custom_games = vec![mine.clone()];
    let mut theirs = mine.clone();
    theirs.integration = Integration::Override;
    read_elsewhere(&mut state, tmp.path(), &ctx, std::slice::from_ref(&theirs));
    assert_eq!(state.notices.len(), 1);

    state.set_custom_game_aside(&ctx, &theirs);
    assert_eq!(state.custom_games.offered("Game"), Some(&theirs));
    assert_eq!(not_now(&state), [theirs.clone()]);
    assert_eq!(state.custom_games.waiting(config(&state)), 0);
    assert_eq!(config(&state).custom_games, [mine]);
    assert!(
        config(&state).custom_games_unsent.is_empty(),
        "nothing to send"
    );

    // The first read after a restart is news for every offer but the one set aside.
    state.custom_games = CustomGameJobs::default();
    read_elsewhere(&mut state, tmp.path(), &ctx, &[game("Other", None)]);
    assert_eq!(state.notices.len(), 2);
    assert!(state.notices[1].ends_with(": Other"), "{:?}", state.notices);
    assert_eq!(state.custom_games.offered("Game"), Some(&theirs));
    assert_eq!(not_now(&state), [theirs.clone()]);
    assert_eq!(state.custom_games.waiting(config(&state)), 1);

    // Another copy of it is offered again, with a notice, and is no longer set aside.
    let mut newer = theirs.clone();
    newer.skips = vec!["<xdgData>/Newer".into()];
    resave_elsewhere(&state, tmp.path(), &theirs, &newer);
    state.read_custom_games(&ctx);
    poll_until_idle(&mut state, &ctx);
    assert_eq!(state.notices.len(), 3);
    assert!(state.notices[2].ends_with(": Game"), "{:?}", state.notices);
    assert_eq!(state.custom_games.offered("Game"), Some(&newer));
    assert_eq!(config(&state).custom_games_not_now, None);
    assert_eq!(state.custom_games.waiting(config(&state)), 2);
    let saved = AppConfig::load(&tmp.path().join("config"))
        .unwrap()
        .unwrap();
    assert_eq!(saved.custom_games_not_now, None);
}

#[test]
fn accepting_a_copy_set_aside_uses_it_and_drops_it_from_the_set_aside() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    let theirs = game("Theirs", None);
    let kept = game("Kept", None);
    read_elsewhere(
        &mut state,
        tmp.path(),
        &ctx,
        &[theirs.clone(), kept.clone()],
    );
    state.set_custom_game_aside(&ctx, &theirs);
    state.set_custom_game_aside(&ctx, &kept);

    state.accept_custom_game(&ctx, &theirs);
    assert_eq!(config(&state).custom_games, std::slice::from_ref(&theirs));
    assert_eq!(not_now(&state), std::slice::from_ref(&kept));
    state.accept_custom_game(&ctx, &kept);
    assert_eq!(config(&state).custom_games_not_now, None);
    poll_until_idle(&mut state, &ctx);
}

#[test]
fn a_game_the_repository_removed_drops_its_copy_set_aside() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    let theirs = game("Theirs", None);
    read_elsewhere(&mut state, tmp.path(), &ctx, std::slice::from_ref(&theirs));
    state.set_custom_game_aside(&ctx, &theirs);
    assert_eq!(not_now(&state), std::slice::from_ref(&theirs));

    let storage = state.session.as_ref().unwrap().storage.clone();
    let stage = tmp.path().join("elsewhere");
    let mut storage = storage.lock().unwrap();
    custom_games::save(&mut storage, "Theirs", Some(&theirs), None, &stage).unwrap();
    drop(storage);
    state.read_custom_games(&ctx);
    poll_until_idle(&mut state, &ctx);
    assert!(state.custom_games.all_offered().is_empty());
    assert_eq!(config(&state).custom_games_not_now, None);
}

#[test]
fn a_read_with_a_new_offer_adds_one_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    let games = [game("A", None), game("B", None)];
    read_elsewhere(&mut state, tmp.path(), &ctx, &games);
    assert_eq!(state.notices.len(), 1);
    assert!(state.notices[0].ends_with("A, B"), "{:?}", state.notices);

    // The same offers read again are not news.
    state.read_custom_games(&ctx);
    poll_until_idle(&mut state, &ctx);
    assert_eq!(state.notices.len(), 1);
    read_elsewhere(&mut state, tmp.path(), &ctx, &[game("C", None)]);
    assert_eq!(state.notices.len(), 2);
    assert!(state.notices[1].ends_with(": C"), "{:?}", state.notices);
    // An offer changed again is news too.
    let mut changed = game("A", None);
    changed.integration = Integration::Override;
    resave_elsewhere(&state, tmp.path(), &game("A", None), &changed);
    state.read_custom_games(&ctx);
    poll_until_idle(&mut state, &ctx);
    assert_eq!(state.notices.len(), 3);
    assert!(state.notices[2].ends_with(": A"), "{:?}", state.notices);
}

/// Saves `game` over `based_on` in the repository of `state` as another PC would.
fn resave_elsewhere(state: &State, dir: &Path, based_on: &CustomGame, game: &CustomGame) {
    let storage = state.session.as_ref().unwrap().storage.clone();
    let mut storage = storage.lock().unwrap();
    let stage = dir.join("elsewhere");
    let name = &game.name;
    custom_games::save(&mut storage, name, Some(based_on), Some(game), &stage).unwrap();
}

#[test]
fn an_edited_offer_is_not_offered_again_after_back_up() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    let mine = game("Game", None);
    state.config.as_mut().unwrap().custom_games = vec![mine.clone()];
    let mut theirs = mine.clone();
    theirs.integration = Integration::Override;
    read_elsewhere(&mut state, tmp.path(), &ctx, std::slice::from_ref(&theirs));
    assert!(state.custom_games.offered("Game").is_some());

    let mut edited = mine.clone();
    edited.skips = vec!["<xdgData>/Mine".into()];
    let edit = Edit {
        based_on: Some(mine),
        game: Some(edited.clone()),
    };
    state.edit_custom_game(&ctx, edit);
    assert!(state.custom_games.offered("Game").is_none());
    // Back up sent the edit, which clears its unsent entry.
    state.custom_games_sent(&ctx, &[("Game".into(), Some(edited))], None);
    assert!(config(&state).custom_games_unsent.is_empty());
    assert!(state.custom_games.offered("Game").is_none());
    poll_until_idle(&mut state, &ctx);
}

#[test]
fn an_answer_to_an_offer_a_read_replaced_does_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    let drawn = game("Theirs", None);
    read_elsewhere(&mut state, tmp.path(), &ctx, std::slice::from_ref(&drawn));
    let mut newer = drawn.clone();
    newer.integration = Integration::Override;
    resave_elsewhere(&state, tmp.path(), &drawn, &newer);
    state.read_custom_games(&ctx);
    poll_until_idle(&mut state, &ctx);

    state.accept_custom_game(&ctx, &drawn);
    state.set_custom_game_aside(&ctx, &drawn);
    assert!(config(&state).custom_games.is_empty());
    assert!(config(&state).custom_games_unsent.is_empty());
    assert_eq!(config(&state).custom_games_not_now, None);
    assert_eq!(state.custom_games.offered("Theirs"), Some(&newer));
    poll_until_idle(&mut state, &ctx);
}

#[test]
fn adopting_another_repository_forgets_the_last_reads_offers() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    read_elsewhere(&mut state, tmp.path(), &ctx, &[game("Theirs", None)]);
    assert!(state.custom_games.offered("Theirs").is_some());

    let repository = tmp.path().join("other").to_str().unwrap().to_owned();
    let location = RepoLocation::new(repository.clone(), Default::default());
    let storage = Storage::init(&location, "", Compression::Default).unwrap();
    let connected = crate::first_run::Connected {
        config: AppConfig::new(repository, PasswordMode::Ask),
        credentials: Default::default(),
        storage: Box::new(storage),
        warnings: Vec::new(),
    };
    state.adopt(connected, &ctx);
    assert!(state.custom_games.all_offered().is_empty());
    poll_until_idle(&mut state, &ctx);
    assert!(state.custom_games.all_offered().is_empty());
}

/// Makes the repository `repository`, opened as `storage`, the one `state` uses, and reads it.
fn switch_to(state: &mut State, ctx: &egui::Context, repository: String, storage: Storage) {
    let connected = crate::first_run::Connected {
        config: AppConfig::new(repository, PasswordMode::Ask),
        credentials: Default::default(),
        storage: Box::new(storage),
        warnings: Vec::new(),
    };
    state.adopt(connected, ctx);
    poll_until_idle(state, ctx);
}

#[test]
fn a_copy_set_aside_in_one_repository_does_not_hide_another_ones_offer() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    let theirs = game("Theirs", None);
    read_elsewhere(&mut state, tmp.path(), &ctx, std::slice::from_ref(&theirs));
    state.set_custom_game_aside(&ctx, &theirs);
    let set_aside = config(&state).custom_games_not_now.clone();
    let first = config(&state).repository.clone();
    let notices = state.notices.len();

    let other = tmp.path().join("other").to_str().unwrap().to_owned();
    let location = RepoLocation::new(other.clone(), Default::default());
    let mut storage = Storage::init(&location, "", Compression::Default).unwrap();
    let stage = tmp.path().join("elsewhere");
    custom_games::save(&mut storage, "Theirs", None, Some(&theirs), &stage).unwrap();
    switch_to(&mut state, &ctx, other, storage);
    assert_eq!(state.custom_games.offered("Theirs"), Some(&theirs));
    assert_eq!(state.custom_games.waiting(config(&state)), 1);
    assert_eq!(state.notices.len(), notices + 1);
    assert_eq!(config(&state).custom_games_not_now, set_aside);

    // Back on the first repository, the copy is still set aside.
    let location = RepoLocation::new(first.clone(), Default::default());
    let storage = Storage::open(&location, "").unwrap();
    switch_to(&mut state, &ctx, first, storage);
    assert_eq!(state.custom_games.offered("Theirs"), Some(&theirs));
    assert_eq!(state.custom_games.waiting(config(&state)), 0);
    assert_eq!(state.notices.len(), notices + 1);
    assert_eq!(config(&state).custom_games_not_now, set_aside);
}

#[test]
fn a_read_that_fails_for_a_copy_set_aside_keeps_it_set_aside() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut state = state(tmp.path());
    let theirs = game("Theirs", None);
    read_elsewhere(&mut state, tmp.path(), &ctx, std::slice::from_ref(&theirs));
    state.set_custom_game_aside(&ctx, &theirs);
    let set_aside = config(&state).custom_games_not_now.clone();
    let notices = state.notices.len();

    let failed = CustomGameError::Io {
        path: tmp.path().join("custom-game.json"),
        source: std::io::ErrorKind::Interrupted.into(),
    };
    let listed = Listed::from([("Theirs".to_owned(), Err(failed))]);
    let scope = config(&state).secret_scope();
    state.merge_custom_games(&ctx, &listed, scope);
    assert_eq!(config(&state).custom_games_not_now, set_aside);

    state.read_custom_games(&ctx);
    poll_until_idle(&mut state, &ctx);
    assert_eq!(state.custom_games.offered("Theirs"), Some(&theirs));
    assert_eq!(state.custom_games.waiting(config(&state)), 0);
    assert_eq!(state.notices.len(), notices);
}
