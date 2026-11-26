use std::num::NonZeroU32;
use std::time::SystemTime;

use super::*;
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
                true,
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
    serde_json::from_slice(&storage.read_file(&snapshot.id, &path, u64::MAX).unwrap()).unwrap()
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

    // Another PC forgets what this save read and saves a newer one, after the read.
    let mut theirs = game(GAME);
    theirs.skips.push("<home>/Theirs".into());
    let theirs = f
        .save(&mut f.open(), GAME, Some(&game(GAME)), Some(&theirs))
        .unwrap();
    assert!(!ids(&storage).contains(&first.id));

    // Their snapshot is one this save never read, so the new snapshot, which lacks theirs, is
    // withdrawn.
    let mut ours = game(GAME);
    ours.skips.push("<home>/Ours".into());
    let result = write(&mut storage, GAME, Some(&ours), &read, &f.stage);
    assert!(matches!(result, Err(CustomGameError::NotWritten)));
    assert_eq!(ids(&storage), vec![theirs.id]);
}

#[test]
fn forgetting_what_was_read_skips_a_snapshot_another_pc_already_forgot() {
    let (f, mut storage) = Fixture::new();
    let first = f.save(&mut storage, GAME, None, Some(&game(GAME))).unwrap();
    let mut theirs = game(GAME);
    theirs.skips.push("<home>/Theirs".into());
    let second = f
        .save(&mut f.open(), GAME, Some(&game(GAME)), Some(&theirs))
        .unwrap();
    // `first` is gone for good, `second` is still listed.
    let read = vec![first.id, second.id];
    assert!(forget_listed(&storage, &game_tag(&slug(GAME)), &read).is_none());
    assert!(ids(&storage).is_empty());
}

#[test]
fn a_racing_save_that_cannot_forget_its_snapshot_fails_and_leaves_it() {
    let (f, mut a) = Fixture::new();
    f.save(&mut a, GAME, None, Some(&game(GAME))).unwrap();
    let read_a = read_newest(&mut a, GAME).unwrap();
    let mut b = f.open();
    let read_b = read_newest(&mut b, GAME).unwrap();

    let mut mine = game(GAME);
    mine.skips.push("<home>/A".into());
    write(&mut a, GAME, Some(&mine), &read_a, &f.stage).unwrap();
    b.set_append_only();
    let mut theirs = game(GAME);
    theirs.skips.push("<home>/B".into());
    let result = write(&mut b, GAME, Some(&theirs), &read_b, &f.stage);
    assert!(matches!(result, Err(CustomGameError::Storage(_))));
    assert_eq!(ids(&a).len(), 2);
}

#[test]
fn a_save_that_raced_another_is_changed_once_retried() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, GAME, None, Some(&game(GAME))).unwrap();
    let read = read_newest(&mut storage, GAME).unwrap();

    let mut theirs = game(GAME);
    theirs.skips.push("<home>/Theirs".into());
    f.save(&mut f.open(), GAME, Some(&game(GAME)), Some(&theirs))
        .unwrap();
    let mut ours = game(GAME);
    ours.skips.push("<home>/Ours".into());
    let result = write(&mut storage, GAME, Some(&ours), &read, &f.stage);
    assert!(matches!(result, Err(CustomGameError::NotWritten)));

    // The retry meets their copy, so the edit goes through the conflict flow.
    let result = save(&mut storage, GAME, Some(&game(GAME)), Some(&ours), &f.stage);
    match result {
        Err(CustomGameError::Changed(Some(newest))) => assert_eq!(*newest, theirs),
        other => panic!("expected Changed, got {other:?}"),
    }
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

fn listed(
    entries: Vec<(&str, Result<Stored, CustomGameError>)>,
) -> BTreeMap<String, Result<Stored, CustomGameError>> {
    (entries.into_iter())
        .map(|(name, stored)| (name.to_owned(), stored))
        .collect()
}

fn stored(game: CustomGame) -> Result<Stored, CustomGameError> {
    Ok(Stored::Game(Box::new(game)))
}

#[test]
fn merge_keeps_the_local_copy_and_offers_a_changed_one() {
    let local = [game("B"), with_prefix(game("A"), "/prefixes/a")];
    let mut changed = game("A");
    changed.skips.clear();
    let listed = listed(vec![
        ("D", stored(game("D"))),
        ("A", stored(changed.clone())),
        ("C", stored(game("C"))),
        ("B", stored(game("B"))),
    ]);
    let merged = merge(&local, &[], &listed, true);
    assert_eq!(merged.games, local);
    assert_eq!(merged.offered, [changed, game("C"), game("D")]);
    assert!(merged.new_unsent.is_empty());
    assert!(merged.complete);
}

