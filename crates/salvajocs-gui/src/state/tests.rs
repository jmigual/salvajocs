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
