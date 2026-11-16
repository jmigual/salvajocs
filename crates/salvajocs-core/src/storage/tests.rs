use std::fs;

use super::*;

const TAGS: &[&str] = &["salvajocs", "game:test-slug"];
const AS_PATH: &str = "/salvajocs/test-slug";

fn local(dir: &Path) -> RepoLocation {
    RepoLocation {
        repository: dir.to_str().unwrap().to_string(),
        options: BTreeMap::new(),
    }
}

fn save_dir(root: &Path) -> PathBuf {
    let src = root.join("save");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("a.sav"), "alpha").unwrap();
    fs::write(src.join("sub/b.sav"), "beta").unwrap();
    src
}

fn init_local(root: &Path, password: &str, compression: Compression) -> Storage {
    Storage::init(&local(&root.join("repo")), password, compression).unwrap()
}

#[test]
fn init_and_open_with_empty_password() {
    let tmp = tempfile::tempdir().unwrap();
    let location = local(&tmp.path().join("repo"));
    assert_eq!(probe(&location, ""), RepoState::Missing);
    Storage::init(&location, "", Compression::Default).unwrap();
    assert_eq!(probe(&location, ""), RepoState::Ready);
    Storage::open(&location, "").unwrap();
}

#[test]
fn wrong_password_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let location = local(&tmp.path().join("repo"));
    Storage::init(&location, "secret", Compression::Default).unwrap();
    assert_eq!(probe(&location, "other"), RepoState::WrongPassword);
    assert_eq!(probe(&location, ""), RepoState::WrongPassword);
    assert!(matches!(
        Storage::open(&location, "other"),
        Err(StorageError::WrongPassword)
    ));
}

#[test]
fn missing_repo_is_missing_not_error() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(
        probe(&local(&tmp.path().join("nothing-here")), ""),
        RepoState::Missing
    );
}

#[test]
fn unreadable_repo_is_error_not_missing() {
    let location = RepoLocation {
        repository: "nosuchbackend:somewhere".to_string(),
        options: BTreeMap::new(),
    };
    assert!(matches!(probe(&location, ""), RepoState::Error(_)));
}

#[cfg(unix)]
#[test]
fn unreadable_local_repo_is_error_not_missing() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let repo_dir = tmp.path().join("repo");
    let location = local(&repo_dir);
    Storage::init(&location, "", Compression::Default).unwrap();
    fs::set_permissions(&repo_dir, fs::Permissions::from_mode(0o000)).unwrap();
    // Root ignores permission bits, so there is nothing to observe.
    let enforced = fs::exists(repo_dir.join("config")).is_err();
    let state = probe(&location, "");
    fs::set_permissions(&repo_dir, fs::Permissions::from_mode(0o755)).unwrap();
    if enforced {
        assert!(matches!(state, RepoState::Error(_)));
    }
}

#[test]
fn entry_paths_must_stay_inside_the_destination() {
    for ok in ["", "a.sav", "sub/b.sav"] {
        assert!(check_entry_path(Path::new(ok)).is_ok());
    }
    for bad in ["..", "../x", "sub/../../x", "/etc/passwd"] {
        assert!(matches!(
            check_entry_path(Path::new(bad)),
            Err(StorageError::UnsafeEntryPath(_))
        ));
    }
}

#[test]
fn compression_set_at_init_and_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_local(tmp.path(), "", Compression::Level(19));
    assert_eq!(storage.compression(), Compression::Level(19));

    storage.set_compression(Compression::Off).unwrap();
    assert_eq!(storage.compression(), Compression::Off);
    storage.set_compression(Compression::Level(-3)).unwrap();
    assert_eq!(storage.compression(), Compression::Level(-3));
    storage.set_compression(Compression::Default).unwrap();
    assert_eq!(storage.compression(), Compression::Level(3));

    // The setting is stored in the repository, so a new session sees it.
    let reopened = Storage::open(&local(&tmp.path().join("repo")), "").unwrap();
    assert_eq!(reopened.compression(), Compression::Level(3));
}

