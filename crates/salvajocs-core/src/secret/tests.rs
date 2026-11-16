use super::*;

#[test]
fn account_is_scope_or_scope_hash_option() {
    assert_eq!(SecretKey::Password.account(r#"["r",{}]"#), r#"["r",{}]"#);
    assert_eq!(
        SecretKey::Option("secret_access_key".into()).account(r#"["r",{}]"#),
        r#"["r",{}]#secret_access_key"#
    );
}

#[test]
fn too_long_store_error_is_typed() {
    let err = SecretError::from(Error::TooLong("password".into(), 2560));
    assert!(matches!(err, SecretError::TooLong));
    let err = SecretError::from(Error::NoDefaultStore);
    assert!(matches!(err, SecretError::Store(_)));
}

/// Needs a running credential store (Secret Service on Linux, Credential Manager on Windows).
#[test]
#[ignore = "needs an OS credential store"]
fn keyring_set_get_delete() {
    let scope = format!("salvajocs-test-{}", std::process::id());
    let key = SecretKey::Option("token".into());

    assert_eq!(get(&scope, &key).unwrap(), None);
    set(&scope, &key, "s3cret").unwrap();
    assert_eq!(get(&scope, &key).unwrap().as_deref(), Some("s3cret"));
    delete(&scope, &key).unwrap();
    assert_eq!(get(&scope, &key).unwrap(), None);
    delete(&scope, &key).unwrap();
}
