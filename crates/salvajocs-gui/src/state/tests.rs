use salvajocs_core::manifest::fetch::{FetchError, Freshness};

use crate::env::SecretStore;
use salvajocs_core::config::RepoOption;

use super::*;

fn config(repository: &str, password: PasswordMode, secret_option: bool) -> AppConfig {
    AppConfig {
        repository: repository.to_string(),
        repository_options: secret_option
            .then(|| RepoOption {
                key: "access_key".to_string(),
                value: None,
                secret: true,
            })
            .into_iter()
            .collect(),
        password,
        roots: Vec::new(),
        custom_games: Vec::new(),
        check_updates: true,
    }
}

#[test]
fn a_new_location_leaves_the_old_keyring_entries_stale() {
    let old = config("opendal:s3", PasswordMode::Keyring, true);
    let new = config("/backups", PasswordMode::Keyring, false);

    let stale = stale_credentials(&old, &new).expect("the location changed");

    assert_eq!(stale.scope, old.secret_scope());
    assert_eq!(
        stale.keys,
        [
            SecretKey::Password,
            SecretKey::Option("access_key".to_string())
        ]
    );
}

#[test]
fn the_same_location_leaves_nothing_stale() {
    let old = config("/backups", PasswordMode::Keyring, false);
    assert_eq!(stale_credentials(&old, &old.clone()), None);
}

#[test]
fn a_location_that_stored_nothing_leaves_nothing_stale() {
    let old = config("/backups", PasswordMode::Ask, false);
    let new = config("/elsewhere", PasswordMode::Ask, false);
    assert_eq!(stale_credentials(&old, &new), None);
}

fn state_with(store: crate::env::MemorySecrets) -> State {
    fn offline(_: &std::path::Path) -> Result<Freshness, FetchError> {
        Ok(Freshness::NotModified)
    }
    let tmp = std::env::temp_dir().join("salvajocs-state-tests");
    let mut state = State::new(Env {
        config_dir: tmp.join("config"),
        cache_dir: tmp.join("cache"),
        secrets: Arc::new(store),
        refresh_manifest: offline,
        environment: None,
        wineserver_running: salvajocs_core::prefix::wineserver_running,
    });
    state.config = Some(config("/backups", PasswordMode::Keyring, false));
    state.credentials.password = "right".to_string();
    state
}

fn poll_until_idle(state: &mut State, ctx: &egui::Context) {
    for _ in 0..1000 {
        state.poll(ctx);
        if state.remembering.is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the keyring job never finished");
}

#[test]
fn remembering_a_password_updates_the_stored_secret() {
    let store = crate::env::MemorySecrets::new();
    let scope = config("/backups", PasswordMode::Keyring, false).secret_scope();
    store.set(&scope, &SecretKey::Password, "bad").unwrap();
    let mut state = state_with(store);
    let ctx = egui::Context::default();

    state.remember_password(&ctx);
    poll_until_idle(&mut state, &ctx);

    let stored = state.env.secrets.get(&scope, &SecretKey::Password).unwrap();
    assert_eq!(stored.as_deref(), Some("right"));
    assert_eq!(state.config.unwrap().password, PasswordMode::Keyring);
}

#[test]
fn a_store_that_refuses_the_password_falls_back_to_asking() {
    let mut state = state_with(crate::env::MemorySecrets::full());
    let ctx = egui::Context::default();

    state.remember_password(&ctx);
    poll_until_idle(&mut state, &ctx);

    assert_eq!(state.config.unwrap().password, PasswordMode::Ask);
    assert_eq!(state.notices.len(), 1);
}
