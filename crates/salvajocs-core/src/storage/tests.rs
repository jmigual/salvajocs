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

/// The URL of a local server that answers every request with 401, as one does when the login
/// changed.
fn refusing_server() -> String {
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
    format!("http://127.0.0.1:{port}/")
}

fn refusing_rest_server() -> RepoLocation {
    RepoLocation {
        repository: format!("rest:{}", refusing_server()),
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

/// A rest location at `url` with the user's password as the option, not in the URL.
fn rest_with_password(url: &str, password: &str) -> RepoLocation {
    RepoLocation {
        repository: format!("rest:{url}"),
        options: BTreeMap::from([(REST_PASSWORD_KEY.to_string(), password.to_string())]),
    }
}

#[test]
fn the_rest_password_option_is_sent_as_the_login() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sender, requests) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            use std::io::{Read, Write};
            let mut request = [0; 2048];
            let read = stream.read(&mut request).unwrap_or(0);
            let _ = sender.send(String::from_utf8_lossy(&request[..read]).into_owned());
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    let location = rest_with_password(&format!("http://user@127.0.0.1:{port}/"), "p@ss:w/rd");
    assert_eq!(probe(&location, ""), RepoState::Missing);
    let request = requests.recv().unwrap();
    // Base64 of `user:p@ss:w/rd`.
    assert!(
        request.contains("authorization: Basic dXNlcjpwQHNzOncvcmQ="),
        "{request}"
    );
}

#[test]
fn the_rest_password_stays_out_of_debug_and_errors() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut location = rest_with_password(&format!("http://user@127.0.0.1:{port}/"), "p@ss:w/rd");
    // Without this rustic retries for a minute.
    location
        .options
        .insert("retry".to_string(), "false".to_string());
    let RepoState::Error(message) = probe(&location, "") else {
        panic!("nothing listens there");
    };
    let legacy = RepoLocation {
        repository: format!("rest:http://user:hunter2@127.0.0.1:{port}/"),
        options: location.options.clone(),
    };
    for text in [
        message,
        format!("{location:?}"),
        format!("{legacy:?}"),
        format!("{:?}", probe(&legacy, "")),
    ] {
        for leaked in ["p@ss", "p%40ss", "w/rd", "hunter2"] {
            assert!(!text.contains(leaked), "{leaked} in {text}");
        }
    }
}

#[test]
fn the_rest_password_stays_out_of_the_error_for_a_url_rustic_cannot_parse() {
    // Port 99999 does not exist, so rustic fails to parse the URL and quotes it.
    for password in ["p@ss:w/rd", "hunter2"] {
        let location = rest_with_password("http://user@127.0.0.1:99999/", password);
        let RepoState::Error(message) = probe(&location, "") else {
            panic!("the URL is invalid");
        };
        assert!(!message.contains(password), "{message}");
        assert!(!message.contains("p%40ss"), "{message}");
        assert!(
            message.contains("***"),
            "the masked password shows: {message}"
        );
    }
}

#[test]
fn a_refused_webdav_login_is_login_refused_not_missing() {
    let location = RepoLocation {
        repository: "opendal:webdav".to_string(),
        options: BTreeMap::from([("endpoint".to_string(), refusing_server())]),
    };
    assert_eq!(probe(&location, ""), RepoState::LoginRefused);
    assert!(matches!(
        Storage::open(&location, ""),
        Err(StorageError::LoginRefused)
    ));
}

/// An OpenDAL error as rustic reports a failed check for the repository config.
fn config_check_error(err: opendal::Error) -> Box<RusticError> {
    RusticError::with_source(
        rustic_core::ErrorKind::Backend,
        "Path `config` does not exist.",
        err,
    )
}

/// An OpenDAL error for an HTTP response with `status`, with the context OpenDAL gives it.
fn http_error(kind: opendal::ErrorKind, status: u16) -> opendal::Error {
    opendal::Error::new(kind, "Username or password was incorrect").with_context(
        "response",
        format!("Parts {{ status: {status}, version: HTTP/1.1, headers: {{}} }}"),
    )
}

#[test]
fn a_refused_login_in_the_error_chain_is_login_refused() {
    for status in [401, 403] {
        let err = http_error(opendal::ErrorKind::Unexpected, status);
        assert!(matches!(
            open_failure(config_check_error(err)),
            StorageError::LoginRefused
        ));
    }
}

