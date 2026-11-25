use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use super::*;
use crate::changes::Mapped;
use crate::layout::{FileHash, HASHES_FORMAT, PrefixOrigin};
use crate::scan::FoundFile;

/// One file's path key and bytes.
type Files<'a> = &'a [(&'a str, &'a [u8])];

const BASE: Files = &[("Home/g/a.sav", b"base")];
const OURS: Files = &[("Home/g/a.sav", b"ours")];
const THEIRS: Files = &[("Home/g/a.sav", b"theirs")];

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A hash list of `files`, each with mtime `modified`.
fn list_at(files: Files, modified: Option<Duration>) -> Hashes {
    Hashes {
        format: HASHES_FORMAT,
        meta: backup::meta_sha256(&bare_scan()).unwrap(),
        registry: None,
        files: (files.iter())
            .map(|(key, bytes)| {
                let file = FileHash {
                    sha256: sha256(bytes),
                    size: bytes.len() as u64,
                    modified,
                };
                ((*key).to_owned(), file)
            })
            .collect(),
        selection: BTreeMap::new(),
    }
}

fn list(files: Files) -> Hashes {
    list_at(files, None)
}

fn snapshot(id: &str, host: &str, tags: &[&str]) -> SnapshotInfo {
    SnapshotInfo {
        id: id.into(),
        time: at(1),
        host: host.into(),
        label: "Game".into(),
        tags: tags.iter().map(|&tag| tag.to_owned()).collect(),
        size: None,
    }
}

/// The game's snapshot `id` from `host` holding `files`, read with its hash list.
fn backup(id: &str, host: &str, files: Files) -> Baseline {
    let mut baseline = unlisted(id, host, files);
    baseline.hashes = Some(list(files));
    baseline
}

/// As [`backup`], without a hash list (made by 0.5.0 or older).
fn unlisted(id: &str, host: &str, files: Files) -> Baseline {
    let listing = (files.iter())
        .map(|(key, bytes)| SnapshotFile {
            path: format!("files/{key}"),
            size: bytes.len() as u64,
            modified: Some(at(5)),
            kind: EntryKind::File,
        })
        .collect();
    let snapshot = snapshot(id, host, &["salvajocs", "game:game"]);
    Baseline::new(snapshot, listing, None, None)
}

/// The state of the staged `files` against `newest`, `base` and `base_backup`.
fn staged(
    files: Files,
    newest: Option<&Baseline>,
    base: Option<&Base>,
    base_backup: Option<&Baseline>,
) -> State {
    state(&Saves::Staged(&list(files)), newest, base, base_backup)
}

#[test]
fn no_backup_yet_without_a_newest_backup() {
    let base = Base::new("b".into(), Some(list(BASE)));
    assert_eq!(staged(OURS, None, None, None), State::NoBackupYet);
    assert_eq!(staged(OURS, None, Some(&base), None), State::NoBackupYet);
}

#[test]
fn up_to_date_when_the_saves_equal_the_newest_whatever_the_base() {
    let newest = backup("n", "there", OURS);
    let base = Base::new("b".into(), Some(list(BASE)));
    assert_eq!(staged(OURS, Some(&newest), None, None), State::UpToDate);
    assert_eq!(
        staged(OURS, Some(&newest), Some(&base), None),
        State::UpToDate
    );
}

#[test]
fn ahead_when_the_newest_is_the_base() {
    let newest = backup("n", "here", BASE);
    let by_id = Base::new("n".into(), None);
    assert_eq!(
        staged(OURS, Some(&newest), Some(&by_id), None),
        State::Ahead
    );
    // A base whose snapshot is gone still matches the newest by its hash list.
    let by_list = Base::new("forgotten".into(), Some(list(BASE)));
    assert_eq!(
        staged(OURS, Some(&newest), Some(&by_list), None),
        State::Ahead
    );
}

#[test]
fn behind_when_the_saves_are_still_the_base_even_once_it_is_forgotten() {
    let newest = backup("n", "there", THEIRS);
    let base = Base::new("forgotten".into(), Some(list(BASE)));
    assert_eq!(
        staged(BASE, Some(&newest), Some(&base), None),
        State::Behind
    );
}