#[test]
fn merge_offers_a_game_this_pc_lacks() {
    let listed = listed(vec![("G", stored(game("G")))]);
    for mirrors in [true, false] {
        let merged = merge(&[], &[], &listed, mirrors);
        assert!(merged.games.is_empty());
        assert_eq!(merged.offered, [game("G")]);
    }
}

#[test]
fn merge_offers_nothing_for_an_equal_copy_or_an_unsent_name() {
    let local = [
        with_prefix(game("Equal"), "/prefixes/equal"),
        game("Edited"),
    ];
    let unsent = [
        Unsent::new("Edited".into(), Some(game("Edited"))),
        Unsent::new("Added".into(), None),
    ];
    let mut theirs = game("Edited");
    theirs.files.clear();
    let listed = listed(vec![
        ("Equal", stored(game("Equal"))),
        ("Edited", stored(theirs)),
        ("Added", stored(game("Added"))),
    ]);
    for mirrors in [true, false] {
        let merged = merge(&local, &unsent, &listed, mirrors);
        assert_eq!(merged.games, local);
        assert!(merged.offered.is_empty());
    }
}

#[test]
fn merge_still_drops_a_removed_game() {
    let local = [game("Removed"), game("Kept")];
    let listed = listed(vec![("Removed", Ok(Stored::Removed))]);
    let merged = merge(&local, &[], &listed, true);
    assert_eq!(merged.games, [game("Kept")]);
    assert!(merged.offered.is_empty());
}

#[test]
fn a_mirror_keeps_each_local_game_it_cannot_compare() {
    let local = [game("Removed"), game("Unknown"), game("Unreadable")];
    let listed = listed(vec![
        ("Removed", Ok(Stored::Removed)),
        ("Unreadable", Err(CustomGameError::NewerFormat(2))),
    ]);
    let merged = merge(&local, &[], &listed, true);
    assert_eq!(merged.games, [game("Unknown"), game("Unreadable")]);
    // A game the repository has no snapshot of is kept, to be sent as new.
    assert_eq!(merged.new_unsent, [Unsent::new("Unknown".into(), None)]);
    assert!(merged.complete);
}

#[test]
fn a_merge_leaves_every_unsent_game_as_this_pc_has_it() {
    let mut mine = game("Edited");
    mine.skips.clear();
    let local = [mine.clone(), game("Added")];
    let unsent = [
        Unsent::new("Edited".into(), Some(game("Edited"))),
        Unsent::new("Added".into(), None),
        Unsent::new("Removed".into(), Some(game("Removed"))),
    ];
    let mut theirs = game("Edited");
    theirs.files.clear();
    let listed = listed(vec![
        ("Edited", stored(theirs)),
        ("Removed", stored(game("Removed"))),
    ]);
    for mirrors in [true, false] {
        let merged = merge(&local, &unsent, &listed, mirrors);
        assert_eq!(merged.games, local, "the removal is not added back");
        assert!(merged.new_unsent.is_empty());
    }
}

#[test]
fn a_migration_keeps_every_local_game_and_marks_those_the_repository_differs_on() {
    let local = [
        with_prefix(game("Equal"), "/prefixes/equal"),
        game("Lacking"),
        game("Other"),
        game("Removed"),
    ];
    let mut other = game("Other");
    other.integration = Integration::Override;
    let listed = listed(vec![
        ("Equal", stored(game("Equal"))),
        ("Other", stored(other)),
        ("Removed", Ok(Stored::Removed)),
        ("Theirs", stored(game("Theirs"))),
    ]);
    let merged = merge(&local, &[], &listed, false);
    assert_eq!(merged.games, local);
    assert_eq!(merged.offered, [game("Theirs")]);
    assert_eq!(
        merged.new_unsent,
        ["Lacking", "Other", "Removed"].map(|name| Unsent::new(name.into(), Some(game(name))))
    );
    assert!(merged.complete);
}

#[test]
fn a_migration_that_meets_an_unreadable_copy_is_incomplete() {
    let local = [game("A")];
    let listed = listed(vec![
        ("A", Err(CustomGameError::NewerFormat(2))),
        ("B", Err(CustomGameError::NotWritten)),
    ]);
    let merged = merge(&local, &[], &listed, false);
    assert_eq!(merged.games, local);
    assert!(merged.new_unsent.is_empty());
    assert!(!merged.complete);
    // An unreadable game this PC lacks leaves nothing to compare.
    assert!(merge(&[], &[], &listed, false).complete);
}

