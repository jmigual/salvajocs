use std::path::Path;
use std::sync::Arc;

use salvajocs_core::manifest::fetch::{FetchError, Freshness};

use super::*;
use crate::env::{MemorySecrets, SecretStore};
use salvajocs_core::config::RepoOption;

fn env(secrets: Arc<dyn SecretStore>) -> Env {
    fn offline(_: &Path) -> Result<Freshness, FetchError> {
        Ok(Freshness::NotModified)
    }
    Env {
        config_dir: "config".into(),
        cache_dir: "cache".into(),
        secrets,
        refresh_manifest: offline,
        environment: None,
    }
}

fn config(password: PasswordMode) -> AppConfig {
    AppConfig {
        repository: "opendal:s3".to_string(),
        repository_options: vec![
            RepoOption {
                key: "access_key".to_string(),
                value: None,
                secret: true,
            },
            RepoOption {
                key: "secret_key".to_string(),
                value: None,
                secret: true,
            },
        ],
        password,
        roots: Vec::new(),
        custom_games: Vec::new(),
    }
}

#[test]
fn resolve_asks_only_for_what_the_keyring_lacks() {
    let secrets = Arc::new(MemorySecrets::new());
    let config = config(PasswordMode::Keyring);
    let scope = config.secret_scope();
    secrets.set(&scope, &SecretKey::Password, "pw").unwrap();
    secrets
        .set(&scope, &SecretKey::Option("access_key".into()), "ak")
        .unwrap();

    let resolved = resolve(&env(secrets), &config);

    assert_eq!(
        resolved.missing,
        [SecretKey::Option("secret_key".to_string())]
    );
    assert_eq!(resolved.found.password, "pw");
    assert_eq!(resolved.found.options["access_key"], "ak");
}

#[test]
fn resolve_always_asks_for_an_ask_mode_password() {
    let resolved = resolve(
        &env(Arc::new(MemorySecrets::new())),
        &config(PasswordMode::Ask),
    );
    assert_eq!(resolved.missing[0], SecretKey::Password);
}

#[test]
fn resolve_asks_for_nothing_without_password_or_secret_options() {
    let mut config = config(PasswordMode::None);
    config.repository_options.clear();
    let resolved = resolve(&env(Arc::new(MemorySecrets::new())), &config);
    assert!(resolved.missing.is_empty());
}

#[test]
fn answers_fill_the_credentials() {
    let mut credentials = Credentials::default();
    credentials.answer(vec![
        (SecretKey::Password, "pw".to_string()),
        (SecretKey::Option("k".to_string()), "v".to_string()),
    ]);
    assert_eq!(credentials.password, "pw");
    assert_eq!(credentials.options["k"], "v");
}

#[test]
fn a_failed_download_keeps_the_error_and_the_empty_cache() {
    fn offline(_: &Path) -> Result<Freshness, FetchError> {
        Err(FetchError::Io(std::io::Error::other("no network")))
    }
    let tmp = tempfile::tempdir().unwrap();

    let load = load_manifest(tmp.path(), offline);

    assert!(load.manifest.is_none());
    assert!(load.error.is_some());
}

#[test]
fn a_cached_manifest_survives_a_failed_download() {
    fn offline(_: &Path) -> Result<Freshness, FetchError> {
        Err(FetchError::Io(std::io::Error::other("no network")))
    }
    let tmp = tempfile::tempdir().unwrap();
    let json = salvajocs_core::manifest::to_json(&Manifest::default()).unwrap();
    std::fs::write(tmp.path().join("manifest.json"), json).unwrap();

    let load = load_manifest(tmp.path(), offline);

    assert!(load.manifest.is_some());
    assert!(load.error.is_some(), "the failure still reaches the banner");
}