#[test]
fn a_base_without_a_list_compares_through_its_backup() {
    let newest = backup("n", "there", THEIRS);
    let base = Base::new("b".into(), None);
    let base_backup = backup("b", "here", BASE);
    assert_eq!(
        staged(BASE, Some(&newest), Some(&base), Some(&base_backup)),
        State::Behind
    );
    assert_eq!(
        staged(OURS, Some(&newest), Some(&base), Some(&base_backup)),
        State::ChangedOnBoth
    );
    // Its snapshot forgotten, nothing tells: asked about as changed on both.
    assert_eq!(
        staged(BASE, Some(&newest), Some(&base), None),
        State::ChangedOnBoth
    );
    // Another snapshot is not the base's.
    let other = backup("other", "here", BASE);
    assert_eq!(
        staged(BASE, Some(&newest), Some(&base), Some(&other)),
        State::ChangedOnBoth
    );
}

#[test]
fn a_base_with_a_list_ignores_its_backup() {
    let newest = backup("n", "there", THEIRS);
    let base = Base::new("b".into(), Some(list(BASE)));
    let base_backup = backup("b", "here", OURS);
    assert_eq!(
        staged(BASE, Some(&newest), Some(&base), Some(&base_backup)),
        State::Behind
    );
}

/// The other build's snapshot `id` holding `files` at `WinAppData/<key>`, mapped back to `files`
/// with the path keys of `unplaced` left without a place.
fn mapped(id: &str, files: Files, unplaced: &[&str]) -> Baseline {
    let own: Vec<(String, &[u8])> = (files.iter())
        .map(|(key, bytes)| (format!("WinAppData/{key}"), *bytes))
        .collect();
    let own: Vec<(&str, &[u8])> = own
        .iter()
        .map(|(key, bytes)| (key.as_str(), *bytes))
        .collect();
    let mut baseline = backup(id, "there", &own);
    baseline.mapped = Some(Mapped::new(
        unlisted(id, "there", files).listing,
        Some(list(files)),
        BTreeMap::new(),
        Vec::new(),
        unplaced
            .iter()
            .map(|&key| key.to_owned())
            .collect::<BTreeSet<_>>(),
        Vec::new(),
    ));
    baseline
}

#[test]
fn a_newest_of_the_other_build_is_the_base_by_its_mapped_list() {
    let base = Base::new("b".into(), Some(list(BASE)));
    let newest = mapped("n", BASE, &[]);
    assert_eq!(staged(OURS, Some(&newest), Some(&base), None), State::Ahead);

    // A file left unplaced: the newest is not the base.
    let newest = mapped("n", BASE, &["WinAppData/g/b.sav"]);
    assert_eq!(
        staged(BASE, Some(&newest), Some(&base), None),
        State::Behind
    );
    assert_eq!(
        staged(OURS, Some(&newest), Some(&base), None),
        State::ChangedOnBoth
    );
}

#[test]
fn a_seed_that_is_the_newest_is_an_unknown_base() {
    // Every Steam Deck is `steamdeck`: the newest may be another one's.
    let snapshots = [snapshot("n", "steamdeck", &["salvajocs", "game:game"])];
    let bases = Bases::default();
    let seed = bases.base("repo", "game", &snapshots, "steamdeck").unwrap();
    let newest = backup("n", "steamdeck", BASE);
    assert_eq!(
        staged(OURS, Some(&newest), Some(&seed), None),
        State::UnknownBase
    );
    assert_eq!(
        staged(BASE, Some(&newest), Some(&seed), None),
        State::UpToDate
    );
    // Once recorded, the same base is ahead.
    let recorded = Base::new("n".into(), None);
    assert_eq!(
        staged(OURS, Some(&newest), Some(&recorded), None),
        State::Ahead
    );
}

#[test]
fn changed_on_both_when_the_saves_the_newest_and_the_base_all_differ() {
    let newest = backup("n", "there", THEIRS);
    let base = Base::new("b".into(), Some(list(BASE)));
    assert_eq!(
        staged(OURS, Some(&newest), Some(&base), None),
        State::ChangedOnBoth
    );
}

#[test]
fn unknown_base_when_the_saves_differ_and_there_is_no_base() {
    let newest = backup("n", "there", THEIRS);
    assert_eq!(staged(OURS, Some(&newest), None, None), State::UnknownBase);
}

#[test]
fn a_newest_without_a_hash_list_is_never_equal() {
    let newest = unlisted("n", "here", OURS);
    let base = Base::new("n".into(), None);
    assert_eq!(staged(OURS, Some(&newest), Some(&base), None), State::Ahead);
    assert_eq!(staged(OURS, Some(&newest), None, None), State::UnknownBase);
}

