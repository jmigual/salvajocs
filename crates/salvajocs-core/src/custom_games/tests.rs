use std::num::NonZeroU32;
use std::time::SystemTime;

use super::*;
use crate::layout::Base;
use crate::manifest::{InstallDir, RegistryRule};
use crate::retention::{self, DeleteError, Retention};
use crate::storage::{Compression, RepoLocation};

const GAME: &str = "Game, with comma: ünï";

struct Fixture {
    _tmp: tempfile::TempDir,
    location: RepoLocation,
    stage: PathBuf,
}

impl Fixture {
    fn new() -> (Self, Storage) {
        let tmp = tempfile::tempdir().unwrap();
        let location = RepoLocation {
            repository: tmp.path().join("repo").to_str().unwrap().to_string(),
            options: BTreeMap::new(),
        };
        let storage = Storage::init(&location, "", Compression::Default).unwrap();
        let stage = tmp.path().join("stage");
        (
            Self {
                _tmp: tmp,
                location,
                stage,
            },
            storage,
        )
    }

    /// A second session on the same repository, as another PC would have.
    fn open(&self) -> Storage {
        Storage::open(&self.location, "").unwrap()
    }

    /// Saves `new` as `name` over `based_on`, expecting no forget error.
    fn save(
        &self,
        storage: &mut Storage,
        name: &str,
        based_on: Option<&CustomGame>,
        new: Option<&CustomGame>,
    ) -> Option<SnapshotInfo> {
        let saved = save(storage, name, based_on, new, &self.stage).unwrap();
        assert!(saved.forget_error.is_none(), "{:?}", saved.forget_error);
        saved.written
    }

    /// Writes `json` as the newest custom game snapshot of `name`, as another version would.
    fn write_raw(&self, storage: &mut Storage, name: &str, json: &str) -> SnapshotInfo {
        let slug = slug(name);
        let dir = self.stage.join("raw");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(GAME_FILE), json).unwrap();
        storage
            .backup_dir(
                &dir,
                &as_path(&slug),
                &[SETTINGS_TAG, &game_tag(&slug)],
                name,
            )
            .unwrap()
            .unwrap()
    }
}

/// A game with every shared field set.
fn game(name: &str) -> CustomGame {
    let entry = GameEntry {
        registry: BTreeMap::from([(
            "HKEY_CURRENT_USER/Software/Game".to_owned(),
            RegistryRule::default(),
        )]),
        install_dir: BTreeMap::from([("Game".to_owned(), InstallDir::default())]),
        ..GameEntry::default()
    };
    let mut game = CustomGame::new(name.into(), entry, Integration::Extend);
    game.files = vec![row("<xdgData>/game", "<winAppData>/Game")];
    game.pairs = vec![Pair {
        linux: "<xdgConfig>/game".into(),
        windows: "<winLocalAppData>/Game".into(),
    }];
    game.skips = vec!["<home>/Game/cache".into()];
    game
}

fn row(linux: &str, windows: &str) -> CustomFile {
    CustomFile {
        linux: Some(linux.into()),
        windows: Some(windows.into()),
    }
}

fn with_prefix(mut game: CustomGame, prefix: &str) -> CustomGame {
    game.wine_prefix = Some(prefix.into());
    game
}

fn loaded(storage: &mut Storage) -> BTreeMap<String, Stored> {
    load_all(storage)
        .unwrap()
        .into_iter()
        .map(|(name, stored)| (name, stored.unwrap()))
        .collect()
}

fn raw(storage: &mut Storage, snapshot: &SnapshotInfo, name: &str) -> Value {
    let path = format!("{}/{GAME_FILE}", as_path(&slug(name)));
    serde_json::from_slice(&storage.read_file(&snapshot.id, &path).unwrap()).unwrap()
}

fn ids(storage: &Storage) -> Vec<String> {
    let snapshots = storage.snapshots(SETTINGS_TAG).unwrap();
    snapshots.into_iter().map(|snapshot| snapshot.id).collect()
}

#[test]
fn save_then_load_from_another_session_round_trips() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, GAME, None, Some(&game(GAME))).unwrap();
    assert_eq!(
        loaded(&mut f.open()),
        BTreeMap::from([(GAME.to_owned(), Stored::Game(Box::new(game(GAME))))])
    );
}

