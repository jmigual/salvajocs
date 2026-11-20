use std::path::PathBuf;

use super::*;
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

    /// A second session on the same repository, as another machine would have.
    fn open(&self) -> Storage {
        Storage::open(&self.location, "").unwrap()
    }

    fn save(&self, storage: &mut Storage, title: &str, changes: &[Change]) -> Option<SnapshotInfo> {
        let saved = save(storage, title, changes, &self.stage).unwrap();
        assert!(saved.forget_error.is_none(), "{:?}", saved.forget_error);
        saved.written
    }
}

fn exclude(node: &str) -> Change {
    Change::Path {
        node: node.into(),
        included: false,
    }
}

fn expected(changes: &[Change]) -> GameSettings {
    let mut settings = GameSettings::default();
    settings.apply_changes(changes);
    settings
}

fn loaded(storage: &mut Storage) -> BTreeMap<String, GameSettings> {
    load_all(storage)
        .unwrap()
        .into_iter()
        .map(|(title, settings)| (title, settings.unwrap()))
        .collect()
}

#[test]
fn save_then_load_round_trips() {
    let (f, mut storage) = Fixture::new();
    let changes = [Change::Skip(true), exclude("Home/a")];
    assert!(f.save(&mut storage, GAME, &changes).is_some());
    let mut reopened = f.open();
    assert_eq!(
        loaded(&mut reopened),
        BTreeMap::from([(GAME.to_string(), expected(&changes))])
    );
}

#[test]
fn settings_snapshots_are_not_game_snapshots() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, GAME, &[Change::Skip(true)]).unwrap();
    assert!(storage.snapshots("salvajocs").unwrap().is_empty());
    assert!(
        storage
            .snapshots(&format!("game:{}", slug(GAME)))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn unchanged_save_writes_nothing() {
    let (f, mut storage) = Fixture::new();
    assert!(f.save(&mut storage, GAME, &[Change::Skip(false)]).is_none());
    assert!(storage.snapshots(SETTINGS_TAG).unwrap().is_empty());

    f.save(&mut storage, GAME, &[Change::Skip(true)]).unwrap();
    let before = storage.snapshots(SETTINGS_TAG).unwrap();
    assert!(f.save(&mut storage, GAME, &[Change::Skip(true)]).is_none());
    assert_eq!(storage.snapshots(SETTINGS_TAG).unwrap(), before);
}

#[test]
fn a_save_replaces_the_snapshot_it_read() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, GAME, &[exclude("Home/a")]).unwrap();
    let second = f.save(&mut storage, GAME, &[exclude("Home/b")]).unwrap();
    assert_eq!(storage.snapshots(SETTINGS_TAG).unwrap(), vec![second]);
}

#[test]
fn a_save_forgets_only_what_it_read() {
    let (f, mut storage) = Fixture::new();
    let first = f.save(&mut storage, GAME, &[exclude("Home/a")]).unwrap();
    let read = read_newest(&mut storage, GAME).unwrap();

    // Another machine saves between this save's read and its write.
    let mut other = f.open();
    let theirs = f.save(&mut other, GAME, &[exclude("Home/b")]).unwrap();

    // Its forget finds the snapshot it read already gone, which is no error.
    let saved = write(&mut storage, GAME, &[exclude("Home/c")], &read, &f.stage).unwrap();
    assert!(saved.forget_error.is_none());
    let ours = saved.written.unwrap();
    let ids: Vec<String> = storage
        .snapshots(SETTINGS_TAG)
        .unwrap()
        .into_iter()
        .map(|snapshot| snapshot.id)
        .collect();
    assert!(ids.contains(&theirs.id));
    assert!(ids.contains(&ours.id));
    assert!(!ids.contains(&first.id));

    // The next save, even an unchanged one, forgets the leftover older snapshot.
    assert!(f.save(&mut storage, GAME, &[]).is_none());
    assert_eq!(storage.snapshots(SETTINGS_TAG).unwrap(), vec![ours]);
}

#[test]
fn a_failed_forget_still_saves() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, GAME, &[exclude("Home/a")]).unwrap();
    storage.set_append_only();
    let saved = save(&mut storage, GAME, &[exclude("Home/b")], &f.stage).unwrap();
    assert!(saved.written.is_some());
    assert!(saved.forget_error.is_some());
    assert_eq!(
        loaded(&mut storage)[GAME],
        expected(&[exclude("Home/a"), exclude("Home/b")])
    );
}