#[test]
fn compression_level_out_of_range_rejected() {
    for field in [Some(23), Some(-8), Some(i32::MAX)] {
        assert!(matches!(
            Compression::from_field(field),
            Err(StorageError::InvalidCompression(_))
        ));
    }
    assert_eq!(Compression::from_field(None).unwrap(), Compression::Default);
    assert_eq!(Compression::from_field(Some(0)).unwrap(), Compression::Off);
    assert_eq!(
        Compression::from_field(Some(22)).unwrap(),
        Compression::Level(22)
    );
    assert_eq!(
        Compression::from_field(Some(-7)).unwrap(),
        Compression::Level(-7)
    );

    let tmp = tempfile::tempdir().unwrap();
    let location = local(&tmp.path().join("repo"));
    assert!(matches!(
        Storage::init(&location, "", Compression::Level(0)),
        Err(StorageError::InvalidCompression(0))
    ));
    assert_eq!(probe(&location, ""), RepoState::Missing);
}

#[test]
fn backup_with_as_path_then_restore_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);

    let info = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .expect("first backup is written");
    assert_eq!(
        storage.snapshots("game:test-slug").unwrap(),
        vec![info.clone()]
    );

    let out = tmp.path().join("out");
    storage.restore_path(&info.id, AS_PATH, &out).unwrap();
    assert_eq!(fs::read_to_string(out.join("a.sav")).unwrap(), "alpha");
    assert_eq!(fs::read_to_string(out.join("sub/b.sav")).unwrap(), "beta");
}

#[test]
fn second_unchanged_backup_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let location = local(&tmp.path().join("repo"));
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    assert!(
        storage
            .backup_dir(&src, AS_PATH, TAGS, "Test")
            .unwrap()
            .is_some()
    );
    drop(storage);

    // A new app session: reopen and reindex before the next run.
    let mut storage = Storage::open(&location, "").unwrap();
    storage.reindex().unwrap();
    assert!(
        storage
            .backup_dir(&src, AS_PATH, TAGS, "Test")
            .unwrap()
            .is_none()
    );
    assert_eq!(storage.snapshots("salvajocs").unwrap().len(), 1);

    fs::write(src.join("a.sav"), "changed").unwrap();
    storage.reindex().unwrap();
    assert!(
        storage
            .backup_dir(&src, AS_PATH, TAGS, "Test")
            .unwrap()
            .is_some()
    );
    assert_eq!(storage.snapshots("salvajocs").unwrap().len(), 2);
}

#[test]
fn label_keeps_commas_and_unicode() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let label = "Title, with comma: ünï";
    storage.backup_dir(&src, AS_PATH, TAGS, label).unwrap();

    let snapshots = storage.snapshots("game:test-slug").unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].label, label);
    assert!(snapshots[0].tags.iter().any(|tag| tag == "salvajocs"));
    assert!(storage.snapshots("game:other").unwrap().is_empty());
}

#[test]
fn opendal_memory_backend_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let location = RepoLocation {
        repository: "opendal:memory".to_string(),
        options: BTreeMap::new(),
    };
    let mut storage = Storage::init(&location, "", Compression::Default).unwrap();
    let info = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();
    let out = tmp.path().join("out");
    storage.restore_path(&info.id, AS_PATH, &out).unwrap();
    assert_eq!(fs::read_to_string(out.join("a.sav")).unwrap(), "alpha");
}

#[test]
fn snapshot_path_is_the_same_on_every_os() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let info = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();

    // A single file is addressed under the forward-slash path, whatever the host OS.
    let out = tmp.path().join("out");
    storage
        .restore_path(&info.id, "/salvajocs/test-slug/sub/b.sav", &out)
        .unwrap();
    assert_eq!(fs::read_to_string(out.join("b.sav")).unwrap(), "beta");
    // The source's real location is not what the snapshot stores.
    let real = format!("{}/sub/b.sav", src.to_str().unwrap().replace('\\', "/"));
    assert!(storage.restore_path(&info.id, &real, &out).is_err());
}