#[test]
fn the_stored_file_has_no_wine_prefix() {
    let (f, mut storage) = Fixture::new();
    let local = with_prefix(game(GAME), "/prefixes/game");
    let written = f.save(&mut storage, GAME, None, Some(&local)).unwrap();
    let json = raw(&mut storage, &written, GAME);
    assert!(json.get("wine_prefix").is_none(), "{json}");
    assert_eq!(json["format"], 1);
    assert_eq!(json["name"], GAME);
    assert_eq!(json["integration"], "extend");
    assert_eq!(
        loaded(&mut storage)[GAME],
        Stored::Game(Box::new(game(GAME)))
    );
}

#[test]
fn custom_game_snapshots_are_neither_game_backups_nor_restorable_nor_forgettable() {
    let (f, mut storage) = Fixture::new();
    let written = f.save(&mut storage, GAME, None, Some(&game(GAME))).unwrap();
    let mut tags = written.tags.clone();
    tags.sort();
    assert_eq!(
        tags,
        [format!("custom-game:{}", slug(GAME)), SETTINGS_TAG.into()]
    );
    assert!(storage.snapshots("salvajocs").unwrap().is_empty());
    assert!(crate::restore::list(&storage).unwrap().is_empty());

    let policy = Retention {
        keep_last: NonZeroU32::new(1),
        ..Retention::default()
    };
    let settings = storage.snapshots(SETTINGS_TAG).unwrap();
    assert!(retention::to_forget(&policy, &settings, &[], SystemTime::now()).is_empty());
    retention::apply(&mut storage, &policy, &[], SystemTime::now()).unwrap();
    assert!(matches!(
        retention::delete(&mut storage, &[&written.id]),
        Err(DeleteError::NotAGameBackup(_))
    ));
    assert_eq!(ids(&storage), [written.id]);
}

#[test]
fn custom_games_stage_apart_from_game_backups_and_selections() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, GAME, None, Some(&game(GAME))).unwrap();
    let slug = slug(GAME);
    let staged = f.stage.join("settings").join(STAGE_DIR).join(&slug);
    assert!(staged.join(GAME_FILE).is_file());
    assert!(!f.stage.join(&slug).exists());
    assert!(!f.stage.join("settings").join(&slug).exists());
}

#[test]
fn a_remove_writes_a_tombstone_and_adding_the_name_again_replaces_it() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, GAME, None, Some(&game(GAME))).unwrap();
    let tombstone = f.save(&mut storage, GAME, Some(&game(GAME)), None).unwrap();
    assert_eq!(
        raw(&mut storage, &tombstone, GAME),
        serde_json::json!({ "format": 1, "name": GAME, "removed": true })
    );
    assert_eq!(ids(&storage), std::slice::from_ref(&tombstone.id));
    assert_eq!(loaded(&mut f.open())[GAME], Stored::Removed);

    // Removing it again changes nothing.
    assert!(f.save(&mut storage, GAME, None, None).is_none());

    let again = f.save(&mut storage, GAME, None, Some(&game(GAME))).unwrap();
    assert_eq!(ids(&storage), [again.id]);
    assert_eq!(
        loaded(&mut storage)[GAME],
        Stored::Game(Box::new(game(GAME)))
    );
}

#[test]
fn removing_a_game_the_repository_never_had_writes_nothing() {
    let (f, mut storage) = Fixture::new();
    assert!(f.save(&mut storage, GAME, None, None).is_none());
    assert!(ids(&storage).is_empty());
}

#[test]
fn an_edit_of_a_game_the_repository_lacks_uploads_it_and_a_remove_writes_nothing() {
    // A local game whose upload failed: the repository has no snapshot of it at all.
    let (f, mut storage) = Fixture::new();
    let mut edited = game(GAME);
    edited.skips.push("<home>/Game/logs".into());
    let saved = save(
        &mut storage,
        GAME,
        Some(&game(GAME)),
        Some(&edited),
        &f.stage,
    )
    .unwrap();
    assert!(saved.written.is_some());
    assert_eq!(loaded(&mut f.open())[GAME], Stored::Game(Box::new(edited)));

    let saved = save(&mut storage, "Other", Some(&game("Other")), None, &f.stage).unwrap();
    assert!(saved.written.is_none());
    assert!(!loaded(&mut storage).contains_key("Other"));
}