#[test]
fn other_backend_errors_stay_backend_errors() {
    let needle = "response: Parts { status: 401, version: HTTP/1.1, headers: {} }";
    for err in [
        http_error(opendal::ErrorKind::Unexpected, 500),
        // The message is the server's body, and the source any error; both may hold any text.
        opendal::Error::new(opendal::ErrorKind::Unexpected, needle),
        opendal::Error::new(opendal::ErrorKind::Unexpected, "")
            .set_source(std::io::Error::other(needle)),
    ] {
        assert!(matches!(
            open_failure(config_check_error(err)),
            StorageError::Backend(_)
        ));
    }
}

#[test]
fn a_body_with_the_needle_stays_backend() {
    let err = opendal::Error::new(
        opendal::ErrorKind::Unexpected,
        "proxy said: response: Parts { status: 401, ...",
    )
    .with_context(
        "response",
        "Parts { status: 500, version: HTTP/1.1, headers: {} }",
    );
    assert!(matches!(
        open_failure(config_check_error(err)),
        StorageError::Backend(_)
    ));
}

#[test]
fn a_permission_denied_is_not_a_login() {
    // Such as an SFTP folder the user cannot read.
    let err = opendal::Error::new(opendal::ErrorKind::PermissionDenied, "sftp error");
    assert!(matches!(
        open_failure(config_check_error(err)),
        StorageError::Backend(_)
    ));
}

#[test]
fn a_refusal_after_opening_stays_a_backend_error() {
    // A WebDAV server answers 403 for a locked file too.
    let err = http_error(opendal::ErrorKind::PermissionDenied, 403);
    assert!(matches!(
        backend(config_check_error(err)),
        StorageError::Backend(_)
    ));
}

#[test]
fn a_refused_login_is_its_own_probe_state() {
    assert_eq!(
        probe_failed(StorageError::LoginRefused),
        RepoState::LoginRefused
    );
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
        .read_file(&info.id, "/salvajocs/test-slug/sub/b.sav", u64::MAX)
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
                kind: EntryKind::File,
            },
            SnapshotFile {
                path: "sub/b.sav".into(),
                size: 4,
                modified: modified("sub/b.sav"),
                kind: EntryKind::File,
            },
        ]
    );
}

/// [`save_dir`] with a symlink `link` next to `a.sav`, on Unix.
fn save_dir_with_link(root: &Path) -> PathBuf {
    let src = save_dir(root);
    #[cfg(unix)]
    std::os::unix::fs::symlink("a.sav", src.join("link")).unwrap();
    src
}

#[test]
fn snapshot_entries_lists_dirs_files_and_symlinks_with_kinds() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir_with_link(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let info = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();
    let mut listed: Vec<_> = (storage.snapshot_entries(&info.id, AS_PATH).unwrap())
        .into_iter()
        .map(|entry| (entry.path, entry.kind, entry.size))
        .collect();
    listed.sort_by(|a, b| a.0.cmp(&b.0));
    // Only the unix-only cases below add to it.
    #[cfg_attr(not(unix), allow(unused_mut, clippy::useless_vec))]
    let mut expected = vec![
        ("a.sav".to_owned(), EntryKind::File, 5),
        ("sub".to_owned(), EntryKind::Dir, 0),
        ("sub/b.sav".to_owned(), EntryKind::File, 4),
    ];
    #[cfg(unix)]
    expected.insert(
        1,
        ("link".to_owned(), EntryKind::Other, "a.sav".len() as u64),
    );
    assert_eq!(listed, expected);
}

#[test]
fn snapshot_files_still_lists_regular_files_only() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir_with_link(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let info = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();
    let mut listed: Vec<_> = (storage.snapshot_files(&info.id, AS_PATH).unwrap())
        .into_iter()
        .map(|file| (file.path, file.kind))
        .collect();
    listed.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        listed,
        [
            ("a.sav".to_owned(), EntryKind::File),
            ("sub/b.sav".to_owned(), EntryKind::File)
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
            .read_file_if_present(&info.id, &format!("{AS_PATH}/{path}"), u64::MAX)
            .unwrap()
    };
    assert_eq!(read("sub/b.sav"), Some(b"beta".to_vec()));
    assert_eq!(read("missing.json"), None);
    assert_eq!(read("missing/b.sav"), None);
}

