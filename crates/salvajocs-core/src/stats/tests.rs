use std::time::{Duration, UNIX_EPOCH};

use super::*;

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

fn snapshot(id: &str, seconds: u64, tags: &[&str], size: Option<u64>) -> SnapshotInfo {
    let tags = tags.iter().map(|&tag| tag.to_owned()).collect();
    SnapshotInfo::new(id.into(), at(seconds), "pc".into(), "Game".into(), tags).with_size(size)
}

const GAME: &[&str] = &["salvajocs", "game:game"];
const SAFETY: &[&str] = &["salvajocs", "game:game", PRE_RESTORE_TAG];

#[test]
fn the_last_backup_is_the_newest_that_is_not_a_safety_backup() {
    let stats = BackupStats::of(&[
        snapshot("c", 3, SAFETY, Some(30)),
        snapshot("b", 2, GAME, Some(20)),
        snapshot("a", 1, GAME, Some(10)),
    ]);
    assert_eq!(stats.last, Some(at(2)));
    assert_eq!(stats.last_size, Some(20));
    assert_eq!(stats.space_used, None);
    assert_eq!(stats.snapshot_ids, ["a", "b", "c"]);
}

#[test]
fn only_safety_backups_give_no_last_backup_but_keep_their_ids() {
    let stats = BackupStats::of(&[
        snapshot("y", 2, SAFETY, Some(20)),
        snapshot("x", 1, SAFETY, Some(10)),
    ]);
    assert_eq!(stats.last, None);
    assert_eq!(stats.last_size, None);
    assert_eq!(stats.snapshot_ids, ["x", "y"]);
}

#[test]
fn a_last_backup_without_a_summary_has_no_size() {
    let stats = BackupStats::of(&[snapshot("a", 1, GAME, None)]);
    assert_eq!(stats.last, Some(at(1)));
    assert_eq!(stats.last_size, None);
}

#[test]
fn by_game_groups_by_the_game_tag_and_ignores_untagged_snapshots() {
    let games = by_game(&[
        snapshot("b2", 4, &["salvajocs", "game:b"], Some(4)),
        snapshot("a2", 3, &["salvajocs", "game:a"], Some(3)),
        snapshot("none", 2, &["salvajocs"], Some(2)),
        snapshot("a1", 1, &["salvajocs", "game:a"], Some(1)),
    ]);
    assert_eq!(games.len(), 2);
    assert_eq!(games["a"].snapshot_ids, ["a1", "a2"]);
    assert_eq!(games["a"].last, Some(at(3)));
    assert_eq!(games["b"].snapshot_ids, ["b2"]);
}

fn stats(last: Option<u64>, size: Option<u64>, space: Option<u64>) -> BackupStats {
    BackupStats {
        last: last.map(at),
        last_size: size,
        space_used: space,
        snapshot_ids: Vec::new(),
    }
}

type Row<'a> = (&'a str, Option<&'a BackupStats>, Option<usize>);

fn sorted(rows: &[Row], by: SortKey, descending: bool) -> Vec<usize> {
    order(rows, ListSort::new(by, descending))
}

#[test]
fn name_sorts_by_bytes_both_ways_and_keeps_equal_titles_in_order() {
    let rows = [
        ("B", None, None),
        ("a", None, None),
        ("b", None, None),
        ("a", None, None),
    ];
    assert_eq!(sorted(&rows, SortKey::Name, false), [0, 1, 3, 2]);
    assert_eq!(sorted(&rows, SortKey::Name, true), [2, 1, 3, 0]);
}

#[test]
fn name_ascending_keeps_a_list_in_byte_order_as_it_is() {
    let rows = [("B", None, None), ("a", None, None), ("b", None, None)];
    assert_eq!(sorted(&rows, SortKey::Name, false), [0, 1, 2]);
}

#[test]
fn each_key_sorts_both_ways_with_missing_values_last() {
    let (small_old, big_new) = (
        stats(Some(1), Some(5), Some(50)),
        stats(Some(2), Some(9), Some(10)),
    );
    let unknown = stats(Some(3), None, None);
    let rows = [
        ("Missing", None, None),
        ("Small old", Some(&small_old), None),
        ("Unknown", Some(&unknown), None),
        ("Big new", Some(&big_new), None),
    ];
    assert_eq!(sorted(&rows, SortKey::LastBackup, true), [2, 3, 1, 0]);
    assert_eq!(sorted(&rows, SortKey::LastBackup, false), [1, 3, 2, 0]);
    assert_eq!(sorted(&rows, SortKey::LastBackupSize, true), [3, 1, 0, 2]);
    assert_eq!(sorted(&rows, SortKey::LastBackupSize, false), [1, 3, 0, 2]);
    assert_eq!(sorted(&rows, SortKey::SpaceUsed, true), [1, 3, 0, 2]);
    assert_eq!(sorted(&rows, SortKey::SpaceUsed, false), [3, 1, 0, 2]);
}

#[test]
fn ties_fall_back_to_the_title_then_the_index() {
    let same = stats(Some(1), Some(5), Some(5));
    let rows = [
        ("b", Some(&same), None),
        ("a", Some(&same), None),
        ("b", Some(&same), None),
    ];
    for descending in [false, true] {
        assert_eq!(sorted(&rows, SortKey::SpaceUsed, descending), [1, 0, 2]);
    }
}

#[test]
fn changes_sorts_by_count_with_missing_values_last_and_ties_by_title() {
    let rows = [
        ("none", None, None),
        ("ten", None, Some(10)),
        ("zero", None, Some(0)),
        ("tie b", None, Some(3)),
        ("tie a", None, Some(3)),
    ];
    assert_eq!(sorted(&rows, SortKey::Changes, true), [1, 4, 3, 2, 0]);
    assert_eq!(sorted(&rows, SortKey::Changes, false), [2, 4, 3, 1, 0]);
}

#[test]
fn natural_directions() {
    assert!(!SortKey::Name.descending_by_default());
    for key in [
        SortKey::LastBackup,
        SortKey::LastBackupSize,
        SortKey::SpaceUsed,
        SortKey::Changes,
    ] {
        assert!(key.descending_by_default());
    }
}