/// Writes `bytes` to `path` with mtime `modified`.
fn write(path: &Path, bytes: &[u8], modified: SystemTime) {
    fs::write(path, bytes).unwrap();
    (File::options().write(true).open(path).unwrap())
        .set_modified(modified)
        .unwrap();
}

/// A scan of the game without files, whose `salvajocs.json` every hash list here hashes.
fn bare_scan() -> GameScan {
    GameScan {
        title: "Game".into(),
        files: Vec::new(),
        roots: Vec::new(),
        registry_keys: Vec::new(),
        registry_source: None,
        prefixes: Vec::new(),
        prefix_origin: None,
        unreadable: Vec::new(),
    }
}

/// A scan of the game with the file `Home/g/a.sav` at `real`.
fn scan(real: &Path) -> GameScan {
    let meta = fs::metadata(real).unwrap();
    let mut game = bare_scan();
    game.files = vec![FoundFile {
        base: crate::layout::Base::Home,
        tail: "g/a.sav".into(),
        real: real.to_path_buf(),
        size: meta.len(),
        modified: meta.modified().ok(),
    }];
    game
}

/// The state of the scan `game` against `newest`, `base` and `base_backup`, as the preview
/// asks.
fn scanned(
    game: &GameScan,
    newest: &Baseline,
    base: Option<&Base>,
    base_backup: Option<&Baseline>,
) -> State {
    let settings = GameSettings::default();
    let (listing, hashes) = newest.compared();
    let compared = changes::compare_game(game, listing, hashes);
    let saves = Saves::Scanned {
        game,
        settings: &settings,
        compared: &compared,
    };
    state(&saves, Some(newest), base, base_backup)
}

#[test]
fn a_scan_reads_the_states_by_the_bases_list() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.sav");
    write(&path, b"base", at(1));
    let game = scan(&path);
    let newest = backup("n", "there", THEIRS);
    let base = Base::new(
        "b".into(),
        Some(list_at(BASE, Some(Duration::from_secs(1)))),
    );
    assert_eq!(scanned(&game, &newest, Some(&base), None), State::Behind);
    assert_eq!(scanned(&game, &newest, None, None), State::UnknownBase);

    // The same size as the base: the hash tells.
    write(&path, b"ours", at(2));
    let game = scan(&path);
    assert_eq!(
        scanned(&game, &newest, Some(&base), None),
        State::ChangedOnBoth
    );

    write(&path, b"theirs", at(2));
    let game = scan(&path);
    assert_eq!(scanned(&game, &newest, Some(&base), None), State::UpToDate);
}

#[test]
fn a_scan_reads_the_base_only_when_the_registry_and_meta_cannot_differ() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.sav");
    write(&path, b"base", at(1));
    let game = scan(&path);
    let newest = backup("n", "there", THEIRS);
    let mut files = list_at(BASE, Some(Duration::from_secs(1)));
    files.registry = Some(sha256(b"registry"));
    let base = Base::new("b".into(), Some(files));
    assert_eq!(
        scanned(&game, &newest, Some(&base), None),
        State::ChangedOnBoth
    );
    assert_eq!(
        staged(BASE, Some(&newest), Some(&base), None),
        State::ChangedOnBoth
    );

    // The same through a base snapshot without a list.
    let unlisted_base = Base::new("b".into(), None);
    let mut base_backup = backup("b", "here", BASE);
    (base_backup.hashes.as_mut().unwrap()).registry = Some(sha256(b"registry"));
    assert_eq!(
        scanned(&game, &newest, Some(&unlisted_base), Some(&base_backup)),
        State::ChangedOnBoth
    );
    assert_eq!(
        staged(
            BASE,
            Some(&newest),
            Some(&unlisted_base),
            Some(&base_backup)
        ),
        State::ChangedOnBoth
    );

    // Another prefix stages another `salvajocs.json`.
    let mut game = game;
    game.prefix_origin = Some(PrefixOrigin::Proton(70));
    let base = Base::new(
        "b".into(),
        Some(list_at(BASE, Some(Duration::from_secs(1)))),
    );
    assert_eq!(
        scanned(&game, &newest, Some(&base), None),
        State::ChangedOnBoth
    );
}

