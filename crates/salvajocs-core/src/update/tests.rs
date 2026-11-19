use super::*;

const RELEASE_JSON: &str = r#"{
    "tag_name": "v1.2.3",
    "html_url": "https://github.com/jmigual/salvajocs/releases/tag/v1.2.3",
    "draft": false,
    "prerelease": false,
    "assets": [
        {
            "name": "salvajocs-1.2.3-x86_64-setup.exe",
            "browser_download_url": "https://github.com/jmigual/salvajocs/releases/download/v1.2.3/salvajocs-1.2.3-x86_64-setup.exe",
            "size": 5
        },
        {
            "name": "SHA256SUMS",
            "browser_download_url": "https://github.com/jmigual/salvajocs/releases/download/v1.2.3/SHA256SUMS",
            "size": 100
        }
    ]
}"#;

/// The SHA-256 of `hello`.
const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
const SETUP: &str = "salvajocs-1.2.3-x86_64-setup.exe";

#[test]
fn parse_release_strips_the_v_and_keeps_the_assets() {
    let release = parse_release(RELEASE_JSON).unwrap();
    assert_eq!(release.version, "1.2.3");
    assert_eq!(
        release.html_url,
        "https://github.com/jmigual/salvajocs/releases/tag/v1.2.3"
    );
    assert_eq!(release.assets.len(), 2);
}

#[test]
fn parse_release_rejects_other_json() {
    assert!(matches!(
        parse_release(r#"{"message": "API rate limit exceeded"}"#),
        Err(UpdateError::Json(_))
    ));
}

#[test]
fn is_newer_compares_numerically() {
    assert!(is_newer("0.1.0", "0.2.0").unwrap());
    assert!(is_newer("0.9.0", "0.10.0").unwrap());
    assert!(is_newer("1.2.3", "1.2.4").unwrap());
    assert!(!is_newer("0.1.0", "0.1.0").unwrap());
    assert!(!is_newer("0.10.0", "0.9.0").unwrap());
    assert!(!is_newer("0.1.0", "0.2.0-rc.1").unwrap());
    assert!(!is_newer("0.1.0", "1.2.3+build").unwrap());
    assert!(matches!(
        is_newer("0.1.0", "latest"),
        Err(UpdateError::Version(_))
    ));
}

#[test]
fn asset_picks_the_installer_by_name() {
    let release = parse_release(RELEASE_JSON).unwrap();
    assert_eq!(release.installer_name(), SETUP);
    assert_eq!(
        release.asset(SETUP).unwrap().browser_download_url,
        "https://github.com/jmigual/salvajocs/releases/download/v1.2.3/salvajocs-1.2.3-x86_64-setup.exe"
    );
    assert!(matches!(
        release.asset("salvajocs-1.2.3-aarch64-setup.exe"),
        Err(UpdateError::MissingAsset(_))
    ));
}

#[test]
fn asset_refuses_a_url_outside_this_release() {
    let mut release = parse_release(RELEASE_JSON).unwrap();
    for url in [
        "https://example.invalid/salvajocs-1.2.3-x86_64-setup.exe",
        "http://github.com/jmigual/salvajocs/releases/download/v1.2.3/salvajocs-1.2.3-x86_64-setup.exe",
        "https://github.com/someone/salvajocs/releases/download/v1.2.3/salvajocs-1.2.3-x86_64-setup.exe",
        "https://github.com/jmigual/salvajocs/releases/download/v1.2.2/salvajocs-1.2.3-x86_64-setup.exe",
        "https://github.com/jmigual/salvajocs/releases/download/v1.2.3/../../../../someone/evil/releases/download/x/salvajocs-1.2.3-x86_64-setup.exe",
        "https://github.com/jmigual/salvajocs/releases/download/v1.2.3/%2e%2e/%2e%2e/%2e%2e/%2e%2e/someone/evil/releases/download/x/salvajocs-1.2.3-x86_64-setup.exe",
        "https://github.com/jmigual/salvajocs/releases/download/v1.2.3/other.exe",
    ] {
        release.assets[0].browser_download_url = url.to_string();
        assert!(
            matches!(release.asset(SETUP), Err(UpdateError::ForeignUrl(_))),
            "{url}"
        );
    }
}

#[test]
fn read_capped_refuses_bodies_over_the_limit() {
    assert_eq!(read_capped(Some(5), &b"hello"[..], 5).unwrap(), b"hello");
    assert!(matches!(
        read_capped(Some(6), &b"hello!"[..], 5),
        Err(UpdateError::TooLarge)
    ));
    assert!(matches!(
        read_capped(None, &b"hello!"[..], 5),
        Err(UpdateError::TooLarge)
    ));
}

#[test]
fn checksum_reads_sha256sum_lines() {
    let sums = format!(
        "{HELLO_SHA256}  salvajocs-1.2.3-x86_64-linux.tar.gz\r\n{}  {SETUP}\n{HELLO_SHA256} *other.zip\n",
        HELLO_SHA256.to_uppercase()
    );
    assert_eq!(checksum(&sums, SETUP).as_deref(), Some(HELLO_SHA256));
    assert_eq!(checksum(&sums, "other.zip").as_deref(), Some(HELLO_SHA256));
    assert_eq!(checksum(&sums, "missing.exe"), None);
    assert_eq!(checksum(&format!("abc  {SETUP}"), SETUP), None);
}

#[test]
fn verify_accepts_a_matching_hash() {
    verify(b"hello", &format!("{HELLO_SHA256}  {SETUP}\n"), SETUP).unwrap();
}

#[test]
fn verify_refuses_a_mismatch() {
    assert!(matches!(
        verify(b"hellp", &format!("{HELLO_SHA256}  {SETUP}\n"), SETUP),
        Err(UpdateError::ChecksumMismatch(_))
    ));
}

#[test]
fn verify_refuses_a_file_missing_from_the_sums() {
    assert!(matches!(
        verify(b"hello", &format!("{HELLO_SHA256}  other.exe\n"), SETUP),
        Err(UpdateError::MissingChecksum(_))
    ));
}

#[test]
fn installed_by_installer_looks_for_the_uninstaller() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!installed_by_installer(dir.path()));
    fs::write(dir.path().join(UNINSTALLER), b"").unwrap();
    assert!(installed_by_installer(dir.path()));
}