#[test]
fn reading_a_file_stops_at_the_limit_whatever_size_the_snapshot_declares() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let info = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();
    let path = format!("{AS_PATH}/sub/b.sav");
    // `beta` is 4 bytes: the read counts what it dumps, not a size from the listing.
    let at = |storage: &mut Storage, limit| storage.read_file_if_present(&info.id, &path, limit);
    assert_eq!(at(&mut storage, 4).unwrap(), Some(b"beta".to_vec()));
    assert!(matches!(
        at(&mut storage, 3),
        Err(StorageError::TooLarge { limit: 3, .. })
    ));
    assert!(matches!(
        at(&mut storage, 0),
        Err(StorageError::TooLarge { .. })
    ));
}

#[test]
fn read_file_stops_at_the_limit_and_still_reports_a_missing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let info = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();
    let path = format!("{AS_PATH}/sub/b.sav");
    assert_eq!(
        storage.read_file(&info.id, &path, 4).unwrap(),
        b"beta".to_vec()
    );
    assert!(matches!(
        storage.read_file(&info.id, &path, 3),
        Err(StorageError::TooLarge { limit: 3, .. })
    ));
    assert!(matches!(
        storage.read_file(&info.id, &format!("{AS_PATH}/missing.sav"), u64::MAX),
        Err(StorageError::Backend(_))
    ));
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

#[test]
fn a_snapshot_another_session_added_is_read_after_a_reindex() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    // Another session, such as another machine, backs up after this one loaded the index.
    let mut other = Storage::open(&local(&tmp.path().join("repo")), "").unwrap();
    let info = other
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();

    let listed = storage.snapshot_entries(&info.id, AS_PATH).unwrap();
    assert_eq!(listed.len(), 3);
    let read = storage
        .read_file_if_present(&info.id, &format!("{AS_PATH}/a.sav"), u64::MAX)
        .unwrap();
    assert_eq!(read, Some(b"alpha".to_vec()));
    let out = tmp.path().join("out");
    storage.restore_path(&info.id, AS_PATH, &out).unwrap();
    assert_eq!(fs::read_to_string(out.join("sub/b.sav")).unwrap(), "beta");
}

#[test]
fn a_backup_and_the_listing_carry_the_size_of_its_files() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let info = storage
        .backup_dir(&src, AS_PATH, TAGS, "Test")
        .unwrap()
        .unwrap();
    // "alpha" and "beta".
    assert!(info.size.is_some_and(|size| size >= 9), "{info:?}");
    assert_eq!(storage.snapshots("salvajocs").unwrap()[0].size, info.size);
}

#[test]
fn a_snapshot_without_a_summary_has_no_size() {
    let snap = RusticSnapshot {
        summary: None,
        ..RusticSnapshot::default()
    };
    assert_eq!(snapshot_info(&snap).size, None);
}

/// `len` bytes no compressor shrinks, the same for the same `seed`.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            // xorshift64
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

const NOISE_LEN: u64 = 64 * 1024;

/// A save dir with a 64 KiB incompressible file, and that file.
fn noisy_save(root: &Path) -> (PathBuf, PathBuf) {
    let src = root.join("noisy");
    fs::create_dir_all(&src).unwrap();
    let file = src.join("big.sav");
    fs::write(&file, noise(NOISE_LEN as usize, 7)).unwrap();
    (src, file)
}

/// Backs `src` up and returns the new snapshot's id; panics when nothing was written.
fn backed_up(storage: &mut Storage, src: &Path) -> String {
    storage
        .backup_dir(src, AS_PATH, TAGS, "Test")
        .unwrap()
        .expect("a new snapshot")
        .id
}

/// Sets `file`'s mtime `seconds` after a fixed time, so a backup after a change stores a new
/// tree with the same data.
fn touch(file: &Path, seconds: u64) {
    let file = fs::File::options().write(true).open(file).unwrap();
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + seconds);
    file.set_modified(time).unwrap();
}

#[test]
fn space_used_counts_a_shared_blob_once() {
    let tmp = tempfile::tempdir().unwrap();
    let (src, file) = noisy_save(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    touch(&file, 0);
    let first = backed_up(&mut storage, &src);
    touch(&file, 1);
    let second = backed_up(&mut storage, &src);

    let one = storage.space_used(&[&first]).unwrap();
    let both = storage.space_used(&[&first, &second]).unwrap();
    assert!(one >= NOISE_LEN, "{one}");
    assert!(
        both > one,
        "the second snapshot's trees count: {both} vs {one}"
    );
    assert!(both < 2 * one, "the data counts once: {both} vs {one}");
}

#[test]
fn space_used_counts_a_shared_tree_once() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let id = backed_up(&mut storage, &src);

    let once = storage.space_used(&[&id]).unwrap();
    assert_eq!(storage.space_used(&[&id, &id]).unwrap(), once);
}

