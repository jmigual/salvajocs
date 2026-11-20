use std::cell::RefCell;

use super::services::config_json;
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
            files: Vec::new(),
            entry: GameEntry::default(),
            integration: Integration::Extend,
            wine_prefix: None,
            pairs: Vec::new(),
            skips: Vec::new(),
        }],
        check_updates: false,
        ignored_prefixes: vec!["/prefixes/foo".into()],
        theme: ThemeChoice::Dark,
        backup_sort: ListSort::default(),
        restore_sort: ListSort::default(),
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

#[cfg(unix)]
#[test]
fn a_failed_rclone_is_described_by_its_stderr_or_else_its_status() {
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    let output = |stderr: &str| Output {
        status: ExitStatus::from_raw(1 << 8),
        stdout: Vec::new(),
        stderr: stderr.as_bytes().to_vec(),
    };
    assert_eq!(
        failure(&output("  config is locked\n")).to_string(),
        "config is locked"
    );
    assert!(failure(&output("")).to_string().contains("exit status: 1"));
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

/// What opendal's config struct for `service` reads from `input`, as JSON; `null` is its
/// default. The struct ignores unknown keys when it is built, so a typo in the table would
/// otherwise only show at runtime.
fn config_value(service: &str, input: serde_json::Value) -> serde_json::Value {
    config_json(service, input).expect("the config struct reads it")
}

/// The keys of opendal's config struct for `service`.
fn config_keys(service: &str) -> Vec<String> {
    config_value(service, serde_json::Value::Null)
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

/// Fails when an opendal upgrade adds a key the options reference does not name, so the page is
/// re-checked on a bump. It cannot see an alias or a key opendal dropped. The page is not in the
/// crates.io tarball, so the test is skipped there.
#[test]
fn options_reference_names_every_config_key() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/guide/advanced-options.md");
    let Ok(page) = fs::read_to_string(&path) else {
        eprintln!("skipped: {} is not here", path.display());
        return;
    };
    for service in OPENDAL_SERVICES {
        let heading = format!("## {}", service.label);
        let section: Vec<&str> = page
            .lines()
            .skip_while(|line| *line != heading)
            .skip(1)
            .take_while(|line| !line.starts_with("## "))
            .collect();
        assert!(!section.is_empty(), "no section for {}", service.label);
        let section = section.join("\n");
        for key in config_keys(service.name) {
            assert!(
                section.contains(&format!("`{key}`")),
                "the section of {} does not name `{key}`",
                service.name
            );
        }
    }
}

/// Fails when a table alias is not one opendal reads as the field's key.
#[test]
fn field_aliases_are_read_by_opendal_as_the_field() {
    for service in OPENDAL_SERVICES {
        for field in service.fields {
            for alias in field.aliases {
                // The other required fields are there, for structs that have no default for them.
                let mut input = serde_json::Map::new();
                for other in service.fields.iter().filter(|f| f.required) {
                    input.insert(other.key.to_string(), "y".into());
                }
                input.remove(field.key);
                input.insert(alias.to_string(), "x".into());
                let value = config_value(service.name, input.into());
                assert_eq!(
                    value[field.key], "x",
                    "{} `{alias}` is not `{}`",
                    service.name, field.key
                );
            }
        }
    }
}

#[test]
fn an_option_key_is_known_when_the_backend_reads_it() {
    let webdav = BackendKind::OpenDal("webdav");
    assert!(
        option_key_known(webdav, "disable_create_dir"),
        "a config key"
    );
    assert!(
        option_key_known(webdav, "enable_conditional_read"),
        "true by default"
    );
    assert!(
        option_key_known(webdav, "Disable_Create_Dir"),
        "any letter case"
    );
    assert!(
        option_key_known(webdav, "retry"),
        "rustic's key for services"
    );
    assert!(
        !option_key_known(webdav, "Retry"),
        "rustic matches its keys exactly"
    );
    assert!(!option_key_known(webdav, "disable_create_dirs"), "a typo");
    assert!(
        !option_key_known(webdav, "post-create-command"),
        "another backend's key"
    );
    let s3 = BackendKind::OpenDal("s3");
    assert!(
        option_key_known(s3, "aws_profile"),
        "an alias of a key with no form field"
    );
    assert!(
        option_key_known(s3, "AWS_REGION"),
        "an alias, in any letter case"
    );
    assert!(option_key_known(s3, "assume_role_session_tags"), "a map");
    assert!(
        option_key_known(s3, "assume_role_duration_seconds"),
        "a number"
    );
    assert!(option_key_known(BackendKind::Local, "post-create-command"));
    assert!(!option_key_known(BackendKind::Local, "retry"));
    assert!(option_key_known(BackendKind::Rest, "tls-client-cert"));
    assert!(option_key_known(BackendKind::Rclone, "use-password"));
    assert!(
        option_key_known(BackendKind::Rclone, "timeout"),
        "rclone passes REST keys on"
    );
}

/// Fails when a service has a config key `opendal_reads_key` cannot see, such as one of a type it
/// has no sample for.
#[test]
fn opendal_reads_every_config_key_of_every_service() {
    for service in OPENDAL_SERVICES {
        for key in config_keys(service.name) {
            assert!(
                opendal_reads_key(service.name, &key),
                "{} `{key}`",
                service.name
            );
        }
    }
}

/// Required fields of these services have a fallback or an alias that works on main, so the
/// table must not demand them under the canonical key alone.
#[test]
fn required_fields_leave_out_what_opendal_derives() {
    let required = |service: &str| -> Vec<&str> {
        opendal_fields(service)
            .iter()
            .filter(|field| field.required)
            .map(|field| field.key)
            .collect()
    };
    assert_eq!(required("azfile"), ["share_name", "endpoint"]);
    for service in ["s3", "gcs", "azblob"] {
        let fields = opendal_fields(service);
        assert!(
            fields.iter().any(|f| f.required && !f.aliases.is_empty()),
            "{service} names the aliases of its required fields"
        );
    }
}

#[test]
fn services_are_sorted_by_label_after_the_three_fixed_kinds() {
    let labels: Vec<String> = OPENDAL_SERVICES
        .iter()
        .map(|service| service.label.to_lowercase())
        .collect();
    assert!(labels.is_sorted(), "{labels:?}");
    assert_eq!(
        backend_kinds()[3],
        BackendKind::OpenDal(OPENDAL_SERVICES[0].name)
    );
}

#[test]
fn every_service_has_a_hint() {
    for service in OPENDAL_SERVICES {
        assert!(!service.hint.is_empty(), "{}", service.name);
        assert_eq!(opendal_hint(service.name), Some(service.hint));
    }
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
fn new_configs_and_custom_games_equal_minimal_ones_read_from_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join(CONFIG_FILE),
        "repository: /backups\npassword: ask\ncustom_games:\n\
         - name: Old\n  entry: {}\n  integration: extend\n",
    )
    .unwrap();
    let mut expected = AppConfig::new("/backups".into(), PasswordMode::Ask);
    expected.custom_games = vec![CustomGame::new(
        "Old".into(),
        GameEntry::default(),
        Integration::Extend,
    )];
    assert_eq!(AppConfig::load(tmp.path()).unwrap(), Some(expected));
}

