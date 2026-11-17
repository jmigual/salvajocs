use std::cell::RefCell;

use super::*;
use crate::layout::RootKind;
use crate::manifest::{GameEntry, Integration};

fn option(key: &str, value: Option<&str>, secret: bool) -> RepoOption {
    RepoOption {
        key: key.into(),
        value: value.map(Into::into),
        secret,
    }
}

fn sample() -> AppConfig {
    AppConfig {
        repository: "opendal:s3".into(),
        repository_options: vec![
            option("bucket", Some("saves"), false),
            option("secret_access_key", None, true),
        ],
        password: PasswordMode::Keyring,
        roots: vec![Root {
            path: "/games".into(),
            kind: RootKind::Other,
        }],
        custom_games: vec![CustomGame {
            name: "Homebrew".into(),
            entry: GameEntry::default(),
            integration: Integration::Extend,
            wine_prefix: None,
            pairs: Vec::new(),
            skips: Vec::new(),
        }],
        check_updates: false,
    }
}

#[test]
fn missing_file_is_first_run() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(AppConfig::load(tmp.path()).unwrap(), None);
}

#[test]
fn save_then_load_is_equal() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("nested");
    let config = sample();
    config.save(&dir).unwrap();
    assert_eq!(AppConfig::load(&dir).unwrap(), Some(config));
    assert!(!dir.join("config.yaml.tmp").exists());
}

#[test]
fn config_without_check_updates_checks_for_updates() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join(CONFIG_FILE),
        "repository: /backups\npassword: none\n",
    )
    .unwrap();
    assert!(AppConfig::load(tmp.path()).unwrap().unwrap().check_updates);
}

#[test]
fn invalid_file_is_a_parse_error() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join(CONFIG_FILE), "password: [unclosed").unwrap();
    assert!(matches!(
        AppConfig::load(tmp.path()),
        Err(ConfigError::Parse(_))
    ));
}

#[test]
fn config_file_never_contains_password_or_secret_options() {
    let tmp = tempfile::tempdir().unwrap();
    let mut config = sample();
    config.repository_options[1].value = Some("hunter2".into());
    config.save(tmp.path()).unwrap();

    let yaml = fs::read_to_string(tmp.path().join(CONFIG_FILE)).unwrap();
    assert!(!yaml.contains("hunter2"));
    let loaded = AppConfig::load(tmp.path()).unwrap().unwrap();
    assert_eq!(
        loaded.repository_options[1],
        option("secret_access_key", None, true)
    );
}

#[test]
fn location_merges_secret_option_values() {
    let secrets = BTreeMap::from([("secret_access_key".to_string(), "hunter2".to_string())]);
    let location = sample().location(&secrets).unwrap();
    assert_eq!(location.repository, "opendal:s3");
    assert_eq!(
        location.options,
        BTreeMap::from([
            ("bucket".to_string(), "saves".to_string()),
            ("secret_access_key".to_string(), "hunter2".to_string()),
        ])
    );
    assert!(matches!(
        sample().location(&BTreeMap::new()),
        Err(ConfigError::MissingValue(key)) if key == "secret_access_key"
    ));
}

#[test]
fn backend_kinds_start_with_local_rest_rclone() {
    let kinds = backend_kinds();
    assert_eq!(
        kinds[..3],
        [BackendKind::Local, BackendKind::Rest, BackendKind::Rclone]
    );
    assert!(kinds.contains(&BackendKind::OpenDal("s3")));
}

/// Services rustic_backend builds that the picker hides (ADR-0009).
const HIDDEN_SERVICES: &[&str] = &["fs", "ghac", "http", "memory"];

/// Extra schemes the registry maps onto services above.
const ALIAS_SCHEMES: &[&str] = &["file", "cosn", "https"];

/// Fails when a rustic_backend upgrade adds or drops an opendal service, so the list is revisited.
#[test]
fn opendal_services_match_the_built_registry() {
    opendal::init_default_registry();
    let built = opendal::OperatorRegistry::get().schemes();
    let listed: std::collections::HashSet<String> = OPENDAL_SERVICES
        .iter()
        .map(|service| service.name)
        .chain(HIDDEN_SERVICES.iter().copied())
        .chain(ALIAS_SCHEMES.iter().copied())
        .map(str::to_string)
        .collect();
    assert_eq!(built, listed);
}

#[test]
fn hidden_services_are_not_offered() {
    for service in HIDDEN_SERVICES {
        assert!(!backend_kinds().contains(&BackendKind::OpenDal(service)));
    }
}

#[cfg(windows)]
#[test]
fn backend_kinds_exclude_sftp_on_windows() {
    assert!(!backend_kinds().contains(&BackendKind::OpenDal("sftp")));
}

#[cfg(not(windows))]
#[test]
fn backend_kinds_include_sftp_off_windows() {
    assert!(backend_kinds().contains(&BackendKind::OpenDal("sftp")));
}

#[test]
fn listremotes_output_is_one_remote_per_line() {
    assert_eq!(parse_remotes("gdrive:\r\nnas:\n\n"), ["gdrive", "nas"]);
    assert!(parse_remotes("").is_empty());
}

#[test]
fn every_offered_service_has_named_fields_and_a_label() {
    for service in OPENDAL_SERVICES {
        assert!(!opendal_fields(service.name).is_empty(), "{}", service.name);
        assert_eq!(opendal_label(service.name), Some(service.label));
    }
    assert!(opendal_fields("memory").is_empty());
    assert_eq!(opendal_label("memory"), None);
}

