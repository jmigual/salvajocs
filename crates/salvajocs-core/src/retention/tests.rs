use super::*;

const DAY: Duration = Duration::from_secs(DAY_SECS);

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + DAY * 1000
}

/// A snapshot `age` before [`now`], tagged `salvajocs` plus `tags`.
fn snap(id: &str, host: &str, age: Duration, tags: &[&str]) -> SnapshotInfo {
    SnapshotInfo {
        id: id.to_string(),
        time: now() - age,
        host: host.to_string(),
        label: "Game".to_string(),
        tags: [APP_TAG]
            .iter()
            .chain(tags)
            .map(|tag| tag.to_string())
            .collect(),
        size: None,
    }
}

/// A regular snapshot of game `a` on host `pc`, `days` old.
fn game_a(id: &str, days: u32) -> SnapshotInfo {
    snap(id, "pc", DAY * days, &["game:a"])
}

fn keep_last(n: u32) -> Retention {
    Retention {
        keep_last: NonZeroU32::new(n),
        keep_within: None,
    }
}

fn keep_days(days: u32) -> Retention {
    Retention {
        keep_last: None,
        keep_within: Some(Within {
            amount: NonZeroU32::new(days).unwrap(),
            unit: Unit::Days,
        }),
    }
}

fn forget(policy: Retention, snapshots: &[SnapshotInfo]) -> Vec<String> {
    to_forget(&policy, snapshots, &[], now())
}

#[test]
fn off_forgets_nothing() {
    let snapshots: Vec<_> = (1..=5).map(|day| game_a(&format!("s{day}"), day)).collect();
    assert!(Retention::default().is_off());
    assert!(forget(Retention::default(), &snapshots).is_empty());
}

#[test]
fn keep_last_counts_per_game() {
    let snapshots = [
        game_a("a1", 1),
        game_a("a2", 2),
        game_a("a3", 3),
        game_a("a4", 4),
        snap("b1", "pc", DAY * 5, &["game:b"]),
        snap("b2", "pc", DAY * 6, &["game:b"]),
    ];
    assert_eq!(forget(keep_last(2), &snapshots), ["a3", "a4"]);
}

#[test]
fn sorts_by_time_and_answers_in_input_order() {
    let snapshots = [
        game_a("a4", 4),
        game_a("a1", 1),
        game_a("a3", 3),
        game_a("a2", 2),
    ];
    assert_eq!(forget(keep_last(2), &snapshots), ["a4", "a3"]);
}

#[test]
fn equal_times_break_ties_by_id() {
    let (x, y) = (game_a("x", 1), game_a("y", 1));
    assert_eq!(forget(keep_last(1), &[x.clone(), y.clone()]), ["y"]);
    assert_eq!(forget(keep_last(1), &[y, x]), ["y"]);
}

#[test]
fn ids_in_keep_are_never_forgotten() {
    // An older snapshot from a clock that ran ahead looks newer than the one just made.
    let future = SnapshotInfo {
        time: now() + DAY,
        ..game_a("future", 0)
    };
    let snapshots = [future, game_a("fresh", 0)];
    assert_eq!(forget(keep_last(1), &snapshots), ["fresh"]);
    assert!(to_forget(&keep_last(1), &snapshots, &["fresh"], now()).is_empty());
}

#[test]
fn keep_within_cutoff_boundary() {
    let snapshots = [
        game_a("newest", 0),
        game_a("at-cutoff", 7),
        snap(
            "past-cutoff",
            "pc",
            DAY * 7 + Duration::from_secs(1),
            &["game:a"],
        ),
    ];
    assert_eq!(forget(keep_days(7), &snapshots), ["past-cutoff"]);
}

#[test]
fn keep_within_measures_weeks_months_and_years() {
    for (unit, days) in [(Unit::Weeks, 7), (Unit::Months, 30), (Unit::Years, 365)] {
        let policy = Retention {
            keep_last: None,
            keep_within: Some(Within {
                amount: NonZeroU32::new(2).unwrap(),
                unit,
            }),
        };
        let snapshots = [
            game_a("newest", 0),
            game_a("inside", 2 * days),
            game_a("outside", 2 * days + 1),
        ];
        assert_eq!(forget(policy, &snapshots), ["outside"], "{unit:?}");
    }
}

