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
        }],
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
fn unknown_fields_are_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join(CONFIG_FILE),
        "repository: /backups\npassword: ask\nfuture_setting: 1\n",
    )
    .unwrap();
    assert!(AppConfig::load(tmp.path()).unwrap().is_some());
}