#[test]
fn a_failed_forget_still_writes_and_keeps_the_newest_tombstone() {
    let (f, mut storage) = Fixture::new();
    let first = f.save(&mut storage, GAME, None, Some(&game(GAME))).unwrap();
    storage.set_append_only();
    let saved = save(&mut storage, GAME, Some(&game(GAME)), None, &f.stage).unwrap();
    let tombstone = saved.written.unwrap();
    assert!(saved.forget_error.is_some());
    assert_eq!(ids(&storage), [tombstone.id, first.id]);
    assert_eq!(loaded(&mut f.open())[GAME], Stored::Removed);
}

#[test]
fn an_unchanged_save_writes_nothing_and_a_wine_prefix_alone_is_unchanged() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, GAME, None, Some(&game(GAME))).unwrap();
    let before = ids(&storage);
    let moved = with_prefix(game(GAME), "/prefixes/elsewhere");
    let saved = save(
        &mut storage,
        GAME,
        Some(&game(GAME)),
        Some(&moved),
        &f.stage,
    )
    .unwrap();
    assert!(saved.written.is_none());
    assert_eq!(
        saved.game,
        Some(game(GAME)),
        "the stored game has no prefix"
    );
    assert_eq!(ids(&storage), before);
}

#[test]
fn a_save_forgets_only_what_it_read() {
    let (f, mut storage) = Fixture::new();
    let first = f.save(&mut storage, GAME, None, Some(&game(GAME))).unwrap();
    let read = read_newest(&mut storage, GAME).unwrap();

    // Another PC saves between this save's read and its write.
    let mut theirs = game(GAME);
    theirs.skips.push("<home>/Theirs".into());
    let theirs = f
        .save(&mut f.open(), GAME, Some(&game(GAME)), Some(&theirs))
        .unwrap();

    let mut ours = game(GAME);
    ours.skips.push("<home>/Ours".into());
    let saved = write(&mut storage, GAME, Some(&ours), &read, &f.stage).unwrap();
    assert!(saved.forget_error.is_none());
    let ours = saved.written.unwrap();
    let left = ids(&storage);
    assert!(left.contains(&theirs.id));
    assert!(left.contains(&ours.id));
    assert!(!left.contains(&first.id));
}

#[test]
fn a_stale_edit_is_changed_with_the_newest_copy_and_writes_nothing() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, GAME, None, Some(&game(GAME))).unwrap();
    let mut theirs = game(GAME);
    theirs
        .files
        .push(row("<xdgData>/theirs", "<winAppData>/Theirs"));
    f.save(&mut f.open(), GAME, Some(&game(GAME)), Some(&theirs));
    let before = ids(&storage);

    let mut ours = game(GAME);
    ours.skips.clear();
    let result = save(&mut storage, GAME, Some(&game(GAME)), Some(&ours), &f.stage);
    match result {
        Err(CustomGameError::Changed(Some(newest))) => assert_eq!(*newest, theirs),
        other => panic!("expected Changed, got {other:?}"),
    }
    assert_eq!(ids(&storage), before);

    // A new game whose name another PC already stored is changed too.
    let result = save(&mut storage, GAME, None, Some(&ours), &f.stage);
    assert!(matches!(result, Err(CustomGameError::Changed(Some(_)))));
    // An edit of a game another PC removed gets no copy back.
    f.save(&mut f.open(), GAME, Some(&theirs), None).unwrap();
    let result = save(&mut storage, GAME, Some(&theirs), Some(&ours), &f.stage);
    assert!(matches!(result, Err(CustomGameError::Changed(None))));
}

