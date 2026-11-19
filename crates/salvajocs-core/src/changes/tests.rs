use std::collections::BTreeMap;
use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::*;
use crate::layout::{Base, FileHash, HASHES_FILE, HASHES_FORMAT, META_FILE, REGISTRY_FILE};
use crate::scan::FoundFile;
use crate::storage::EntryKind;

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// Writes `bytes` to `path` with mtime `modified`.
fn write(path: &Path, bytes: &[u8], modified: SystemTime) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    path.to_path_buf()
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A listed file of `size` bytes.
fn entry(path: &str, size: usize, modified: SystemTime) -> SnapshotFile {
    SnapshotFile {
        path: path.to_owned(),
        size: size as u64,
        modified: Some(modified),
        kind: EntryKind::File,
    }
}

/// A hash list with each `(path below files/, bytes)`.
fn hashes(files: &[(&str, &[u8])]) -> Hashes {
    Hashes {
        format: HASHES_FORMAT,
        meta: sha256(b"{}"),
        registry: None,
        files: files
            .iter()
            .map(|(path, bytes)| {
                let file = FileHash {
                    sha256: sha256(bytes),
                    size: bytes.len() as u64,
                    modified: None,
                };
                ((*path).to_owned(), file)
            })
            .collect::<BTreeMap<_, _>>(),
    }
}

fn found(base: Base, tail: &str, real: &Path) -> FoundFile {
    let meta = fs::metadata(real).unwrap();
    FoundFile {
        base,
        tail: tail.to_owned(),
        real: real.to_path_buf(),
        size: meta.len(),
        modified: meta.modified().ok(),
    }
}

fn game(files: Vec<FoundFile>) -> GameScan {
    GameScan {
        title: "Game".into(),
        files,
        roots: Vec::new(),
        registry_keys: Vec::new(),
        registry_source: None,
        prefixes: Vec::new(),
        unreadable: Vec::new(),
    }
}

/// Compares `path` with an entry of `listed` bytes, stored at mtime `stored`, whose hash is
/// that of `listed` when `hashed`.
fn compare(path: &Path, listed: &[u8], stored: SystemTime, hashed: bool) -> FileState {
    let modified = fs::metadata(path).unwrap().modified().ok();
    let sha256 = hashed.then(|| sha256(listed));
    let entry = entry("a", listed.len(), stored);
    compare_file(path, modified, &entry, sha256.as_deref())
}

#[test]
fn a_different_size_is_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write(&tmp.path().join("a"), b"abcd", at(1));
    assert_eq!(compare(&path, b"abc", at(1), true), FileState::Changed);
}

#[test]
fn the_same_size_and_mtime_is_the_same_without_reading() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write(&tmp.path().join("a"), b"abc", at(1));
    // Another hash in the list: only reading the file would notice.
    assert_eq!(compare(&path, b"xyz", at(1), true), FileState::Same);
}

#[test]
fn the_same_bytes_with_another_mtime_are_the_same_by_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write(&tmp.path().join("a"), b"abc", at(2));
    assert_eq!(compare(&path, b"abc", at(1), true), FileState::Same);
}

#[test]
fn other_bytes_of_the_same_size_are_changed_by_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write(&tmp.path().join("a"), b"abd", at(2));
    assert_eq!(compare(&path, b"abc", at(1), true), FileState::Changed);
}

#[test]
fn without_a_hash_another_mtime_is_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write(&tmp.path().join("a"), b"abc", at(2));
    assert_eq!(compare(&path, b"abc", at(1), false), FileState::Changed);
    assert_eq!(compare(&path, b"abc", at(2), false), FileState::Same);
}

#[test]
fn a_missing_local_file_is_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("missing");
    let state = compare_file(&missing, None, &entry("a", 3, at(1)), None);
    assert!(matches!(state, FileState::Unknown(_)));
}