#[test]
fn the_reported_case_reads_behind_through_the_seed() {
    // A backed up A1; JOAN-XPS made J1, now the newest; nothing changed on A since A1.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.sav");
    write(&path, b"base", at(1));
    let game = scan(&path);
    let snapshots = [
        snapshot("j1", "JOAN-XPS-9730", &["salvajocs", "game:game"]),
        snapshot("a1", "a", &["salvajocs", "game:game"]),
    ];
    let bases = Bases::default();
    let base = bases.base("repo", "game", &snapshots, "a").unwrap();
    let newest = backup("j1", "JOAN-XPS-9730", THEIRS);
    assert_eq!(
        base.to_read(&Saves::Staged(&list(BASE)), &newest),
        Some("a1")
    );
    let a1 = backup("a1", "a", BASE);
    assert_eq!(
        scanned(&game, &newest, Some(&base), Some(&a1)),
        State::Behind
    );
    assert_eq!(
        staged(BASE, Some(&newest), Some(&base), Some(&a1)),
        State::Behind
    );
}

#[test]
fn to_read_names_a_base_without_a_list_that_is_not_the_newest_when_the_saves_differ() {
    let newest = backup("n", "there", THEIRS);
    let ours = list(OURS);
    let ours = Saves::Staged(&ours);
    assert_eq!(
        Base::new("b".into(), None).to_read(&ours, &newest),
        Some("b")
    );
    assert_eq!(Base::new("n".into(), None).to_read(&ours, &newest), None);
    assert_eq!(
        Base::new("b".into(), Some(list(BASE))).to_read(&ours, &newest),
        None
    );
    // Up to date: the state does not depend on the base.
    let theirs = list(THEIRS);
    let theirs = Saves::Staged(&theirs);
    assert_eq!(Base::new("b".into(), None).to_read(&theirs, &newest), None);
}

#[test]
fn the_seed_is_this_pcs_newest_regular_backup_of_the_game() {
    let snapshots = [
        snapshot("theirs", "there", &["salvajocs", "game:game"]),
        snapshot("safety", "here", &["salvajocs", "game:game", "pre-restore"]),
        snapshot("other-game", "here", &["salvajocs", "game:other"]),
        snapshot("mine", "here", &["salvajocs", "game:game"]),
        snapshot("older", "here", &["salvajocs", "game:game"]),
    ];
    let bases = Bases::default();
    let seed = bases.base("repo", "game", &snapshots, "here").unwrap();
    assert!(matches!(seed, Cow::Owned(_)));
    assert_eq!(
        (seed.snapshot.as_str(), &seed.saves, seed.seed),
        ("mine", &None, true)
    );
    assert_eq!(bases.base("repo", "game", &snapshots, "new-pc"), None);
    assert_eq!(bases.get("repo", "game"), None);
}

#[test]
fn a_record_wins_over_the_seed_in_its_repository_only() {
    let dir = tempfile::tempdir().unwrap();
    let recorded = Base::new("restored".into(), None);
    record(dir.path(), "repo", [("game".to_owned(), recorded.clone())]).unwrap();
    let snapshots = [snapshot("mine", "here", &["salvajocs", "game:game"])];
    let bases = load(dir.path());
    let base = bases.base("repo", "game", &snapshots, "here").unwrap();
    assert!(matches!(base, Cow::Borrowed(_)));
    assert_eq!(*base, recorded);
    let other = bases
        .base("other-repo", "game", &snapshots, "here")
        .unwrap();
    assert_eq!(other.snapshot, "mine");
}

#[test]
fn a_missing_file_reads_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(load(dir.path()), Bases::default());
    assert_eq!(load(&dir.path().join("no-such-dir")), Bases::default());
}

#[test]
fn a_corrupt_or_newer_file_reads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(BASE_FILE);
    for bytes in [&b"{ not json"[..], br#"{"format": 2, "repositories": {}}"#] {
        fs::write(&path, bytes).unwrap();
        assert_eq!(load(dir.path()), Bases::default());
    }
}

#[test]
fn a_record_replaces_a_corrupt_file() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(BASE_FILE), b"{ not json").unwrap();
    let base = Base::new("b".into(), None);
    record(dir.path(), "repo", [("game".to_owned(), base.clone())]).unwrap();
    assert_eq!(load(dir.path()).get("repo", "game"), Some(&base));
}

#[test]
fn a_record_leaves_a_newer_file_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(BASE_FILE);
    let newer = br#"{"format": 2, "bases": {"repo": {"other": "kept"}}}"#;
    fs::write(&path, newer).unwrap();
    let base = Base::new("b".into(), None);
    let err = record(dir.path(), "repo", [("game".to_owned(), base)]).unwrap_err();
    assert!(
        matches!(err, SyncError::NewerFormat { format: 2, .. }),
        "{err:?}"
    );
    assert_eq!(fs::read(&path).unwrap(), newer);
}

