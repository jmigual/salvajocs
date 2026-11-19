use super::*;

#[test]
fn versions_from_1_52_2_are_supported() {
    for output in [
        "rclone v1.52.2\n- os/arch: linux/amd64\n- go version: go1.14.4\n",
        "rclone v1.66.0\n- os/version: Microsoft Windows 11 Pro 23H2 (64 bit)\n- os/type: windows\n",
        "rclone v1.63.0-beta.7022.e649cf4d5\n- os/arch: linux/amd64\n",
    ] {
        assert!(check_version(output).is_ok(), "{output}");
    }
}

#[test]
fn older_or_unreadable_versions_fail() {
    for output in [
        "",
        "rclone v1.52.1\n- os/arch: linux/amd64\n- go version: go1.14.4\n",
        "rclone v1.51.3-beta\n- os/arch: linux/amd64\n- go version: go1.14.4\n",
    ] {
        assert!(
            matches!(check_version(output), Err(StorageError::RcloneFailed(_))),
            "{output}"
        );
    }
}

#[test]
fn the_served_address_is_read_with_or_without_brackets() {
    for line in [
        "2026/11/19 10:00:00 NOTICE: Serving restic REST API on [http://127.0.0.1:41234/]\n",
        "2026/11/19 10:00:00 NOTICE: Serving restic REST API on http://127.0.0.1:41234/\n",
    ] {
        assert_eq!(served_url(line), Some("http://127.0.0.1:41234/"));
    }
    assert_eq!(served_url("NOTICE: Config file not found\n"), None);
}

#[test]
fn a_missing_rclone_is_rclone_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let program = tmp.path().join("no-rclone");
    let result = Rclone::start_with(
        program.to_str().unwrap(),
        "remote:path",
        &BTreeMap::new(),
        START_TIMEOUT,
    );
    assert!(matches!(result, Err(StorageError::RcloneMissing)));
}

#[test]
fn use_password_must_be_true_or_false() {
    let options = BTreeMap::from([("use-password".to_string(), "yes".to_string())]);
    let result = Rclone::start_with("rclone", "remote:path", &options, START_TIMEOUT);
    assert!(matches!(result, Err(StorageError::Backend(_))));
}

#[test]
fn an_rclone_location_is_opened_through_our_rclone() {
    let location = crate::storage::RepoLocation {
        repository: "rclone:remote:path".to_string(),
        options: BTreeMap::from([(
            "rclone-command".to_string(),
            "salvajocs-no-such-rclone serve restic --addr localhost:0".to_string(),
        )]),
    };
    assert!(matches!(
        crate::storage::Storage::open(&location, ""),
        Err(StorageError::RcloneMissing)
    ));
}

#[cfg(unix)]
mod fake {
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::Receiver;

    use super::*;
    use crate::storage::{RepoLocation, RepoState, Storage, probe};

