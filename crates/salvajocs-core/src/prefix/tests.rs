use super::*;

/// Creates a valid prefix at `root/pfx` with the given user directories.
fn make_prefix(root: &Path, users: &[&str]) -> PathBuf {
    let prefix = root.join("pfx");
    fs::create_dir_all(prefix.join("drive_c/users")).unwrap();
    for user in users {
        fs::create_dir(prefix.join("drive_c/users").join(user)).unwrap();
    }
    fs::write(prefix.join("user.reg"), "").unwrap();
    prefix
}

#[test]
fn proton_prefix_picks_steamuser() {
    let dir = tempfile::tempdir().unwrap();
    let compat = dir.path().join("steamapps/compatdata/620");
    let expected = make_prefix(&compat, &["steamuser", "Public", "deck"]);
    let found = proton_prefix(dir.path(), 620).unwrap();
    assert_eq!(found, expected);
    assert_eq!(wine_user(&found).unwrap(), "steamuser");
    assert_eq!(proton_prefix(dir.path(), 621), None);
}

#[test]
fn single_user_prefix_picks_that_user() {
    let dir = tempfile::tempdir().unwrap();
    let prefix = make_prefix(dir.path(), &["alice", "Public"]);
    assert_eq!(wine_user(&prefix).unwrap(), "alice");
}

#[test]
fn root_and_public_dirs_are_not_users() {
    let dir = tempfile::tempdir().unwrap();
    let prefix = make_prefix(dir.path(), &["root", "Public", "bob"]);
    assert_eq!(wine_user(&prefix).unwrap(), "bob");

    let upper = make_prefix(&dir.path().join("upper"), &["PUBLIC", "Root", "Bob"]);
    assert_eq!(wine_user(&upper).unwrap(), "Bob");

    let only_system = make_prefix(&dir.path().join("empty"), &["root", "Public"]);
    assert!(matches!(
        wine_user(&only_system),
        Err(PrefixError::NoUser(_))
    ));
}

#[test]
fn two_users_without_steamuser_is_ambiguous() {
    let dir = tempfile::tempdir().unwrap();
    let prefix = make_prefix(dir.path(), &["alice", "bob", "Public"]);
    assert!(matches!(
        wine_user(&prefix),
        Err(PrefixError::Ambiguous { users, .. }) if users == ["alice", "bob"]
    ));
}

#[test]
fn dir_without_drive_c_is_not_a_prefix() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("user.reg"), "").unwrap();
    assert!(matches!(
        validate_prefix(dir.path()),
        Err(PrefixError::NotAPrefix(_))
    ));
    assert!(matches!(
        wine_user(dir.path()),
        Err(PrefixError::NotAPrefix(_))
    ));
}

#[test]
fn prefix_without_marker_is_not_a_prefix() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("drive_c/users/alice")).unwrap();
    assert!(matches!(
        validate_prefix(dir.path()),
        Err(PrefixError::NotAPrefix(_))
    ));
    fs::create_dir(dir.path().join("dosdevices")).unwrap();
    assert!(validate_prefix(dir.path()).is_ok());
}

#[test]
fn steam_root_needs_steamapps_and_duplicates_collapse() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("Steam");
    let no_apps = dir.path().join("other");
    fs::create_dir_all(real.join("steamapps")).unwrap();
    fs::create_dir_all(&no_apps).unwrap();
    let missing = dir.path().join("missing");
    #[cfg(unix)]
    let link = {
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        link
    };
    #[cfg(not(unix))]
    let link = real.clone();

    let roots = existing_steam_roots(vec![no_apps, missing, link.clone(), real]);
    assert_eq!(roots, [link]);
}

#[test]
fn wineserver_is_found_by_process_name() {
    let proc_dir = tempfile::tempdir().unwrap();
    assert!(!wineserver_in(proc_dir.path()));
    fs::create_dir(proc_dir.path().join("100")).unwrap();
    fs::write(proc_dir.path().join("100/comm"), "bash\n").unwrap();
    assert!(!wineserver_in(proc_dir.path()));
    fs::create_dir(proc_dir.path().join("200")).unwrap();
    fs::write(proc_dir.path().join("200/comm"), "wineserver\n").unwrap();
    assert!(wineserver_in(proc_dir.path()));
    assert!(!wineserver_in(&proc_dir.path().join("missing")));

    let debian = tempfile::tempdir().unwrap();
    fs::create_dir(debian.path().join("300")).unwrap();
    fs::write(debian.path().join("300/comm"), "wineserver64\n").unwrap();
    assert!(wineserver_in(debian.path()));
}

#[test]
fn a_sandbox_cannot_tell_whether_wineserver_runs() {
    let proc_dir = tempfile::tempdir().unwrap();
    fs::create_dir(proc_dir.path().join("200")).unwrap();
    fs::write(proc_dir.path().join("200/comm"), "wineserver\n").unwrap();
    assert_eq!(wineserver_state(false, proc_dir.path()), Some(true));
    assert_eq!(wineserver_state(true, proc_dir.path()), None);
}

#[test]
fn steamuser_match_is_case_insensitive() {
    let dir = tempfile::tempdir().unwrap();
    let prefix = make_prefix(dir.path(), &["SteamUser", "deck"]);
    assert_eq!(wine_user(&prefix).unwrap(), "SteamUser");
}

#[cfg(unix)]
#[test]
fn non_utf8_user_dir_is_skipped() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let prefix = make_prefix(dir.path(), &["alice"]);
    fs::create_dir(
        prefix
            .join("drive_c/users")
            .join(OsStr::from_bytes(b"b\xffb")),
    )
    .unwrap();
    assert_eq!(wine_user(&prefix).unwrap(), "alice");
}

#[cfg(unix)]
#[test]
fn unreadable_prefix_is_an_io_error_not_a_missing_prefix() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let prefix = make_prefix(dir.path(), &["alice"]);
    fs::set_permissions(&prefix, fs::Permissions::from_mode(0o000)).unwrap();
    let result = validate_prefix(&prefix);
    // root ignores permission bits, so the check only applies to unprivileged runs
    let denied = fs::read_dir(&prefix).is_err();
    fs::set_permissions(&prefix, fs::Permissions::from_mode(0o755)).unwrap();
    if denied {
        assert!(matches!(result, Err(PrefixError::Io { .. })));
    }
}