#[test]
fn a_change_the_repository_already_holds_is_not_changed() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, GAME, None, Some(&game(GAME))).unwrap();
    let mut both = game(GAME);
    both.skips.clear();
    // Another PC made the same edit first.
    f.save(&mut f.open(), GAME, Some(&game(GAME)), Some(&both))
        .unwrap();
    let before = ids(&storage);
    let saved = save(&mut storage, GAME, Some(&game(GAME)), Some(&both), &f.stage).unwrap();
    assert!(saved.written.is_none());
    assert_eq!(ids(&storage), before);

    // The same holds for a removal of a game another PC removed too.
    f.save(&mut f.open(), GAME, Some(&both), None);
    let saved = save(&mut storage, GAME, Some(&game(GAME)), None, &f.stage).unwrap();
    assert!(saved.written.is_none());
}

fn unsent(name: &str, based_on: Option<&CustomGame>) -> Unsent {
    Unsent::new(name.into(), based_on.cloned())
}

#[test]
fn an_unsent_base_has_no_wine_prefix() {
    let entry = unsent(GAME, Some(&with_prefix(game(GAME), "/prefixes/game")));
    assert_eq!(entry.based_on, Some(game(GAME)));
}

#[test]
fn conflicts_are_changes_made_elsewhere_since_the_base() {
    let (f, mut storage) = Fixture::new();
    let mut other = f.open();
    let mut theirs = game("Edited");
    theirs.skips.clear();
    f.save(&mut other, "Edited", None, Some(&theirs));
    f.save(&mut other, "Same", None, Some(&game("Same")));
    f.save(&mut other, "Gone", None, Some(&game("Gone")));
    f.save(&mut other, "Gone", Some(&game("Gone")), None);
    f.save(&mut other, "Base", None, Some(&game("Base")));
    f.write_raw(&mut other, "Newer", r#"{ "format": 2, "name": "Newer" }"#);

    let mut mine = with_prefix(game("Edited"), "/prefixes/edited");
    mine.files.clear();
    let games = [
        mine,
        game("Same"),
        game("Gone"),
        game("Base"),
        game("New"),
        game("Newer"),
    ];
    let entries = [
        // Edited here from game("Edited"), and since then elsewhere.
        unsent("Edited", Some(&game("Edited"))),
        // Changed to what the repository holds already.
        unsent("Same", Some(&game("Other"))),
        // A migration's entry: based on this PC's copy, removed elsewhere.
        unsent("Gone", Some(&game("Gone"))),
        // Unchanged elsewhere since the base.
        unsent("Base", Some(&game("Base"))),
        // A name the repository never had.
        unsent("New", None),
        // A copy that cannot be read is the send's to report.
        unsent("Newer", None),
        // Removed here from a copy the repository has changed since.
        unsent("Base", Some(&game("Older"))),
    ];
    let found = conflicts(&mut storage, &games, &entries[..6]).unwrap();
    assert_eq!(
        found,
        [
            Conflict {
                name: "Edited".into(),
                newest: Some(theirs)
            },
            Conflict {
                name: "Gone".into(),
                newest: None
            },
        ]
    );
    let found = conflicts(&mut storage, &[], &entries[6..]).unwrap();
    let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Base"]);
}

#[test]
fn send_writes_games_then_removals_and_reports_each() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, "Old", None, Some(&game("Old")));
    f.save(&mut storage, "Kept", None, Some(&game("Kept")));
    let mut edited = with_prefix(game("Kept"), "/prefixes/kept");
    edited.skips.clear();
    let games = [game("New"), edited.clone()];
    // A rename of Old to New, and an edit of Kept.
    let entries = [
        unsent("Old", Some(&game("Old"))),
        unsent("New", None),
        unsent("Kept", Some(&game("Kept"))),
    ];
    let sent = send(&mut storage, &games, &entries, &f.stage);
    assert!(sent.failed.is_empty(), "{:?}", sent.failed);
    assert!(sent.held_back.is_empty());
    assert!(sent.forget_errors.is_empty());
    let mut shared_edit = edited.clone();
    shared_edit.wine_prefix = None;
    assert_eq!(
        sent.sent,
        [
            ("New".to_owned(), Some(game("New"))),
            ("Kept".to_owned(), Some(shared_edit.clone())),
            ("Old".to_owned(), None),
        ]
    );
    assert_eq!(sent.written, ["New", "Kept", "Old"]);
    let all = loaded(&mut f.open());
    assert_eq!(all["Old"], Stored::Removed);
    assert_eq!(all["New"], Stored::Game(Box::new(game("New"))));
    assert_eq!(all["Kept"], Stored::Game(Box::new(shared_edit)));
}