#[test]
fn load_all_reads_snapshots_written_after_the_index_loaded() {
    let (f, mut storage) = Fixture::new();
    let mut other = f.open();
    f.save(&mut storage, GAME, &[Change::Skip(true)]).unwrap();
    assert_eq!(loaded(&mut other)[GAME], expected(&[Change::Skip(true)]));
}

#[test]
fn settings_stage_apart_from_game_backups() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, GAME, &[Change::Skip(true)]).unwrap();
    let slug = slug(GAME);
    assert!(
        f.stage
            .join(STAGE_DIR)
            .join(&slug)
            .join(SETTINGS_FILE)
            .is_file()
    );
    assert!(!f.stage.join(&slug).exists());
}

#[test]
fn saves_from_the_same_stale_view_both_keep_their_changes() {
    let (f, mut storage) = Fixture::new();
    // Both sessions load before either saves.
    let mut other = f.open();
    assert!(loaded(&mut storage).is_empty());
    assert!(loaded(&mut other).is_empty());

    f.save(&mut storage, GAME, &[exclude("Home/a")]).unwrap();
    f.save(&mut other, GAME, &[exclude("Home/b")]).unwrap();

    let both = expected(&[exclude("Home/a"), exclude("Home/b")]);
    assert_eq!(loaded(&mut f.open())[GAME], both);
    assert_eq!(storage.snapshots(SETTINGS_TAG).unwrap().len(), 1);
}

#[test]
fn games_do_not_interfere() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, "One", &[exclude("Home/a")]).unwrap();
    f.save(&mut storage, "Two", &[Change::Skip(true)]).unwrap();
    f.save(&mut storage, "One", &[exclude("Home/b")]).unwrap();
    assert_eq!(
        loaded(&mut f.open()),
        BTreeMap::from([
            (
                "One".to_string(),
                expected(&[exclude("Home/a"), exclude("Home/b")])
            ),
            ("Two".to_string(), expected(&[Change::Skip(true)])),
        ])
    );
}

/// Custom games are settings snapshots too, under a tag outside `settings:`, which this code,
/// the same since 0.4.0, would read as a game's selection (ADR-0028).
#[test]
fn load_all_ignores_custom_game_snapshots() {
    let (f, mut storage) = Fixture::new();
    let game = crate::manifest::CustomGame::new(
        GAME.into(),
        crate::manifest::GameEntry::default(),
        crate::manifest::Integration::Extend,
    );
    crate::custom_games::save(&mut storage, GAME, None, Some(&game), &f.stage).unwrap();
    assert!(load_all(&mut storage).unwrap().is_empty());

    f.save(&mut storage, GAME, &[Change::Skip(true)]).unwrap();
    assert_eq!(
        loaded(&mut f.open()),
        BTreeMap::from([(GAME.to_string(), expected(&[Change::Skip(true)]))])
    );
}

#[test]
fn newer_format_is_neither_loaded_nor_saved() {
    let (f, mut storage) = Fixture::new();
    // A newer version's settings snapshot.
    let slug = slug(GAME);
    let dir = f.stage.join("newer");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(SETTINGS_FILE), r#"{ "format": 2 }"#).unwrap();
    let tag = game_tag(&slug);
    storage
        .backup_dir(&dir, &as_path(&slug), &[SETTINGS_TAG, &tag], GAME)
        .unwrap()
        .unwrap();
    f.save(&mut storage, "Other", &[Change::Skip(true)])
        .unwrap();
    let before = storage.snapshots(SETTINGS_TAG).unwrap();

    assert!(matches!(
        save(&mut storage, GAME, &[Change::Skip(true)], &f.stage),
        Err(SelectionError::NewerFormat(2))
    ));
    assert_eq!(storage.snapshots(SETTINGS_TAG).unwrap(), before);

    let all = load_all(&mut f.open()).unwrap();
    assert!(matches!(all[GAME], Err(SelectionError::NewerFormat(2))));
    assert!(all["Other"].is_ok());
}
