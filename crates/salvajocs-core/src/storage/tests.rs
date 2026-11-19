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
fn opening_a_missing_repo_reports_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let location = local(&tmp.path().join("nothing-here"));
    assert!(matches!(
        Storage::open(&location, ""),
        Err(StorageError::Missing)
    ));
}

#[cfg(unix)]
#[test]
fn opening_an_unreadable_local_repo_is_a_backend_error_not_missing() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let repo_dir = tmp.path().join("repo");
    let location = local(&repo_dir);
    Storage::init(&location, "", Compression::Default).unwrap();
    fs::set_permissions(&repo_dir, fs::Permissions::from_mode(0o000)).unwrap();
    // Root ignores permission bits, so there is nothing to observe.
    let enforced = fs::exists(repo_dir.join("config")).is_err();
    let result = Storage::open(&location, "");
    fs::set_permissions(&repo_dir, fs::Permissions::from_mode(0o755)).unwrap();
    if enforced {
        assert!(matches!(result, Err(StorageError::Backend(_))));
    }
}

/// A REST server that answers every request with 401, as one does when the login changed.
fn refusing_rest_server() -> RepoLocation {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            use std::io::{Read, Write};
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    RepoLocation {
        repository: format!("rest:http://127.0.0.1:{port}/"),
        options: BTreeMap::new(),
    }
}

#[test]
fn a_refused_rest_login_is_not_a_missing_repo() {
    let location = refusing_rest_server();
    assert!(!location.proves_absence());
    assert!(matches!(
        Storage::open(&location, ""),
        Err(StorageError::Backend(_))
    ));
}

#[test]
fn only_local_and_opendal_locations_prove_absence() {
    let repository = |repository: &str| RepoLocation {
        repository: repository.to_string(),
        options: BTreeMap::new(),
    };
    for proves in ["/some/dir", "local:/some/dir", "opendal:s3"] {
        assert!(repository(proves).proves_absence(), "{proves}");
    }
    for does_not in ["rest:http://host/", "rclone:remote:path"] {
        assert!(!repository(does_not).proves_absence(), "{does_not}");
    }
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
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    // The Windows stale-listing race hit about one round in four, so one round proves little.
    // A fresh source per round shares one repository, saving a key derivation per round.
    let rounds = if cfg!(windows) { 20 } else { 1 };
    let mut src = PathBuf::new();
    for round in 0..rounds {
        src = save_dir(&tmp.path().join(round.to_string()));
        assert!(
            storage
                .backup_dir(&src, AS_PATH, TAGS, "Test")
                .unwrap()
                .is_some()
        );
        // A new app run reindexes before the next backup.
        storage.reindex().unwrap();
        assert!(
            storage
                .backup_dir(&src, AS_PATH, TAGS, "Test")
                .unwrap()
                .is_none(),
            "round {round} wrote an extra snapshot"
        );
    }
    assert_eq!(storage.snapshots("salvajocs").unwrap().len(), rounds);

    fs::write(src.join("a.sav"), "changed").unwrap();
    storage.reindex().unwrap();
    assert!(
        storage
            .backup_dir(&src, AS_PATH, TAGS, "Test")
            .unwrap()
            .is_some()
    );
    assert_eq!(storage.snapshots("salvajocs").unwrap().len(), rounds + 1);
}

#[test]
fn forgotten_snapshot_is_no_longer_listed() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let kept = storage
        .backup_dir(&src, AS_PATH, TAGS, "Kept")
        .unwrap()
        .unwrap();
    let forgotten = storage
        .backup_dir(&src, "/salvajocs/other", TAGS, "Forgotten")
        .unwrap()
        .unwrap();
    assert_eq!(storage.snapshots("salvajocs").unwrap().len(), 2);

    storage.forget(&[&forgotten.id]).unwrap();
    assert_eq!(storage.snapshots("salvajocs").unwrap(), vec![kept]);
}

#[test]
fn a_failed_prune_leaves_the_index_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    storage.set_append_only();
    assert!(storage.prune().is_err());
    assert!(storage.stale);
}

#[test]
fn data_only_in_marked_packs_is_written_again_after_a_prune() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let old = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();
    fs::write(src.join("a.sav"), "newer").unwrap();
    storage.reindex().unwrap();
    storage.backup_dir(&src, AS_PATH, TAGS, "Test").unwrap();
    storage.forget(&[&old.id]).unwrap();
    storage.prune().unwrap();
    assert!(!storage.stale);

    // "alpha" is now only in a marked pack, which the reloaded index no longer offers to reuse.
    fs::write(src.join("a.sav"), "alpha").unwrap();
    let again = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();
    let mut fresh = Storage::open(&local(&tmp.path().join("repo")), "").unwrap();
    let out = tmp.path().join("out");
    fresh.restore_path(&again.id, AS_PATH, &out).unwrap();
    assert_eq!(fs::read_to_string(out.join("a.sav")).unwrap(), "alpha");
}

#[test]
fn newest_snapshot_restores_after_forget_and_prune() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let old = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();
    fs::write(src.join("a.sav"), "newer").unwrap();
    storage.reindex().unwrap();
    let newest = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();
    assert!(!storage.has_marked_packs().unwrap());

    storage.forget(&[&old.id]).unwrap();
    storage.prune().unwrap();
    assert!(storage.has_marked_packs().unwrap());

    let out = tmp.path().join("out");
    storage.restore_path(&newest.id, AS_PATH, &out).unwrap();
    assert_eq!(fs::read_to_string(out.join("a.sav")).unwrap(), "newer");
    assert_eq!(fs::read_to_string(out.join("sub/b.sav")).unwrap(), "beta");
}

#[test]
fn read_file_returns_a_file_of_a_snapshot_written_this_session() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let info = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();
    let bytes = storage
        .read_file(&info.id, "/salvajocs/test-slug/sub/b.sav")
        .unwrap();
    assert_eq!(bytes, b"beta");
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

#[test]
fn listed_files_carry_their_nested_path_size_and_mtime() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let info = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();
    let mut listed = storage.snapshot_files(&info.id, AS_PATH).unwrap();
    listed.sort_by(|a, b| a.path.cmp(&b.path));
    let modified = |path: &str| fs::metadata(src.join(path)).unwrap().modified().ok();
    assert_eq!(
        listed,
        [
            SnapshotFile {
                path: "a.sav".into(),
                size: 5,
                modified: modified("a.sav"),
            },
            SnapshotFile {
                path: "sub/b.sav".into(),
                size: 4,
                modified: modified("sub/b.sav"),
            },
        ]
    );
}

#[test]
fn a_game_without_files_lists_none_below_files() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("stage");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("salvajocs.json"), "{}").unwrap();
    fs::write(src.join("registry.json"), "{}").unwrap();
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let info = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();
    let mut paths: Vec<_> = (storage.snapshot_files(&info.id, AS_PATH).unwrap())
        .into_iter()
        .map(|file| file.path)
        .collect();
    paths.sort();
    assert_eq!(paths, ["registry.json", "salvajocs.json"]);
}

#[test]
fn reading_a_file_the_snapshot_lacks_gives_none() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let info = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();
    let mut read = |path: &str| {
        storage
            .read_file_if_present(&info.id, &format!("{AS_PATH}/{path}"))
            .unwrap()
    };
    assert_eq!(read("sub/b.sav"), Some(b"beta".to_vec()));
    assert_eq!(read("missing.json"), None);
    assert_eq!(read("missing/b.sav"), None);
}

#[test]
fn hostname_is_the_host_snapshots_are_stored_with() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let info = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();
    assert_eq!(storage.hostname().unwrap(), info.host);
}
