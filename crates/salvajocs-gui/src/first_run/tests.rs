use std::collections::BTreeMap;

use salvajocs_core::config::RepoOption;

use super::*;
use crate::env::MemorySecrets;

fn config(password: PasswordMode) -> AppConfig {
    let mut config = AppConfig::new("opendal:s3".to_string(), password);
    config.repository_options = vec![RepoOption::new("access_key".to_string(), None, true)];
    config
}

fn credentials() -> Credentials {
    Credentials {
        password: "pw".to_string(),
        options: BTreeMap::from([("access_key".to_string(), "ak".to_string())]),
        ..Credentials::default()
    }
}

#[test]
fn a_refused_login_says_what_to_check() {
    let refused = anyhow::Error::from(StorageError::LoginRefused);
    assert_eq!(failure_message(&refused), LOGIN_REFUSED);
    let other = anyhow::Error::from(StorageError::Missing);
    assert_eq!(failure_message(&other), format!("{other:#}"));
}

#[test]
fn the_form_starts_ticked_only_when_something_is_remembered() {
    let remembered = Credentials {
        options_remembered: true,
        ..credentials()
    };
    assert!(Connect::editing(&config(PasswordMode::None), &remembered).remember);
    assert!(Connect::editing(&config(PasswordMode::Keyring), &credentials()).remember);
    assert!(!Connect::editing(&config(PasswordMode::None), &credentials()).remember);
    assert!(!Connect::editing(&config(PasswordMode::Ask), &credentials()).remember);
}

#[test]
fn remembering_stores_the_password_and_secret_options() {
    let store = MemorySecrets::new();
    let mut config = config(PasswordMode::Keyring);
    let scope = config.secret_scope();

    let warnings = persist_secrets(&store, &mut config, &credentials(), true);

    assert!(warnings.is_empty());
    assert_eq!(config.password, PasswordMode::Keyring);
    assert_eq!(
        store.get(&scope, &SecretKey::Password).unwrap().as_deref(),
        Some("pw")
    );
    assert_eq!(
        store
            .get(&scope, &SecretKey::Option("access_key".into()))
            .unwrap()
            .as_deref(),
        Some("ak")
    );
}

#[test]
fn a_store_that_refuses_the_password_falls_back_to_asking() {
    let store = MemorySecrets::full();
    let mut config = config(PasswordMode::Keyring);

    let warnings = persist_secrets(&store, &mut config, &credentials(), true);

    assert_eq!(config.password, PasswordMode::Ask);
    assert_eq!(warnings.len(), 2, "the password and the option");
}

#[test]
fn not_remembering_forgets_earlier_entries() {
    let store = MemorySecrets::new();
    let mut config = config(PasswordMode::Ask);
    let scope = config.secret_scope();
    store.set(&scope, &SecretKey::Password, "old").unwrap();
    store
        .set(&scope, &SecretKey::Option("access_key".into()), "old")
        .unwrap();

    persist_secrets(&store, &mut config, &credentials(), false);

    assert_eq!(store.get(&scope, &SecretKey::Password).unwrap(), None);
    assert_eq!(
        store
            .get(&scope, &SecretKey::Option("access_key".into()))
            .unwrap(),
        None
    );
}

#[test]
fn no_password_stores_no_password_entry() {
    let store = MemorySecrets::new();
    let mut config = config(PasswordMode::None);
    let scope = config.secret_scope();

    persist_secrets(&store, &mut config, &credentials(), true);

    assert_eq!(store.get(&scope, &SecretKey::Password).unwrap(), None);
}