#[test]
fn newer_format_is_neither_loaded_nor_saved() {
    let (f, mut storage) = Fixture::new();
    f.write_raw(&mut storage, GAME, r#"{ "format": 2, "name": "x" }"#);
    f.save(&mut storage, "Other", None, Some(&game("Other")))
        .unwrap();
    let before = ids(&storage);

    for result in [
        save(&mut storage, GAME, None, Some(&game(GAME)), &f.stage),
        save(&mut storage, GAME, None, None, &f.stage),
        save_choices(&mut storage, GAME, None, &[choice()], &f.stage),
    ] {
        assert!(matches!(result, Err(CustomGameError::NewerFormat(2))));
    }
    assert_eq!(ids(&storage), before);

    let all = load_all(&mut f.open()).unwrap();
    assert!(matches!(all[GAME], Err(CustomGameError::NewerFormat(2))));
    assert!(all["Other"].is_ok());
}

#[test]
fn a_game_without_an_integration_does_not_parse() {
    let (f, mut storage) = Fixture::new();
    f.write_raw(&mut storage, GAME, r#"{ "format": 1, "name": "x" }"#);
    let all = load_all(&mut storage).unwrap();
    assert!(matches!(all["x"], Err(CustomGameError::Parse(_))));
}

#[test]
fn an_unknown_top_level_field_survives_a_rewrite() {
    let (f, mut storage) = Fixture::new();
    let mut stored = File::game(GAME, &game(GAME), BTreeMap::new());
    stored
        .other
        .insert("future".into(), serde_json::json!({ "a": [1] }));
    f.write_raw(
        &mut storage,
        GAME,
        &String::from_utf8(stored.to_json()).unwrap(),
    );

    // The local copy cannot hold the field, and the save is still not a conflict.
    let mut edited = game(GAME);
    edited.skips.clear();
    let written = f
        .save(&mut storage, GAME, Some(&game(GAME)), Some(&edited))
        .unwrap();
    let json = raw(&mut storage, &written, GAME);
    assert_eq!(json["future"], serde_json::json!({ "a": [1] }));
    assert!(json.get("skips").is_none());
}

fn choice() -> (SnapshotRoot, Choice) {
    (
        (Base::XdgData, "mapped".into()),
        Choice::Template("<winAppData>/Mapped".into()),
    )
}

#[test]
fn choices_are_saved_on_the_newest_copy() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, GAME, None, Some(&game(GAME))).unwrap();
    // Another PC adds a row after this one last loaded.
    let mut theirs = game(GAME);
    theirs
        .files
        .push(row("<xdgData>/theirs", "<winAppData>/Theirs"));
    f.save(&mut f.open(), GAME, Some(&game(GAME)), Some(&theirs));

    let saved = save_choices(&mut storage, GAME, None, &[choice()], &f.stage).unwrap();
    let mut expected = theirs.clone();
    expected.pairs.push(Pair {
        linux: "<xdgData>/mapped".into(),
        windows: "<winAppData>/Mapped".into(),
    });
    assert_eq!(saved.game.as_ref(), Some(&expected));
    assert!(saved.written.is_some());
    assert_eq!(
        loaded(&mut f.open())[GAME],
        Stored::Game(Box::new(expected))
    );

    // The same choice again changes nothing.
    let again = save_choices(&mut storage, GAME, None, &[choice()], &f.stage).unwrap();
    assert!(again.written.is_none());
}

#[test]
fn choices_for_a_game_the_repository_lacks_make_an_extend_game() {
    let (f, mut storage) = Fixture::new();
    let saved = save_choices(&mut storage, GAME, None, &[choice()], &f.stage).unwrap();
    let game = saved.game.unwrap();
    assert_eq!(game.integration, Integration::Extend);
    assert_eq!(loaded(&mut storage)[GAME], Stored::Game(Box::new(game)));
}

#[test]
fn choices_for_a_local_game_the_repository_lacks_keep_its_definition() {
    // A local game whose upload failed: the repository has no snapshot of it at all.
    let (f, mut storage) = Fixture::new();
    let mut local = with_prefix(game(GAME), "/prefixes/game");
    local.integration = Integration::Override;
    let saved = save_choices(&mut storage, GAME, Some(&local), &[choice()], &f.stage).unwrap();
    let mut expected = vec![game(GAME)];
    expected[0].integration = Integration::Override;
    crossbuild::apply_choices(&mut expected, GAME, &[choice()]);
    assert_ne!(
        expected[0].pairs,
        game(GAME).pairs,
        "the choice adds a pair"
    );
    let game = saved.game.unwrap();
    assert_eq!(game, expected[0]);
    assert_eq!(loaded(&mut storage)[GAME], Stored::Game(Box::new(game)));
}