#[test]
fn space_used_reads_a_snapshot_another_session_added() {
    let tmp = tempfile::tempdir().unwrap();
    let src = save_dir(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    // Another session, such as another machine, backs up after this one loaded the index.
    let mut other = Storage::open(&local(&tmp.path().join("repo")), "").unwrap();
    let id = backed_up(&mut other, &src);

    let expected = other.space_used(&[&id]).unwrap();
    assert_eq!(storage.space_used(&[&id]).unwrap(), expected);
}

#[test]
fn space_used_survives_forgetting_the_snapshot_that_added_the_data() {
    let tmp = tempfile::tempdir().unwrap();
    let (src, file) = noisy_save(tmp.path());
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    touch(&file, 0);
    let first = backed_up(&mut storage, &src);
    touch(&file, 1);
    let second = backed_up(&mut storage, &src);

    storage.forget(&[&first]).unwrap();
    assert!(storage.space_used(&[&second]).unwrap() >= NOISE_LEN);
}

#[test]
fn space_used_of_no_snapshot_is_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    assert_eq!(storage.space_used(&[]).unwrap(), 0);
}

/// The timing ADR-0027 records: 20 games of 20 backups each, every game's space computed from a
/// newly opened repository with rustic's tree cache empty (cold), then again (warm).
#[test]
#[ignore = "timing, Linux only; set SALVAJOCS_TIMING=1 and XDG_CACHE_HOME to a dir below the \
            temp dir or the repository's tmp/, run with --release"]
fn space_used_timing() {
    // CI runs the ignored tests too, and the cold run empties rustic's cache.
    if std::env::var_os("SALVAJOCS_TIMING").is_none() {
        eprintln!("skipped: set SALVAJOCS_TIMING=1 and XDG_CACHE_HOME to a scratch dir");
        return;
    }
    let cache = PathBuf::from(std::env::var_os("XDG_CACHE_HOME").expect("XDG_CACHE_HOME set"));
    let cache = cache.canonicalize().expect("XDG_CACHE_HOME exists");
    // Never a developer's real cache, which holds every rustic repository's trees.
    let scratch = [
        std::env::temp_dir(),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp"),
    ];
    assert!(
        (scratch.iter())
            .filter_map(|dir| dir.canonicalize().ok())
            .any(|dir| cache.starts_with(dir)),
        "XDG_CACHE_HOME must be below the temp dir or the repository's tmp/"
    );
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let mut games = Vec::new();
    for game in 0..20u64 {
        let (slug, src) = (
            format!("game-{game}"),
            tmp.path().join(format!("src-{game}")),
        );
        // Ten 16 KiB files in three folders, one of which changes before each backup.
        for index in 0..10u64 {
            let dir = src.join(["a", "a/b", "c"][index as usize % 3]);
            fs::create_dir_all(&dir).unwrap();
            let bytes = noise(16 * 1024, game * 100 + index);
            fs::write(dir.join(format!("{index}.sav")), bytes).unwrap();
        }
        let tags = ["salvajocs", &format!("game:{slug}")];
        let mut ids = Vec::new();
        for backup in 0..20u64 {
            let file = src.join("a/0.sav");
            fs::write(&file, noise(16 * 1024, 10_000 + game * 100 + backup)).unwrap();
            let as_path = format!("/salvajocs/{slug}");
            let info = storage.backup_dir(&src, &as_path, &tags, &slug).unwrap();
            ids.push(info.expect("a new snapshot").id);
        }
        games.push(ids);
    }
    let location = local(&tmp.path().join("repo"));
    let measure = |label: &str| {
        let mut storage = Storage::open(&location, "").unwrap();
        let start = Instant::now();
        let mut total = 0;
        for ids in &games {
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            total += storage.space_used(&ids).unwrap();
        }
        println!(
            "{label}: {:?} for {} games, {total} bytes",
            start.elapsed(),
            games.len()
        );
    };
    let _ = fs::remove_dir_all(cache.join("rustic"));
    measure("cold");
    measure("warm");
}

#[test]
fn space_used_of_an_unknown_snapshot_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_local(tmp.path(), "", Compression::Default);
    let unknown = "0".repeat(64);
    assert!(matches!(
        storage.space_used(&[&unknown]),
        Err(StorageError::Backend(_))
    ));
}