#[test]
fn send_holds_removals_back_after_a_write_that_may_pass() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, "Old", None, Some(&game("Old")));
    // A directory where the game's file is staged makes its write fail.
    let blocked = (f.stage.join("settings").join(STAGE_DIR))
        .join(slug("New"))
        .join(GAME_FILE);
    fs::create_dir_all(&blocked).unwrap();
    let entries = [unsent("Old", Some(&game("Old"))), unsent("New", None)];
    let sent = send(&mut storage, &[game("New")], &entries, &f.stage);
    assert!(matches!(
        sent.failed.as_slice(),
        [(name, CustomGameError::Io { .. })] if name == "New"
    ));
    assert_eq!(sent.held_back, ["Old"]);
    assert!(sent.sent.is_empty());
    assert_eq!(
        loaded(&mut f.open())["Old"],
        Stored::Game(Box::new(game("Old")))
    );
}

#[test]
fn send_of_one_stuck_game_does_not_hold_back_another_removal() {
    let (f, mut storage) = Fixture::new();
    f.write_raw(&mut storage, "Newer", r#"{ "format": 2, "name": "Newer" }"#);
    f.save(&mut storage, "Gone", None, Some(&game("Gone")));
    let mut theirs = game("Changed");
    f.save(&mut storage, "Changed", None, Some(&theirs));
    theirs.skips.clear();
    f.save(
        &mut f.open(),
        "Changed",
        Some(&game("Changed")),
        Some(&theirs),
    );
    let entries = [
        unsent("Newer", None),
        unsent("Changed", Some(&game("Changed"))),
        unsent("Gone", Some(&game("Gone"))),
    ];
    let mut mine = game("Changed");
    mine.files.clear();
    let sent = send(&mut storage, &[game("Newer"), mine], &entries, &f.stage);
    assert!(matches!(
        sent.failed.as_slice(),
        [
            (_, CustomGameError::NewerFormat(2)),
            (_, CustomGameError::Changed(Some(_))),
        ]
    ));
    assert!(sent.held_back.is_empty());
    assert_eq!(sent.sent, [("Gone".to_owned(), None)]);
    let all = load_all(&mut f.open()).unwrap();
    assert!(matches!(all["Gone"], Ok(Stored::Removed)));
}

#[test]
fn after_send_drops_what_was_sent_and_rebases_what_changed_meanwhile() {
    let mut edited = game("Edited");
    edited.skips.clear();
    let mut entries = vec![
        unsent("Sent", Some(&game("Base"))),
        unsent("Edited", Some(&game("Edited"))),
        unsent("Removed", Some(&game("Removed"))),
        unsent("Failed", None),
        unsent("Later", None),
    ];
    // During the backup Edited was edited again, and Later added.
    let mut again = with_prefix(edited.clone(), "/prefixes/edited");
    again.files.clear();
    let games = [
        with_prefix(game("Sent"), "/prefixes/sent"),
        again,
        game("Failed"),
        game("Later"),
    ];
    let sent = [
        ("Sent".to_owned(), Some(game("Sent"))),
        ("Edited".to_owned(), Some(edited.clone())),
        ("Removed".to_owned(), None),
    ];
    after_send(&mut entries, &games, &sent);
    assert_eq!(
        entries,
        [
            unsent("Edited", Some(&edited)),
            unsent("Failed", None),
            unsent("Later", None),
        ]
    );
}

#[test]
fn a_game_added_and_removed_here_is_neither_asked_about_nor_sent() {
    let (f, mut storage) = Fixture::new();
    // Another PC has a game of the same name this PC never sent.
    f.save(&mut f.open(), "Brief", None, Some(&game("Brief")));
    let before = ids(&storage);
    let entries = [unsent("Brief", None)];
    assert!(conflicts(&mut storage, &[], &entries).unwrap().is_empty());

    let sent = send(&mut storage, &[], &entries, &f.stage);
    assert!(sent.failed.is_empty());
    assert!(sent.written.is_empty());
    assert_eq!(sent.sent, [("Brief".to_owned(), None)]);
    assert_eq!(ids(&storage), before, "the other PC's game stays");
    let mut entries = entries.to_vec();
    after_send(&mut entries, &[], &sent.sent);
    assert!(entries.is_empty());
}