#[test]
fn files_compare_by_path_key_and_unmatched_entries_are_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let same = write(&tmp.path().join("same"), b"same", at(2));
    let changed = write(&tmp.path().join("changed"), b"new!", at(2));
    let new = write(&tmp.path().join("new"), b"new", at(1));
    let scan = game(vec![
        found(Base::Home, "same.sav", &same),
        found(Base::Home, "changed.sav", &changed),
        found(Base::Home, "new.sav", &new),
    ]);
    let baseline = [
        entry("files/Home/same.sav", 4, at(1)),
        entry("files/Home/changed.sav", 4, at(1)),
        entry("files/Home/gone.sav", 4, at(1)),
        entry("salvajocs.json", 2, at(1)),
    ];
    let list = hashes(&[
        ("Home/same.sav", b"same"),
        ("Home/changed.sav", b"old!"),
        ("Home/gone.sav", b"gone"),
    ]);
    let comparison = compare_game(&scan, &baseline, Some(&list));
    assert_eq!(
        comparison.found,
        [FileState::Same, FileState::Changed, FileState::New]
    );
    let gone: Vec<_> = comparison.gone.iter().map(|file| &file.path).collect();
    assert_eq!(gone, ["Home/gone.sav"]);
    let counts = counts(&scan, &comparison, &GameSettings::default()).unwrap();
    assert_eq!(
        counts,
        Counts {
            new: 1,
            changed: 1,
            removed: 1,
            unknown: 0
        }
    );

    // A snapshot without a hash list: every file with another mtime is changed.
    let comparison = compare_game(&scan, &baseline, None);
    assert_eq!(
        comparison.found,
        [FileState::Changed, FileState::Changed, FileState::New]
    );
}

#[test]
fn case_variants_match_below_windows_bases_only() {
    let tmp = tempfile::tempdir().unwrap();
    let save = write(&tmp.path().join("save"), b"save", at(2));
    let scan = game(vec![
        found(Base::WinAppData, "Foo/save.sav", &save),
        found(Base::XdgConfig, "Foo/save.sav", &save),
    ]);
    let baseline = [
        entry("files/WinAppData/foo/save.sav", 4, at(1)),
        entry("files/XdgConfig/foo/save.sav", 4, at(1)),
    ];
    // The hash is looked up by the entry's own path, not the found file's case.
    let list = hashes(&[
        ("WinAppData/foo/save.sav", b"save"),
        ("XdgConfig/foo/save.sav", b"save"),
    ]);
    let comparison = compare_game(&scan, &baseline, Some(&list));
    assert_eq!(comparison.found, [FileState::Same, FileState::New]);
    let gone: Vec<_> = comparison.gone.iter().map(|file| &file.path).collect();
    assert_eq!(gone, ["XdgConfig/foo/save.sav"]);
}

#[test]
fn an_exact_match_wins_over_a_case_variant() {
    let tmp = tempfile::tempdir().unwrap();
    let save = write(&tmp.path().join("save"), b"save", at(2));
    let scan = game(vec![found(Base::WinAppData, "foo/save.sav", &save)]);
    let baseline = [
        entry("files/WinAppData/FOO/save.sav", 4, at(1)),
        entry("files/WinAppData/foo/save.sav", 4, at(1)),
    ];
    let list = hashes(&[
        ("WinAppData/FOO/save.sav", b"SAVE"),
        ("WinAppData/foo/save.sav", b"save"),
    ]);
    let comparison = compare_game(&scan, &baseline, Some(&list));
    assert_eq!(comparison.found, [FileState::Same]);
    assert_eq!(comparison.gone.len(), 1);
}

#[test]
fn an_unticked_file_in_the_snapshot_is_removed_and_a_new_one_is_not_counted() {
    let tmp = tempfile::tempdir().unwrap();
    let kept = write(&tmp.path().join("kept"), b"kept", at(1));
    let new = write(&tmp.path().join("new"), b"new", at(1));
    let scan = game(vec![
        found(Base::Home, "kept.sav", &kept),
        found(Base::Home, "new.sav", &new),
    ]);
    let baseline = [entry("files/Home/kept.sav", 4, at(1))];
    let comparison = compare_game(&scan, &baseline, None);
    let mut settings = GameSettings::default();
    settings.set_path("Home/kept.sav", false);
    settings.set_path("Home/new.sav", false);
    // Nothing is left to back up, so no snapshot would be made.
    assert_eq!(counts(&scan, &comparison, &settings), None);

    let other = write(&tmp.path().join("other"), b"other", at(1));
    let mut scan = scan;
    scan.files.push(found(Base::Home, "other.sav", &other));
    let comparison = compare_game(&scan, &baseline, None);
    assert_eq!(
        counts(&scan, &comparison, &settings),
        Some(Counts {
            new: 1,
            changed: 0,
            removed: 1,
            unknown: 0
        })
    );
}