/// The keys of opendal's config struct for `service`, which ignores unknown keys when it is
/// built, so a typo in the table would otherwise only show at runtime.
fn config_keys(service: &str) -> Vec<String> {
    use opendal::services::*;
    let value = match service {
        "azblob" => serde_json::to_value(AzblobConfig::default()),
        "azdls" => serde_json::to_value(AzdlsConfig::default()),
        "azfile" => serde_json::to_value(AzfileConfig::default()),
        "b2" => serde_json::to_value(B2Config::default()),
        "cos" => serde_json::to_value(CosConfig::default()),
        "dropbox" => serde_json::to_value(DropboxConfig::default()),
        "ftp" => serde_json::to_value(FtpConfig::default()),
        "gcs" => serde_json::to_value(GcsConfig::default()),
        "gdrive" => serde_json::to_value(GdriveConfig::default()),
        "ipmfs" => serde_json::to_value(IpmfsConfig::default()),
        "obs" => serde_json::to_value(ObsConfig::default()),
        "onedrive" => serde_json::to_value(OnedriveConfig::default()),
        "oss" => serde_json::to_value(OssConfig::default()),
        "pcloud" => serde_json::to_value(PcloudConfig::default()),
        "s3" => serde_json::to_value(S3Config::default()),
        #[cfg(not(windows))]
        "sftp" => serde_json::to_value(SftpConfig::default()),
        "swift" => serde_json::to_value(SwiftConfig::default()),
        "webdav" => serde_json::to_value(WebdavConfig::default()),
        "webhdfs" => serde_json::to_value(WebhdfsConfig::default()),
        "yandex-disk" => serde_json::to_value(YandexDiskConfig::default()),
        other => panic!("no config struct is listed for {other}"),
    };
    value
        .expect("a config struct serializes")
        .as_object()
        .expect("a config struct serializes to an object")
        .keys()
        .cloned()
        .collect()
}

/// Fails when an opendal upgrade renames a key the form writes.
#[test]
fn named_field_keys_are_opendal_config_keys() {
    for service in OPENDAL_SERVICES {
        let keys = config_keys(service.name);
        for field in service.fields {
            assert!(
                keys.iter().any(|key| key == field.key),
                "{} has no option `{}`",
                service.name,
                field.key
            );
        }
    }
}

#[test]
fn docs_url_names_the_config_struct() {
    assert!(opendal_docs_url("yandex-disk").ends_with("struct.YandexDiskConfig.html"));
    assert!(opendal_docs_url("s3").ends_with("struct.S3Config.html"));
}

#[test]
fn missing_secrets_lists_password_and_secret_options() {
    let asked = RefCell::new(Vec::new());
    let nothing = |key: &SecretKey| {
        asked.borrow_mut().push(key.clone());
        None
    };
    let mut config = sample();
    assert_eq!(
        missing_secrets(&config, nothing),
        [
            SecretKey::Password,
            SecretKey::Option("secret_access_key".into())
        ]
    );
    // Plain options are never looked up.
    assert!(!asked.borrow().contains(&SecretKey::Option("bucket".into())));

    let everything = |_: &SecretKey| Some("x".to_string());
    assert!(missing_secrets(&config, everything).is_empty());

    config.password = PasswordMode::Ask;
    assert_eq!(missing_secrets(&config, everything), [SecretKey::Password]);

    config.password = PasswordMode::None;
    assert_eq!(
        missing_secrets(&config, |_| None),
        [SecretKey::Option("secret_access_key".into())]
    );
}

#[test]
fn secret_scope_differs_per_bucket_and_ignores_option_order() {
    let mut other = sample();
    other.repository_options[0].value = Some("other-bucket".into());
    assert_ne!(sample().secret_scope(), other.secret_scope());

    let mut reordered = sample();
    reordered
        .repository_options
        .insert(0, option("region", Some("eu"), false));
    let mut shuffled = reordered.clone();
    shuffled.repository_options.reverse();
    assert_eq!(reordered.secret_scope(), shuffled.secret_scope());

    // A secret option's value never reaches the scope.
    let mut with_secret = sample();
    with_secret.repository_options[1].value = Some("hunter2".into());
    assert_eq!(with_secret.secret_scope(), sample().secret_scope());
}

#[test]
fn debug_output_hides_secret_values() {
    let mut config = sample();
    config.repository_options[1].value = Some("hunter2".into());
    assert!(!format!("{config:?}").contains("hunter2"));

    let secrets = BTreeMap::from([("secret_access_key".to_string(), "hunter2".to_string())]);
    let location = config.location(&secrets).unwrap();
    assert!(!format!("{location:?}").contains("hunter2"));
}

#[test]
fn minimal_file_loads_with_empty_lists() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join(CONFIG_FILE),
        "repository: /backups\npassword: none\n",
    )
    .unwrap();
    let config = AppConfig::load(tmp.path()).unwrap().unwrap();
    assert_eq!(config.repository, "/backups");
    assert_eq!(config.password, PasswordMode::None);
    assert!(config.repository_options.is_empty());
    assert!(config.roots.is_empty());
    assert!(config.custom_games.is_empty());
}

#[test]
fn custom_game_without_skips_loads_and_skips_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join(CONFIG_FILE),
        "repository: /backups\npassword: none\ncustom_games:\n\
         - name: Old\n  entry: {}\n  integration: extend\n",
    )
    .unwrap();
    let mut config = AppConfig::load(tmp.path()).unwrap().unwrap();
    assert!(config.custom_games[0].skips.is_empty());

    config.custom_games[0].skips = vec!["<xdgData>/Foo".into()];
    config.save(tmp.path()).unwrap();
    assert_eq!(AppConfig::load(tmp.path()).unwrap(), Some(config));
}

#[test]
fn unknown_fields_are_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join(CONFIG_FILE),
        "repository: /backups\npassword: ask\nfuture_setting: 1\n",
    )
    .unwrap();
    assert!(AppConfig::load(tmp.path()).unwrap().is_some());
}