fn listed(
    entries: Vec<(&str, Result<Stored, CustomGameError>)>,
) -> BTreeMap<String, Result<Stored, CustomGameError>> {
    (entries.into_iter())
        .map(|(name, stored)| (name.to_owned(), stored))
        .collect()
}

fn names(games: &[CustomGame]) -> Vec<&str> {
    games.iter().map(|game| game.name.as_str()).collect()
}

#[test]
fn merge_keeps_the_local_order_and_appends_new_games_by_name() {
    let local = [game("B"), game("A")];
    let mut changed = game("A");
    changed.skips.clear();
    let listed = listed(vec![
        ("D", Ok(Stored::Game(Box::new(game("D"))))),
        ("A", Ok(Stored::Game(Box::new(changed.clone())))),
        ("C", Ok(Stored::Game(Box::new(game("C"))))),
        ("B", Ok(Stored::Game(Box::new(game("B"))))),
    ]);
    let merged = merge(&local, &listed, true);
    assert_eq!(names(&merged.games), ["B", "A", "C", "D"]);
    assert_eq!(merged.games[1], changed);
    assert!(merged.upload.is_empty());
    assert!(
        merged.replaced.is_empty(),
        "a mirror reports no replaced games"
    );
}

#[test]
fn merge_keeps_each_local_wine_prefix_by_name() {
    let local = [with_prefix(game("A"), "/prefixes/a")];
    let mut theirs = game("A");
    theirs.skips.clear();
    let merged = merge(
        &local,
        &listed(vec![("A", Ok(Stored::Game(Box::new(theirs.clone()))))]),
        true,
    );
    assert_eq!(merged.games, [with_prefix(theirs, "/prefixes/a")]);
}

#[test]
fn merge_drops_removed_games_in_both_modes() {
    let local = [with_prefix(game("A"), "/prefixes/a"), game("B")];
    let listed = listed(vec![("A", Ok(Stored::Removed))]);
    let mirrored = merge(&local, &listed, true);
    assert_eq!(names(&mirrored.games), Vec::<&str>::new());
    assert!(
        mirrored.removed.is_empty(),
        "a mirror reports no removed games"
    );
    let migrated = merge(&local, &listed, false);
    assert_eq!(names(&migrated.games), ["B"]);
    assert_eq!(migrated.upload, ["B"]);
    assert_eq!(migrated.removed, ["A"]);
}

#[test]
fn merge_drops_unknown_games_when_mirroring_and_uploads_them_when_migrating() {
    let local = [game("Local"), game("Both")];
    let listed = listed(vec![("Both", Ok(Stored::Game(Box::new(game("Both")))))]);
    assert_eq!(names(&merge(&local, &listed, true).games), ["Both"]);

    let migrated = merge(&local, &listed, false);
    assert_eq!(names(&migrated.games), ["Local", "Both"]);
    assert_eq!(migrated.upload, ["Local"]);
    assert!(
        migrated.replaced.is_empty(),
        "equal copies are not replaced"
    );
}

#[test]
fn merge_takes_the_repository_version_when_migrating_and_names_the_game() {
    let local = [with_prefix(game("A"), "/prefixes/a")];
    let mut repository = game("A");
    repository.integration = Integration::Override;
    let listed = listed(vec![("A", Ok(Stored::Game(Box::new(repository.clone()))))]);
    let migrated = merge(&local, &listed, false);
    assert_eq!(migrated.games, [with_prefix(repository, "/prefixes/a")]);
    assert_eq!(migrated.replaced, ["A"]);
    assert!(migrated.upload.is_empty());
}

#[test]
fn merge_keeps_the_local_copy_of_an_unreadable_game() {
    let local = [game("A")];
    let listed = listed(vec![
        ("A", Err(CustomGameError::NewerFormat(2))),
        ("B", Err(CustomGameError::NotWritten)),
    ]);
    for mirrors in [true, false] {
        let merged = merge(&local, &listed, mirrors);
        assert_eq!(merged.games, local);
        assert!(merged.upload.is_empty());
    }
}