#[cfg(unix)]
#[test]
fn a_record_leaves_an_unreadable_file_as_it_was() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(BASE_FILE);
    let kept = Base::new("kept".into(), None);
    record(dir.path(), "repo", [("other".to_owned(), kept)]).unwrap();
    let bytes = fs::read(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    // Root reads it anyway, so there is nothing to test.
    if fs::read(&path).is_ok() {
        return;
    }
    let base = Base::new("b".into(), None);
    let result = record(dir.path(), "repo", [("game".to_owned(), base)]);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(result, Err(SyncError::Read { .. })), "{result:?}");
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn record_keeps_every_other_record() {
    let dir = tempfile::tempdir().unwrap();
    let staged = Base::new("s".into(), Some(list_at(OURS, Some(Duration::new(7, 9)))));
    let restored = Base::new("r".into(), None);
    record(dir.path(), "repo", [("one".to_owned(), staged.clone())]).unwrap();
    record(dir.path(), "repo", [("two".to_owned(), restored.clone())]).unwrap();
    record(dir.path(), "other", [("one".to_owned(), restored.clone())]).unwrap();
    let bases = load(dir.path());
    assert_eq!(bases.get("repo", "one"), Some(&staged));
    assert_eq!(bases.get("repo", "two"), Some(&restored));
    assert_eq!(bases.get("other", "one"), Some(&restored));
    assert_eq!(bases.get("other", "two"), None);
    // A later record of a game replaces its base.
    record(dir.path(), "repo", [("one".to_owned(), restored.clone())]).unwrap();
    assert_eq!(load(dir.path()).get("repo", "one"), Some(&restored));
}

#[test]
fn record_writes_through_a_temp_file_and_creates_the_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("config");
    record(
        &dir,
        "repo",
        [("game".to_owned(), Base::new("b".into(), None))],
    )
    .unwrap();
    let names = |dir: &Path| -> Vec<String> {
        (fs::read_dir(dir).unwrap())
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect()
    };
    assert_eq!(names(&dir), [BASE_FILE]);

    // A path that cannot be read fails the record and leaves no temp file behind.
    let blocked = tmp.path().join("blocked");
    fs::create_dir_all(blocked.join(BASE_FILE)).unwrap();
    let err = record(&blocked, "repo", []).unwrap_err();
    assert!(
        matches!(&err, SyncError::Read { path, .. } if *path == blocked.join(BASE_FILE)),
        "{err:?}"
    );
    assert_eq!(names(&blocked), [BASE_FILE]);
}

#[cfg(unix)]
#[test]
fn a_failed_write_leaves_no_temp_file() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
    // Root writes anyway, so there is nothing to test.
    if File::create(dir.join("probe")).is_ok() {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let base = Base::new("b".into(), None);
    let result = record(dir, "repo", [("game".to_owned(), base)]);
    let left = fs::read_dir(dir).unwrap().count();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(result, Err(SyncError::Write { .. })), "{result:?}");
    assert_eq!(left, 0);
}

#[test]
fn a_scan_reads_up_to_date_with_a_newest_whose_registry_differs() {
    // The known gap: a scan does not read the registry, so Back up decides otherwise.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.sav");
    write(&path, b"base", at(1));
    let game = scan(&path);
    let mut newest = backup("n", "there", BASE);
    (newest.hashes.as_mut().unwrap()).registry = Some(sha256(b"registry"));
    assert_eq!(scanned(&game, &newest, None, None), State::UpToDate);
    assert_eq!(staged(BASE, Some(&newest), None, None), State::UnknownBase);
}

#[test]
fn a_record_skips_a_seed() {
    let dir = tempfile::tempdir().unwrap();
    let snapshots = [snapshot("mine", "here", &["salvajocs", "game:game"])];
    let seed = (Bases::default().base("repo", "game", &snapshots, "here"))
        .unwrap()
        .into_owned();
    let recorded = Base::new("b".into(), None);
    let updates = [
        ("game".to_owned(), seed),
        ("other".to_owned(), recorded.clone()),
    ];
    record(dir.path(), "repo", updates).unwrap();
    let bases = load(dir.path());
    assert_eq!(bases.get("repo", "game"), None);
    assert_eq!(bases.get("repo", "other"), Some(&recorded));
}
