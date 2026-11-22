use std::num::NonZeroU32;

use super::*;
use crate::retention::{Unit, Within};
use crate::selection::{self, Change};
use crate::storage::{Compression, RepoLocation};

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
            options: Default::default(),
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

    fn save(&self, storage: &mut Storage, policy: &Retention) -> Option<SnapshotInfo> {
        let saved = save(storage, policy, &self.stage).unwrap();
        assert!(saved.forget_error.is_none(), "{:?}", saved.forget_error);
        saved.written
    }

    /// Writes `json` as the newest policy snapshot, as another version would.
    fn write_raw(&self, storage: &mut Storage, json: &str) {
        let dir = self.stage.join("raw");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(POLICY_FILE), json).unwrap();
        storage
            .backup_dir(&dir, AS_PATH, &[SETTINGS_TAG, POLICY_TAG], LABEL)
            .unwrap()
            .unwrap();
    }
}

fn both() -> Retention {
    Retention {
        keep_last: NonZeroU32::new(5),
        keep_within: Some(Within {
            amount: NonZeroU32::new(3).unwrap(),
            unit: Unit::Months,
        }),
    }
}

fn keep_last(n: u32) -> Retention {
    Retention {
        keep_last: NonZeroU32::new(n),
        keep_within: None,
    }
}

#[test]
fn missing_policy_is_off() {
    let (_f, mut storage) = Fixture::new();
    assert_eq!(load(&mut storage).unwrap(), Retention::default());
}

#[test]
fn save_then_load_round_trips() {
    let (f, mut storage) = Fixture::new();
    assert!(f.save(&mut storage, &both()).is_some());
    assert_eq!(load(&mut f.open()).unwrap(), both());
}

#[test]
fn a_save_replaces_the_snapshot_it_read() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, &keep_last(5)).unwrap();
    let second = f.save(&mut storage, &keep_last(3)).unwrap();
    assert_eq!(storage.snapshots(POLICY_TAG).unwrap(), vec![second]);
    assert_eq!(load(&mut storage).unwrap(), keep_last(3));
}

#[test]
fn unchanged_save_writes_nothing() {
    let (f, mut storage) = Fixture::new();
    assert!(f.save(&mut storage, &Retention::default()).is_none());
    assert!(storage.snapshots(POLICY_TAG).unwrap().is_empty());

    f.save(&mut storage, &both()).unwrap();
    let before = storage.snapshots(POLICY_TAG).unwrap();
    assert!(f.save(&mut storage, &both()).is_none());
    assert_eq!(storage.snapshots(POLICY_TAG).unwrap(), before);
}

#[test]
fn turning_every_rule_off_is_saved() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, &both()).unwrap();
    assert!(f.save(&mut storage, &Retention::default()).is_some());
    assert!(load(&mut f.open()).unwrap().is_off());
}

#[test]
fn a_failed_forget_still_saves() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, &keep_last(5)).unwrap();
    storage.set_append_only();
    let saved = save(&mut storage, &keep_last(3), &f.stage).unwrap();
    assert!(saved.written.is_some());
    assert!(saved.forget_error.is_some());
    assert_eq!(load(&mut storage).unwrap(), keep_last(3));
}

/// The newest policy and the ids of all policy snapshots, as a save reads them.
fn newest_and_ids(storage: &mut Storage) -> (Retention, Vec<String>) {
    let listed = storage.snapshots(POLICY_TAG).unwrap();
    let newest = read(storage, &listed[0]).unwrap();
    (
        newest,
        listed.into_iter().map(|snapshot| snapshot.id).collect(),
    )
}

#[test]
fn a_save_that_raced_another_is_not_written_and_forgets_its_snapshot() {
    let (f, mut a) = Fixture::new();
    f.save(&mut a, &keep_last(5)).unwrap();
    let mut b = f.open();
    let (current_a, ids_a) = newest_and_ids(&mut a);
    let (current_b, ids_b) = newest_and_ids(&mut b);

    write(&mut a, &keep_last(3), &current_a, &ids_a, &f.stage).unwrap();
    let result = write(&mut b, &keep_last(2), &current_b, &ids_b, &f.stage);
    assert!(matches!(result, Err(RetentionError::NotWritten)));
    assert_eq!(load(&mut a).unwrap(), keep_last(3));
    assert_eq!(a.snapshots(POLICY_TAG).unwrap().len(), 1);
}

#[test]
fn newer_format_is_neither_loaded_nor_overwritten() {
    let (f, mut storage) = Fixture::new();
    f.write_raw(
        &mut storage,
        r#"{ "format": 2, "policy": { "keep_every": 1 } }"#,
    );
    let before = storage.snapshots(POLICY_TAG).unwrap();
    assert!(matches!(
        load(&mut storage),
        Err(RetentionError::NewerFormat(2))
    ));
    assert!(matches!(
        save(&mut storage, &both(), &f.stage),
        Err(RetentionError::NewerFormat(2))
    ));
    assert_eq!(storage.snapshots(POLICY_TAG).unwrap(), before);
}

#[test]
fn unknown_fields_are_a_parse_error() {
    for json in [
        r#"{ "format": 1, "policy": {}, "extra": true }"#,
        r#"{ "format": 1, "policy": { "keep_last": 2, "keep_every": 1 } }"#,
    ] {
        let (f, mut storage) = Fixture::new();
        f.write_raw(&mut storage, json);
        assert!(
            matches!(load(&mut storage), Err(RetentionError::Parse(_))),
            "{json}"
        );
    }
}

#[test]
fn policy_snapshots_are_neither_game_nor_selection_snapshots() {
    let (f, mut storage) = Fixture::new();
    f.save(&mut storage, &both()).unwrap();
    selection::save(&mut storage, "Game", &[Change::Skip(true)], &f.stage).unwrap();
    assert!(storage.snapshots("salvajocs").unwrap().is_empty());
    let all = selection::load_all(&mut storage).unwrap();
    assert_eq!(all.keys().collect::<Vec<_>>(), ["Game"]);
}