#[test]
fn theme_defaults_to_system_reads_by_name_and_is_not_written_for_system() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join(CONFIG_FILE);
    fs::write(&file, "repository: /backups\npassword: none\n").unwrap();
    let mut config = AppConfig::load(tmp.path()).unwrap().unwrap();
    assert_eq!(config.theme, ThemeChoice::System);

    config.save(tmp.path()).unwrap();
    assert!(!fs::read_to_string(&file).unwrap().contains("theme"));

    fs::write(&file, "repository: /backups\npassword: none\ntheme: dark\n").unwrap();
    config = AppConfig::load(tmp.path()).unwrap().unwrap();
    assert_eq!(config.theme, ThemeChoice::Dark);
    config.save(tmp.path()).unwrap();
    assert!(fs::read_to_string(&file).unwrap().contains("theme: dark"));
}

#[test]
fn sort_defaults_to_name_and_is_not_written() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join(CONFIG_FILE);
    fs::write(&file, "repository: /backups\npassword: none\n").unwrap();
    let config = AppConfig::load(tmp.path()).unwrap().unwrap();
    assert_eq!(config.backup_sort, ListSort::new(SortKey::Name, false));
    assert_eq!(config.restore_sort, ListSort::new(SortKey::Name, false));

    config.save(tmp.path()).unwrap();
    assert!(!fs::read_to_string(&file).unwrap().contains("sort"));
}

#[test]
fn sort_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let mut config = AppConfig::new("/backups".into(), PasswordMode::None);
    config.backup_sort = ListSort::new(SortKey::SpaceUsed, true);
    config.restore_sort = ListSort::new(SortKey::LastBackup, false);
    config.save(tmp.path()).unwrap();

    let yaml = fs::read_to_string(tmp.path().join(CONFIG_FILE)).unwrap();
    assert!(yaml.contains("space_used"), "{yaml}");
    assert_eq!(AppConfig::load(tmp.path()).unwrap(), Some(config));
}

#[test]
fn unknown_sort_value_reads_as_default() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join(CONFIG_FILE),
        "repository: /backups\npassword: none\n\
         backup_sort: { by: play_time, descending: true }\nrestore_sort: 3\n",
    )
    .unwrap();
    let config = AppConfig::load(tmp.path()).unwrap().unwrap();
    assert_eq!(config.repository, "/backups");
    assert_eq!(config.backup_sort, ListSort::default());
    assert_eq!(config.restore_sort, ListSort::default());
}

#[test]
fn a_valid_sort_next_to_an_invalid_one_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join(CONFIG_FILE),
        "repository: /backups\npassword: none\n\
         backup_sort: { by: play_time }\n\
         restore_sort: { by: last_backup_size, descending: true }\n",
    )
    .unwrap();
    let config = AppConfig::load(tmp.path()).unwrap().unwrap();
    assert_eq!(config.backup_sort, ListSort::default());
    let kept = ListSort::new(SortKey::LastBackupSize, true);
    assert_eq!(config.restore_sort, kept);
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
