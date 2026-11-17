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