#[test]
fn keep_within_longer_than_history_keeps_everything() {
    let policy = Retention {
        keep_last: None,
        keep_within: Some(Within {
            amount: NonZeroU32::MAX,
            unit: Unit::Years,
        }),
    };
    let snapshots = [game_a("newest", 0), game_a("old", 999)];
    assert!(forget(policy, &snapshots).is_empty());
}

#[test]
fn rules_combine_as_union() {
    let policy = Retention {
        keep_last: NonZeroU32::new(2),
        ..keep_days(10)
    };
    let snapshots = [
        game_a("d1", 1),
        game_a("d5", 5),
        game_a("d8", 8),
        game_a("d20", 20),
        game_a("d30", 30),
    ];
    assert_eq!(forget(policy, &snapshots), ["d20", "d30"]);

    // d5 is outside the window but among the last two.
    let policy = Retention {
        keep_last: NonZeroU32::new(2),
        ..keep_days(3)
    };
    let snapshots = [game_a("d1", 1), game_a("d5", 5), game_a("d20", 20)];
    assert_eq!(forget(policy, &snapshots), ["d20"]);
}

#[test]
fn newest_of_each_game_always_kept() {
    let snapshots = [
        game_a("a-newest", 40),
        game_a("a-old", 50),
        snap("b-newest", "pc", DAY * 60, &["game:b"]),
        snap("b-old", "pc", DAY * 70, &["game:b"]),
    ];
    assert_eq!(forget(keep_days(7), &snapshots), ["a-old", "b-old"]);
}

#[test]
fn newest_per_host_kept() {
    let snapshots = [
        game_a("pc-1", 1),
        game_a("pc-2", 2),
        snap("deck-1", "deck", DAY * 30, &["game:a"]),
        snap("deck-2", "deck", DAY * 40, &["game:a"]),
    ];
    assert_eq!(forget(keep_last(1), &snapshots), ["pc-2", "deck-2"]);
}

#[test]
fn pre_restore_never_counts_toward_keep_last() {
    let snapshots = [
        snap("pre", "pc", DAY, &["game:a", PRE_RESTORE_TAG]),
        game_a("r1", 2),
        game_a("r2", 3),
    ];
    assert!(forget(keep_last(2), &snapshots).is_empty());
}

#[test]
fn pre_restore_ages_out_by_same_rules() {
    let snapshots = [
        game_a("regular", 20),
        snap("pre-1", "pc", DAY, &["game:a", PRE_RESTORE_TAG]),
        snap("pre-2", "pc", DAY * 9, &["game:a", PRE_RESTORE_TAG]),
        snap("pre-3", "pc", DAY * 10, &["game:a", PRE_RESTORE_TAG]),
    ];
    assert_eq!(forget(keep_days(7), &snapshots), ["pre-2", "pre-3"]);
}

#[test]
fn settings_and_untagged_snapshots_untouched() {
    let other = |id: &str, days: u32, tags: &[&str]| SnapshotInfo {
        tags: tags.iter().map(|tag| tag.to_string()).collect(),
        ..snap(id, "pc", DAY * days, &[])
    };
    let snapshots = [
        game_a("game-new", 1),
        game_a("game-old", 2),
        snap("settings-new", "pc", DAY * 3, &["game:a", SETTINGS_TAG]),
        snap("settings-old", "pc", DAY * 4, &["game:a", SETTINGS_TAG]),
        snap("no-game-new", "pc", DAY * 5, &[]),
        snap("no-game-old", "pc", DAY * 6, &[]),
        snap("near-game-new", "pc", DAY * 7, &["xgame:a"]),
        snap("near-game-old", "pc", DAY * 8, &["xgame:a"]),
        other("no-app-new", 9, &["game:a"]),
        other("no-app-old", 10, &["game:a"]),
        other("near-app-new", 11, &["salvajocs-x", "game:a"]),
        other("near-app-old", 12, &["salvajocs-x", "game:a"]),
    ];
    assert_eq!(forget(keep_last(1), &snapshots), ["game-old"]);
}