#[test]
fn sync_uploads_this_pcs_games_once_and_the_repository_wins_a_conflict() {
    let (f, mut storage) = Fixture::new();
    let mut theirs = game("Shared");
    theirs.integration = Integration::Override;
    f.save(&mut f.open(), "Shared", None, Some(&theirs));
    f.save(&mut f.open(), "Theirs", None, Some(&game("Theirs")));

    let local = [
        with_prefix(game("Shared"), "/prefixes/shared"),
        game("Mine"),
    ];
    let synced = sync(&mut storage, &local, false, &f.stage).unwrap();
    assert_eq!(synced.uploaded, ["Mine"]);
    assert_eq!(synced.replaced, ["Shared"]);
    assert!(synced.failed.is_empty());
    assert!(synced.complete);
    assert!(synced.unreadable.is_empty());
    assert_eq!(
        synced.games,
        [
            with_prefix(theirs.clone(), "/prefixes/shared"),
            game("Mine"),
            game("Theirs"),
        ]
    );
    assert_eq!(
        loaded(&mut f.open())["Shared"],
        Stored::Game(Box::new(theirs))
    );

    let before = ids(&storage);
    let again = sync(&mut storage, &synced.games, true, &f.stage).unwrap();
    assert!(again.uploaded.is_empty());
    assert_eq!(again.games, synced.games);
    assert_eq!(ids(&storage), before);
}

#[test]
fn sync_reports_a_local_game_the_repository_removed_when_migrating() {
    let (f, mut storage) = Fixture::new();
    let mut other = f.open();
    f.save(&mut other, "Gone", None, Some(&game("Gone")));
    f.save(&mut other, "Gone", Some(&game("Gone")), None);
    let before = ids(&storage);

    let local = [with_prefix(game("Gone"), "/prefixes/gone"), game("Kept")];
    let synced = sync(&mut storage, &local, false, &f.stage).unwrap();
    assert_eq!(synced.removed, ["Gone"]);
    assert_eq!(synced.games, [game("Kept")]);
    assert_eq!(synced.uploaded, ["Kept"]);
    assert_eq!(loaded(&mut f.open())["Gone"], Stored::Removed);
    assert_eq!(
        ids(&storage).len(),
        before.len() + 1,
        "only Kept was written"
    );
}

#[test]
fn sync_reports_unreadable_games() {
    let (f, mut storage) = Fixture::new();
    f.write_raw(&mut storage, GAME, r#"{ "format": 2 }"#);
    let synced = sync(&mut storage, &[game(GAME)], true, &f.stage).unwrap();
    assert_eq!(synced.games, [game(GAME)]);
    assert!(matches!(
        synced.unreadable[GAME],
        CustomGameError::NewerFormat(2)
    ));
    assert!(synced.complete, "a mirror stays a mirror");

    // Migrating, the local copy of an unreadable game was never compared with the repository's.
    let synced = sync(&mut storage, &[game(GAME)], false, &f.stage).unwrap();
    assert_eq!(synced.games, [game(GAME)]);
    assert!(!synced.complete);
    // An unreadable game this PC lacks leaves nothing to compare.
    let synced = sync(&mut storage, &[], false, &f.stage).unwrap();
    assert!(synced.complete);
}

#[test]
fn sync_with_a_failed_upload_is_incomplete() {
    let (f, mut storage) = Fixture::new();
    // A directory where the game's file is staged makes its upload fail.
    let blocked = (f.stage.join("settings").join(STAGE_DIR))
        .join(slug("Blocked"))
        .join(GAME_FILE);
    fs::create_dir_all(&blocked).unwrap();
    let local = [game("Blocked"), game("Mine")];
    let synced = sync(&mut storage, &local, false, &f.stage).unwrap();
    assert_eq!(synced.uploaded, ["Mine"]);
    assert!(matches!(
        synced.failed.as_slice(),
        [(name, CustomGameError::Io { .. })] if name == "Blocked"
    ));
    assert!(!synced.complete);
    assert_eq!(synced.games, local, "the game that failed stays");
}