#[test]
fn a_game_with_unreadable_paths_removes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let save = write(&tmp.path().join("save"), b"save", at(1));
    let mut scan = game(vec![found(Base::Home, "save.sav", &save)]);
    scan.unreadable.push(tmp.path().join("locked"));
    let baseline = [
        entry("files/Home/save.sav", 4, at(1)),
        entry("files/Home/gone.sav", 4, at(1)),
    ];
    let comparison = compare_game(&scan, &baseline, None);
    let mut settings = GameSettings::default();
    assert_eq!(counts(&scan, &comparison, &settings).unwrap().removed, 0);
    settings.skip = true;
    assert_eq!(counts(&scan, &comparison, &settings), None);
}

fn snapshot(id: &str, host: &str, tags: &[&str]) -> SnapshotInfo {
    SnapshotInfo {
        id: id.into(),
        time: at(1),
        host: host.into(),
        label: "Game".into(),
        tags: tags.iter().map(|&tag| tag.to_owned()).collect(),
    }
}

#[test]
fn newest_skips_pre_restore_and_other_games_from_any_host() {
    let snapshots = [
        snapshot("other-game", "here", &["salvajocs", "game:other"]),
        snapshot("safety", "here", &["salvajocs", "game:game", "pre-restore"]),
        snapshot("elsewhere", "there", &["salvajocs", "game:game"]),
        snapshot("older", "here", &["salvajocs", "game:game"]),
    ];
    assert_eq!(newest(&snapshots, "game").unwrap().id, "elsewhere");
    assert_eq!(newest(&snapshots, "none"), None);
}

#[test]
fn an_unreadable_included_file_counts_as_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let save = write(&tmp.path().join("save"), b"save", at(2));
    let scan = game(vec![found(Base::Home, "save.sav", &save)]);
    let comparison = Comparison {
        found: vec![FileState::Unknown("denied".into())],
        gone: Vec::new(),
    };
    assert_eq!(
        counts(&scan, &comparison, &GameSettings::default()),
        Some(Counts {
            unknown: 1,
            ..Counts::default()
        })
    );
}

/// A listing with `files/Home/a.sav` (3 bytes at `at(1)`), `salvajocs.json` at `meta`, and
/// `hashes.json` at `written`, with the hash list that matches it.
fn listed(meta: u64, written: u64) -> (Vec<SnapshotFile>, Hashes) {
    let listing = vec![
        entry("files/Home/a.sav", 3, at(1)),
        entry(META_FILE, 2, at(meta)),
        entry(HASHES_FILE, 100, at(written)),
    ];
    let mut hashes = hashes(&[("Home/a.sav", b"abc")]);
    hashes.files.get_mut("Home/a.sav").unwrap().modified = Some(Duration::from_secs(1));
    (listing, hashes)
}

#[test]
fn a_hash_list_is_trusted_only_when_it_describes_the_snapshot() {
    let (listing, list) = listed(5, 5);
    assert_eq!(verified(list.clone(), &listing), Some(list.clone()));

    let mut other_mtime = list.clone();
    other_mtime.files.get_mut("Home/a.sav").unwrap().modified = Some(Duration::from_secs(2));
    let mut other_size = list.clone();
    other_size.files.get_mut("Home/a.sav").unwrap().size = 4;
    let mut no_mtime = list.clone();
    no_mtime.files.get_mut("Home/a.sav").unwrap().modified = None;
    let mut extra = list.clone();
    extra
        .files
        .insert("Home/b.sav".into(), list.files["Home/a.sav"].clone());
    let mut missing = list.clone();
    missing.files.clear();
    let mut registry = list.clone();
    registry.registry = Some("hash".into());
    for stale in [other_mtime, other_size, no_mtime, extra, missing, registry] {
        assert_eq!(verified(stale.clone(), &listing), None, "{stale:?}");
    }

    // `salvajocs.json` rewritten after the list, as an older version does.
    let (newer_meta, list) = listed(6, 5);
    assert_eq!(verified(list.clone(), &newer_meta), None);
    // A `registry.json` the list does not hash.
    let (mut listing, list) = listed(5, 5);
    listing.push(entry(REGISTRY_FILE, 2, at(5)));
    assert_eq!(verified(list, &listing), None);
}