    /// Writes a fake `rclone` into `dir`. It answers `version`, otherwise records its arguments,
    /// login and pid, runs `then` and sleeps until killed.
    fn fake_rclone(dir: &Path, then: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join("rclone");
        let script = format!(
            "#!/bin/sh\n\
             dir=$(dirname \"$0\")\n\
             if [ \"$1\" = version ]; then echo 'rclone v1.66.0'; exit 0; fi\n\
             printf '%s\\n' \"$@\" > \"$dir/args\"\n\
             printf '%s:%s' \"$RCLONE_USER\" \"$RCLONE_PASS\" > \"$dir/login\"\n\
             echo $$ > \"$dir/pid\"\n\
             {then}\n\
             exec sleep 600\n"
        );
        let source = dir.join("rclone.sh");
        fs::write(&source, script).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
        // A child writes the executable: a concurrent test's fork could inherit a write handle of
        // ours to it, and running it would then fail with ETXTBSY.
        assert!(
            Command::new("cp")
                .arg(&source)
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        path
    }

    fn serving(port: u16) -> String {
        format!("echo 'NOTICE: Serving restic REST API on [http://127.0.0.1:{port}/]' >&2")
    }

    /// A REST server that answers every request with 401 and sends on the head of the first one.
    fn recording_rest_server() -> (u16, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut request = [0; 4096];
                let read = stream.read(&mut request).unwrap_or(0);
                _ = send.send(String::from_utf8_lossy(&request[..read]).into_owned());
                _ = stream.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        (port, receive)
    }

    fn running(dir: &Path) -> bool {
        let pid = fs::read_to_string(dir.join("pid")).unwrap();
        Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    #[test]
    fn rclone_is_started_like_rustic_starts_it() {
        let tmp = tempfile::tempdir().unwrap();
        let program = fake_rclone(tmp.path(), &serving(41234));
        let (rclone, url) = Rclone::start_with(
            program.to_str().unwrap(),
            "remote:path",
            &BTreeMap::new(),
            START_TIMEOUT,
        )
        .unwrap();

        let args = fs::read_to_string(tmp.path().join("args")).unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            ["serve", "restic", "--addr", "localhost:0", "remote:path"]
        );
        let login = fs::read_to_string(tmp.path().join("login")).unwrap();
        let (user, password) = login.split_once(':').unwrap();
        assert!(!user.is_empty() && !password.is_empty());
        assert_eq!(url, format!("http://{login}@127.0.0.1:41234/"));

        assert!(running(tmp.path()));
        drop(rclone);
        assert!(!running(tmp.path()));
    }

    #[test]
    fn rustic_reaches_rclone_as_a_rest_server_and_rclone_stops_after() {
        let tmp = tempfile::tempdir().unwrap();
        let (port, requests) = recording_rest_server();
        let program = fake_rclone(tmp.path(), &serving(port));
        let location = RepoLocation {
            repository: "rclone:remote:path".to_string(),
            options: BTreeMap::from([(
                "rclone-command".to_string(),
                format!("{} serve restic --addr localhost:0", program.display()),
            )]),
        };

        // Any non-2xx answer reads as "no config" over REST.
        assert_eq!(probe(&location, ""), RepoState::Missing);
        let request = requests.recv().unwrap().to_lowercase();
        assert!(request.starts_with("head /config "), "{request}");
        assert!(request.contains("authorization: basic "), "{request}");
        assert!(!running(tmp.path()));

        assert!(matches!(
            Storage::open(&location, ""),
            Err(StorageError::Backend(_))
        ));
        assert!(!running(tmp.path()));
    }

    #[test]
    fn dropping_the_storage_stops_rclone() {
        let tmp = tempfile::tempdir().unwrap();
        let program = fake_rclone(tmp.path(), &serving(41234));
        let (rclone, _) = Rclone::start_with(
            program.to_str().unwrap(),
            "remote:path",
            &BTreeMap::new(),
            START_TIMEOUT,
        )
        .unwrap();
        let repo = RepoLocation {
            repository: tmp.path().join("repo").to_str().unwrap().to_string(),
            options: BTreeMap::new(),
        };
        let mut storage = Storage::init(&repo, "", crate::storage::Compression::Default).unwrap();
        storage._rclone = Some(rclone);

        assert!(running(tmp.path()));
        drop(storage);
        assert!(!running(tmp.path()));
    }

    #[test]
    fn a_silent_rclone_times_out_and_is_stopped() {
        let tmp = tempfile::tempdir().unwrap();
        let program = fake_rclone(tmp.path(), "");
        let result = Rclone::start_with(
            program.to_str().unwrap(),
            "remote:path",
            &BTreeMap::new(),
            Duration::from_secs(2),
        );
        assert!(matches!(result, Err(StorageError::RcloneTimeout(..))));
        assert!(!running(tmp.path()));
    }

    #[test]
    fn an_rclone_that_closes_stderr_and_keeps_running_times_out_with_its_output() {
        let tmp = tempfile::tempdir().unwrap();
        let program = fake_rclone(tmp.path(), "echo 'NOTICE: starting' >&2; exec 2>&-");
        let result = Rclone::start_with(
            program.to_str().unwrap(),
            "remote:path",
            &BTreeMap::new(),
            Duration::from_secs(2),
        );
        let Err(StorageError::RcloneTimeout(_, output)) = result else {
            panic!("expected a timeout");
        };
        assert_eq!(output, "NOTICE: starting");
        assert!(!running(tmp.path()));
    }

    #[test]
    fn without_a_password_rclone_gets_no_login_and_the_url_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let program = fake_rclone(tmp.path(), &serving(41234));
        let options = BTreeMap::from([("use-password".to_string(), "false".to_string())]);
        let (_rclone, url) = Rclone::start_with(
            program.to_str().unwrap(),
            "remote:path",
            &options,
            START_TIMEOUT,
        )
        .unwrap();
        assert_eq!(fs::read_to_string(tmp.path().join("login")).unwrap(), ":");
        assert_eq!(url, "http://127.0.0.1:41234/");
    }

    #[test]
    fn a_rest_url_is_used_without_waiting_for_rclone() {
        let tmp = tempfile::tempdir().unwrap();
        let program = fake_rclone(tmp.path(), "");
        let options = BTreeMap::from([(
            "rest-url".to_string(),
            "http://127.0.0.1:41234/".to_string(),
        )]);
        let (_rclone, url) = Rclone::start_with(
            program.to_str().unwrap(),
            "remote:path",
            &options,
            Duration::from_millis(100),
        )
        .unwrap();
        let login = url
            .strip_prefix("http://")
            .and_then(|rest| rest.strip_suffix("@127.0.0.1:41234/"))
            .unwrap();
        assert!(login.contains(':'), "{url}");
    }

    #[test]
    fn a_rest_url_with_a_password_must_be_http() {
        let tmp = tempfile::tempdir().unwrap();
        let program = fake_rclone(tmp.path(), "");
        let options = BTreeMap::from([(
            "rest-url".to_string(),
            "https://127.0.0.1:41234/".to_string(),
        )]);
        let result = Rclone::start_with(
            program.to_str().unwrap(),
            "remote:path",
            &options,
            START_TIMEOUT,
        );
        assert!(matches!(result, Err(StorageError::Backend(_))));
    }

    #[test]
    fn an_rclone_that_exits_early_fails_with_its_output() {
        let tmp = tempfile::tempdir().unwrap();
        let program = fake_rclone(tmp.path(), "echo 'no such remote' >&2; exit 1");
        let result = Rclone::start_with(
            program.to_str().unwrap(),
            "remote:path",
            &BTreeMap::new(),
            START_TIMEOUT,
        );
        assert!(matches!(result, Err(StorageError::RcloneFailed(_))));
    }
}
