use super::*;

#[test]
fn account_is_repository_or_repository_hash_option() {
    assert_eq!(SecretKey::Password.account("s3repo"), "s3repo");
    assert_eq!(
        SecretKey::Option("secret_access_key".into()).account("opendal:s3"),
        "opendal:s3#secret_access_key"
    );
}

/// Needs a running credential store (Secret Service on Linux, Credential Manager on Windows).
#[test]
#[ignore = "needs an OS credential store"]
fn keyring_set_get_delete() {
    let repository = format!("salvajocs-test-{}", std::process::id());
    let key = SecretKey::Option("token".into());

    assert_eq!(get(&repository, &key).unwrap(), None);
    set(&repository, &key, "s3cret").unwrap();
    assert_eq!(get(&repository, &key).unwrap().as_deref(), Some("s3cret"));
    delete(&repository, &key).unwrap();
    assert_eq!(get(&repository, &key).unwrap(), None);
    delete(&repository, &key).unwrap();
}