#[test]
fn serde_missing_keys_are_off() {
    let policy: Retention = serde_json::from_str("{}").unwrap();
    assert!(policy.is_off());
    let policy: Retention = serde_saphyr::from_str("keep_last: 3\n").unwrap();
    assert_eq!(policy, keep_last(3));
    assert_eq!(serde_json::to_string(&Retention::default()).unwrap(), "{}");
}

#[test]
fn serde_rejects_keep_last_zero() {
    assert!(serde_json::from_str::<Retention>(r#"{"keep_last":0}"#).is_err());
    assert!(serde_saphyr::from_str::<Retention>("keep_last: 0\n").is_err());
}

#[test]
fn serde_rejects_unknown_fields() {
    assert!(serde_json::from_str::<Retention>(r#"{"keep_last":3,"keep_daily":2}"#).is_err());
    let within = r#"{"keep_within":{"amount":1,"unit":"days","extra":1}}"#;
    assert!(serde_json::from_str::<Retention>(within).is_err());
    assert!(serde_saphyr::from_str::<Retention>("keep_last: 3\nkeep_daily: 2\n").is_err());
}

#[test]
fn serde_round_trips_yaml_and_json() {
    let policy = Retention {
        keep_last: NonZeroU32::new(5),
        keep_within: Some(Within {
            amount: NonZeroU32::new(3).unwrap(),
            unit: Unit::Months,
        }),
    };
    let json = serde_json::to_string(&policy).unwrap();
    assert_eq!(
        json,
        r#"{"keep_last":5,"keep_within":{"amount":3,"unit":"months"}}"#
    );
    assert_eq!(serde_json::from_str::<Retention>(&json).unwrap(), policy);
    let yaml = serde_saphyr::to_string(&policy).unwrap();
    assert_eq!(serde_saphyr::from_str::<Retention>(&yaml).unwrap(), policy);
}

/// A repository on a temp dir with one game's save folder to back up.
struct Repo {
    tmp: tempfile::TempDir,
    location: crate::storage::RepoLocation,
    storage: Storage,
}

impl Repo {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let location = crate::storage::RepoLocation {
            repository: tmp.path().join("repo").to_str().unwrap().to_string(),
            options: Default::default(),
        };
        let storage = Storage::init(&location, "", crate::storage::Compression::Default).unwrap();
        Self {
            tmp,
            location,
            storage,
        }
    }

    /// A second session on the same repository, as another machine would have.
    fn open(&self) -> Storage {
        Storage::open(&self.location, "").unwrap()
    }

    /// Backs up game `a` with `contents` in its save file.
    fn backup(&mut self, contents: &str) -> SnapshotInfo {
        // A new backup run reindexes first, so the previous snapshot is its parent.
        self.storage.reindex().unwrap();
        let src = self.save(contents);
        self.storage
            .backup_dir(&src, "/salvajocs/a", &[APP_TAG, "game:a"], "A")
            .unwrap()
            .unwrap()
    }

    /// Writes `contents` to game `a`'s save file and returns its folder.
    fn save(&self, contents: &str) -> std::path::PathBuf {
        let src = self.tmp.path().join("save");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.sav"), contents).unwrap();
        src
    }

    /// Every file below the repository folder `dir`, such as `index`, sorted.
    fn files(&self, dir: &str) -> Vec<std::path::PathBuf> {
        let mut files = files_below(&self.tmp.path().join("repo").join(dir));
        files.sort();
        files
    }

    /// Whether a new session restores `snapshot` with `contents` in the save file.
    fn restores(&self, snapshot: &SnapshotInfo, contents: &str) -> bool {
        let out = self.tmp.path().join("out").join(&snapshot.id);
        self.open()
            .restore_path(&snapshot.id, "/salvajocs/a", &out)
            .is_ok_and(|()| std::fs::read_to_string(out.join("a.sav")).unwrap() == contents)
    }

    fn ids(&self, tag: &str) -> Vec<String> {
        let snapshots = self.storage.snapshots(tag).unwrap();
        snapshots.into_iter().map(|snapshot| snapshot.id).collect()
    }
}

#[test]
fn apply_keep_last_one_leaves_the_newest_restorable() {
    let mut repo = Repo::new();
    repo.backup("old");
    let newest = repo.backup("new");

    let packs = repo.files("data").len();

    let cleanup = apply(&mut repo.storage, &keep_last(1), &[], SystemTime::now()).unwrap();
    assert_eq!(
        cleanup,
        Cleanup {
            forgotten: 1,
            pruned: true,
            append_only_kept: 0,
        }
    );
    assert_eq!(repo.ids(APP_TAG), std::slice::from_ref(&newest.id));
    assert!(repo.restores(&newest, "new"));
    // The old snapshot's pack is now unused, but a prune only marks it (no `instant_delete`).
    assert!(repo.files("data").len() >= packs);
    assert!(repo.storage.has_marked_packs().unwrap());
}

#[test]
fn a_backup_reusing_a_marked_pack_is_recovered_by_the_next_cleanup() {
    let mut repo = Repo::new();
    repo.backup("old");
    repo.backup("new");
    // Another machine loaded the index while the "old" data was still in use.
    let mut other = repo.open();
    let now = SystemTime::now();
    let cleanup = apply(&mut repo.storage, &keep_last(1), &[], now).unwrap();
    assert_eq!(cleanup.forgotten, 1);

    // Its backup reuses the "old" data, which is now only in a marked pack.
    let src = repo.save("old");
    let theirs = other
        .backup_dir(&src, "/salvajocs/a", &[APP_TAG, "game:a"], "A")
        .unwrap()
        .unwrap();
    assert!(!repo.restores(&theirs, "old"));

    // Its own cleanup forgets nothing, but still prunes, which recovers the pack.
    let cleanup = apply(&mut other, &keep_last(5), &[&theirs.id], now).unwrap();
    assert_eq!(
        cleanup,
        Cleanup {
            forgotten: 0,
            pruned: true,
            append_only_kept: 0,
        }
    );
    assert!(repo.restores(&theirs, "old"));
}

#[test]
fn apply_leaves_settings_snapshots() {
    let mut repo = Repo::new();
    repo.backup("old");
    let stage = repo.tmp.path().join("stage");
    let skip = [crate::selection::Change::Skip(true)];
    crate::selection::save(&mut repo.storage, "A", &skip, &stage).unwrap();
    save(&mut repo.storage, &keep_last(1), &stage).unwrap();
    repo.backup("new");
    let settings = repo.ids(SETTINGS_TAG);
    assert_eq!(settings.len(), 2);

    let cleanup = apply(&mut repo.storage, &keep_last(1), &[], SystemTime::now()).unwrap();
    assert_eq!(cleanup.forgotten, 1);
    assert_eq!(repo.ids(SETTINGS_TAG), settings);
}

#[test]
fn apply_with_nothing_to_forget_does_not_prune() {
    let mut repo = Repo::new();
    repo.backup("old");
    repo.backup("new");
    let index = repo.files("index");
    let cleanup = apply(&mut repo.storage, &keep_last(2), &[], SystemTime::now()).unwrap();
    assert_eq!(cleanup, Cleanup::default());
    assert_eq!(repo.ids(APP_TAG).len(), 2);
    assert_eq!(repo.files("index"), index);
}

#[test]
fn apply_off_does_nothing() {
    let mut repo = Repo::new();
    repo.backup("old");
    repo.backup("new");
    let before = repo.ids(APP_TAG);
    let off = Retention::default();
    assert_eq!(
        apply(&mut repo.storage, &off, &[], SystemTime::now()).unwrap(),
        Cleanup::default()
    );
    assert_eq!(repo.ids(APP_TAG), before);
}

#[test]
fn apply_off_still_prunes_marked_packs() {
    let mut repo = Repo::new();
    repo.backup("old");
    repo.backup("new");
    let now = SystemTime::now();
    apply(&mut repo.storage, &keep_last(1), &[], now).unwrap();
    let cleanup = apply(&mut repo.storage, &Retention::default(), &[], now).unwrap();
    assert_eq!(
        cleanup,
        Cleanup {
            forgotten: 0,
            pruned: true,
            append_only_kept: 0,
        }
    );
}

#[test]
fn apply_in_an_append_only_repository_only_counts() {
    let mut repo = Repo::new();
    repo.backup("old");
    repo.backup("new");
    let now = SystemTime::now();
    apply(&mut repo.storage, &keep_last(1), &[], now).unwrap();
    repo.storage.set_append_only();

    // Marked packs wait without a prune, which would fail.
    let off = Retention::default();
    assert_eq!(
        apply(&mut repo.storage, &off, &[], now).unwrap(),
        Cleanup::default()
    );
    repo.backup("newer");
    let cleanup = apply(&mut repo.storage, &keep_last(1), &[], now).unwrap();
    assert_eq!(
        cleanup,
        Cleanup {
            forgotten: 0,
            pruned: false,
            append_only_kept: 1,
        }
    );
    assert_eq!(repo.ids(APP_TAG).len(), 2);
    assert!(repo.storage.has_marked_packs().unwrap());
}

#[test]
fn a_prune_that_fails_after_a_forget_reports_the_forgotten_count() {
    let mut repo = Repo::new();
    repo.backup("old");
    repo.backup("new");
    // Without its packs, planning the prune fails; the forget only removes a snapshot file.
    for pack in repo.files("data") {
        std::fs::remove_file(pack).unwrap();
    }

    let err = apply(&mut repo.storage, &keep_last(1), &[], SystemTime::now()).unwrap_err();

    assert_eq!(err.forgotten, 1);
    assert_eq!(repo.ids(APP_TAG).len(), 1);
}

#[test]
fn delete_forgets_only_the_given_ids_and_keeps_the_rest_restorable() {
    let mut repo = Repo::new();
    let old = repo.backup("old");
    let mid = repo.backup("mid");
    let new = repo.backup("new");

    let cleanup = delete(&mut repo.storage, &[&mid.id]).unwrap();

    assert_eq!(
        cleanup,
        Cleanup {
            forgotten: 1,
            pruned: true,
            append_only_kept: 0,
        }
    );
    assert_eq!(repo.ids(APP_TAG), [new.id.clone(), old.id.clone()]);
    assert!(repo.restores(&new, "new"));
    assert!(repo.restores(&old, "old"));
}

#[test]
fn delete_refuses_a_settings_snapshot_and_forgets_nothing() {
    let mut repo = Repo::new();
    let game = repo.backup("old");
    let stage = repo.tmp.path().join("stage");
    let skip = [crate::selection::Change::Skip(true)];
    crate::selection::save(&mut repo.storage, "A", &skip, &stage).unwrap();
    let settings = repo.ids(SETTINGS_TAG);

    let err = delete(&mut repo.storage, &[&game.id, &settings[0]]).unwrap_err();

    assert!(matches!(err, DeleteError::NotAGameBackup(_)));
    assert_eq!(repo.ids(APP_TAG), [game.id]);
    assert_eq!(repo.ids(SETTINGS_TAG), settings);
}

#[test]
fn delete_refuses_a_snapshot_retention_never_picks() {
    let mut repo = Repo::new();
    let game = repo.backup("old");
    let src = repo.save("other");
    for (label, tags) in [
        ("untagged", &[APP_TAG][..]),
        ("settings", &[APP_TAG, SETTINGS_TAG, "game:a"][..]),
    ] {
        let as_path = format!("/salvajocs/{label}");
        let other = (repo.storage.backup_dir(&src, &as_path, tags, label))
            .unwrap()
            .unwrap();
        let before = repo.ids(APP_TAG);

        let err = delete(&mut repo.storage, &[&game.id, &other.id]).unwrap_err();

        assert!(matches!(err, DeleteError::NotAGameBackup(_)), "{label}");
        assert_eq!(repo.ids(APP_TAG), before, "{label}");
    }
}

#[test]
fn delete_in_an_append_only_repository_only_counts() {
    let mut repo = Repo::new();
    let old = repo.backup("old");
    repo.backup("new");
    repo.storage.set_append_only();

    let cleanup = delete(&mut repo.storage, &[&old.id]).unwrap();

    assert_eq!(
        cleanup,
        Cleanup {
            forgotten: 0,
            pruned: false,
            append_only_kept: 1,
        }
    );
    assert_eq!(repo.ids(APP_TAG).len(), 2);
}

/// Every file below `dir`.
fn files_below(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_below(&path));
        } else {
            files.push(path);
        }
    }
    files
}