#[test]
fn download_installer_empties_the_dir_first() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("update");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("salvajocs-1.0.0-x86_64-setup.exe"), b"old").unwrap();
    let release = Release {
        assets: Vec::new(),
        ..parse_release(RELEASE_JSON).unwrap()
    };

    assert!(matches!(
        download_installer(&release, "1.0.0", &dir),
        Err(UpdateError::MissingAsset(_))
    ));
    assert!(!dir.exists());
}

#[test]
fn appimage_name_is_the_release_asset() {
    let release = parse_release(RELEASE_JSON).unwrap();
    assert_eq!(release.appimage_name(), "salvajocs-1.2.3-x86_64.AppImage");
}

/// The names in `dir`.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn replace_swaps_the_file_and_leaves_no_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let appimage = dir.path().join("salvajocs.appimage");
    fs::write(&appimage, b"old").unwrap();

    replace(&appimage, b"new").unwrap();
    assert_eq!(fs::read(&appimage).unwrap(), b"new");
    assert_eq!(names(dir.path()), ["salvajocs.appimage"]);
}

#[cfg(unix)]
#[test]
fn replace_keeps_the_old_mode_and_makes_it_executable() {
    let dir = tempfile::tempdir().unwrap();
    let appimage = dir.path().join("salvajocs.appimage");
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o7777;
    for (old, new) in [(0o640, 0o740), (0o700, 0o700), (0o755, 0o755)] {
        fs::write(&appimage, b"old").unwrap();
        fs::set_permissions(&appimage, fs::Permissions::from_mode(old)).unwrap();
        replace(&appimage, b"new").unwrap();
        assert_eq!(mode(&appimage), new, "{old:o}");
    }
}

#[test]
fn download_limits_each_read_not_the_whole_body() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = io::BufReader::new(&stream);
        let mut line = String::new();
        while io::BufRead::read_line(&mut reader, &mut line).unwrap() > 2 {
            line.clear();
        }
        let mut stream = &stream;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n")
            .unwrap();
        for byte in b"hello" {
            std::thread::sleep(Duration::from_millis(300));
            stream.write_all(&[*byte]).unwrap();
        }
    });
    // Each byte comes within the timeout, the whole body after it.
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    assert_eq!(download(client.get(url)).unwrap(), b"hello");
    server.join().unwrap();
}

#[test]
fn replace_failure_removes_the_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    // A non-empty folder cannot be renamed over.
    let target = dir.path().join("salvajocs.appimage");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("kept"), b"old").unwrap();

    assert!(replace(&target, b"new").is_err());
    assert_eq!(names(dir.path()), ["salvajocs.appimage"]);
    assert_eq!(fs::read(target.join("kept")).unwrap(), b"old");
}

#[test]
fn download_appimage_without_the_asset_leaves_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let appimage = dir.path().join("salvajocs.appimage");
    fs::write(&appimage, b"old").unwrap();
    let release = Release {
        assets: Vec::new(),
        ..parse_release(RELEASE_JSON).unwrap()
    };

    assert!(matches!(
        download_appimage(&release, "1.0.0", &appimage),
        Err(UpdateError::MissingAsset(_))
    ));
    assert_eq!(fs::read(&appimage).unwrap(), b"old");
    assert_eq!(names(dir.path()), ["salvajocs.appimage"]);
}

#[test]
fn replaceable_needs_a_file_in_a_writable_folder() {
    let dir = tempfile::tempdir().unwrap();
    let appimage = dir.path().join("salvajocs.appimage");
    assert!(!replaceable(&appimage), "a missing file");
    fs::write(&appimage, b"old").unwrap();
    assert!(replaceable(&appimage));
    assert_eq!(
        names(dir.path()),
        ["salvajocs.appimage"],
        "the probe is gone"
    );
}

#[cfg(unix)]
#[test]
fn replaceable_refuses_a_read_only_folder() {
    let dir = tempfile::tempdir().unwrap();
    let appimage = dir.path().join("salvajocs.appimage");
    fs::write(&appimage, b"old").unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let replaceable = replaceable(&appimage);
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!replaceable);
}

#[test]
fn running_appimage_needs_the_exe_in_its_mount() {
    let dir = tempfile::tempdir().unwrap();
    let appimage = dir.path().join("salvajocs.appimage");
    fs::write(&appimage, b"").unwrap();
    let mount = dir.path().join("mount");
    fs::create_dir(&mount).unwrap();
    let mount = fs::canonicalize(mount).unwrap();
    let exe = mount.join("usr/bin/salvajocs");

    assert_eq!(
        running_appimage(&exe, appimage.clone(), &mount),
        Some(appimage.clone())
    );
    // Started by another AppImage: its `$APPDIR` does not hold this exe.
    assert_eq!(
        running_appimage(Path::new("/usr/bin/salvajocs"), appimage.clone(), &mount),
        None
    );
    assert_eq!(
        running_appimage(&exe, dir.path().join("missing"), &mount),
        None
    );
}
