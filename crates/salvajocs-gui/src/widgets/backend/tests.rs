use salvajocs_core::config::PasswordMode;

use super::*;

/// Fails when a picker entry has no heading in the options reference, which would leave its
/// link pointing at the top of the page.
#[test]
fn every_backend_has_a_section_in_the_options_reference() {
    // The page is not in the crates.io tarball, so the test is skipped there.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/guide/advanced-options.md");
    let Ok(page) = std::fs::read_to_string(&path) else {
        eprintln!("skipped: {} is not here", path.display());
        return;
    };
    for kind in config::backend_kinds() {
        let heading = format!("## {}", kind_label(kind));
        assert!(
            page.lines().any(|line| line == heading),
            "no section for {}",
            kind_label(kind)
        );
    }
}

#[test]
fn options_link_names_the_tag_and_the_section() {
    let url = options_docs_url(BackendKind::OpenDal("s3"));
    assert!(url.starts_with("https://github.com/jmigual/salvajocs/blob/v"));
    assert!(url.contains(&format!("/blob/v{}/docs/", env!("CARGO_PKG_VERSION"))));
    assert!(url.ends_with("/docs/guide/advanced-options.md#amazon-s3-or-compatible"));
    assert!(options_docs_url(BackendKind::OpenDal("ipmfs")).ends_with("#ipfs-mfs"));
    assert!(options_docs_url(BackendKind::Rest).ends_with("#rest-server"));
}

fn config(repository: &str, options: Vec<RepoOption>) -> AppConfig {
    let mut config = AppConfig::new(repository.to_string(), PasswordMode::None);
    config.repository_options = options;
    config
}

fn option(key: &str, value: Option<&str>, secret: bool) -> RepoOption {
    RepoOption::new(key.to_string(), value.map(str::to_string), secret)
}

fn keys(options: &[OptionRow]) -> Vec<&str> {
    options.iter().map(|row| row.key.as_str()).collect()
}

/// The options as a set, since their order is not part of the config's meaning.
fn option_set(options: Vec<RepoOption>) -> BTreeSet<(String, Option<String>, bool)> {
    options
        .into_iter()
        .map(|o| (o.key, o.value, o.secret))
        .collect()
}

#[test]
fn repository_strings_round_trip_through_the_form() {
    for repository in [
        "/backups/games",
        "rest:https://host/repo",
        "rclone:remote:games",
        "opendal:s3",
        "opendal:memory",
    ] {
        let form = BackendForm::from_config(&config(repository, Vec::new()), &BTreeMap::new());
        assert_eq!(form.repository(), repository);
    }
}

#[test]
fn a_secret_option_keeps_its_value_out_of_the_config_options() {
    let mut form = BackendForm::default();
    form.push_option("access_key", "hunter2", true);
    form.push_option("bucket", "games", false);
    form.push_option("  ", "ignored", false);

    let options = form.options();
    assert_eq!(options.len(), 2);
    assert_eq!(options[0].value, None);
    assert!(options[0].secret);
    assert_eq!(options[1].value.as_deref(), Some("games"));
    assert_eq!(
        form.secrets(),
        BTreeMap::from([("access_key".to_string(), "hunter2".to_string())])
    );
}

#[test]
fn from_config_fills_secret_rows_from_the_session_secrets() {
    let option = RepoOption::new("access_key".to_string(), None, true);
    let secrets = BTreeMap::from([("access_key".to_string(), "hunter2".to_string())]);
    let form = BackendForm::from_config(&config("opendal:s3", vec![option]), &secrets);
    assert_eq!(form.secrets(), secrets);
}

#[test]
fn named_and_advanced_options_round_trip() {
    let secrets = BTreeMap::from([("secret_access_key".to_string(), "hunter2".to_string())]);
    let original = config(
        "opendal:s3",
        vec![
            option("bucket", Some("games"), false),
            option("region", Some("eu-west-1"), false),
            option("secret_access_key", None, true),
            option("retry", Some("5"), false),
            option("aws_profile", Some("x"), false),
        ],
    );
    let form = BackendForm::from_config(&original, &secrets);

    assert_eq!(form.named.get("bucket").map(String::as_str), Some("games"));
    assert_eq!(keys(&form.options), ["retry", "aws_profile"]);
    assert!(form.advanced_open, "a loaded Advanced option is not hidden");
    assert_eq!(
        option_set(form.options()),
        option_set(original.repository_options.clone())
    );
    assert_eq!(form.secrets(), secrets);
    let mut saved = original.clone();
    saved.repository_options = form.options();
    assert_eq!(saved.secret_scope(), original.secret_scope());
}

#[test]
fn a_field_stored_with_another_secret_flag_stays_advanced() {
    let original = config(
        "opendal:webdav",
        vec![
            option("endpoint", Some("https://dav.example.com"), false),
            option("password", Some("plain"), false),
        ],
    );
    let form = BackendForm::from_config(&original, &BTreeMap::new());

    assert_eq!(keys(&form.options), ["password"]);
    assert_eq!(
        option_set(form.options()),
        option_set(original.repository_options.clone())
    );
    let mut saved = original.clone();
    saved.repository_options = form.options();
    assert_eq!(saved.secret_scope(), original.secret_scope());
}

#[test]
fn an_option_with_an_empty_value_stays_advanced_and_is_kept() {
    let original = config(
        "opendal:webdav",
        vec![
            option("endpoint", Some("https://dav.example.com"), false),
            option("root", Some(""), false),
            option("password", None, true),
        ],
    );
    let form = BackendForm::from_config(&original, &BTreeMap::new());

    assert_eq!(keys(&form.options), ["root", "password"]);
    assert_eq!(
        option_set(form.options()),
        option_set(original.repository_options.clone())
    );
    let mut saved = original.clone();
    saved.repository_options = form.options();
    assert_eq!(saved.secret_scope(), original.secret_scope());
}

#[test]
fn a_required_secret_stored_plain_stays_complete() {
    let original = config(
        "opendal:pcloud",
        vec![
            option("endpoint", Some("https://api.pcloud.com"), false),
            option("username", Some("me@example.com"), false),
            option("password", Some("plain"), false),
        ],
    );
    let form = BackendForm::from_config(&original, &BTreeMap::new());

    assert!(form.is_complete(), "the untouched config still applies");
    assert_eq!(
        option_set(form.options()),
        option_set(original.repository_options.clone())
    );
    let mut saved = original.clone();
    saved.repository_options = form.options();
    assert_eq!(saved.secret_scope(), original.secret_scope());
}

#[test]
fn empty_named_fields_are_not_emitted() {
    let mut form = BackendForm::from_config(&config("opendal:s3", Vec::new()), &BTreeMap::new());
    form.set_named("bucket", "games");
    form.set_named("region", "");
    form.set_named("secret_access_key", "  ");

    assert_eq!(
        option_set(form.options()),
        option_set(vec![option("bucket", Some("games"), false)])
    );
    assert!(form.secrets().is_empty());
}

#[test]
fn only_the_current_kinds_named_fields_are_emitted() {
    let mut form =
        BackendForm::from_config(&config("opendal:webdav", Vec::new()), &BTreeMap::new());
    form.set_named("endpoint", "https://dav.example.com");
    form.set_named("bucket", "left over from another service");

    assert_eq!(
        option_set(form.options()),
        option_set(vec![option(
            "endpoint",
            Some("https://dav.example.com"),
            false
        )])
    );
}

#[test]
fn required_fields_gate_is_complete() {
    let mut form =
        BackendForm::from_config(&config("opendal:webdav", Vec::new()), &BTreeMap::new());
    assert!(!form.is_complete());
    form.set_named("endpoint", "   ");
    assert!(!form.is_complete(), "blank is not a value");
    form.set_named("endpoint", "https://dav.example.com");
    assert!(form.is_complete());

    let mut local = BackendForm::default();
    assert!(!local.is_complete(), "a local repository needs a location");
    local.location = "/backups".to_string();
    assert!(local.is_complete());
}

#[test]
fn a_required_key_stored_under_an_alias_counts() {
    let original = config(
        "opendal:s3",
        vec![option("bucket_name", Some("games"), false)],
    );
    let mut form = BackendForm::from_config(&original, &BTreeMap::new());

    assert!(
        form.is_complete(),
        "bucket_name worked before the named fields"
    );
    assert_eq!(
        option_set(form.options()),
        option_set(original.repository_options.clone())
    );
    form.set_named("bucket", "other");
    assert!(
        !form.is_complete(),
        "opendal rejects the alias and the key together"
    );
    assert_eq!(form.duplicate_key().as_deref(), Some("bucket"));
}

#[test]
fn azfile_needs_no_account_name_next_to_its_endpoint() {
    let mut form =
        BackendForm::from_config(&config("opendal:azfile", Vec::new()), &BTreeMap::new());
    form.set_named("share_name", "saves");
    form.set_named("endpoint", "https://myaccount.file.core.windows.net");
    assert!(form.is_complete());
}

#[test]
fn the_duplicate_message_says_where_the_keys_are() {
    let mut form =
        BackendForm::from_config(&config("opendal:webdav", Vec::new()), &BTreeMap::new());
    assert_eq!(form.duplicate_message(), None);

    form.push_option("username", "a", false);
    form.push_option("username", "b", false);
    let twice = form.duplicate_message().unwrap();
    assert!(twice.contains("twice in Advanced"), "{twice}");

    form.options.pop();
    form.set_named("username", "c");
    let both = form.duplicate_message().unwrap();
    assert!(both.contains("both a field and Advanced"), "{both}");
}

#[test]
fn changing_the_kind_drops_secrets_but_keeps_plain_values() {
    let mut form =
        BackendForm::from_config(&config("opendal:webdav", Vec::new()), &BTreeMap::new());
    form.set_named("endpoint", "https://dav.example.com");
    form.set_named("password", "hunter2");

    form.switch_kind(BackendKind::OpenDal("pcloud"));

    assert_eq!(
        form.named.get("endpoint").map(String::as_str),
        Some("https://dav.example.com")
    );
    assert!(!form.named.contains_key("password"));
    assert!(form.secrets().is_empty());
}

#[test]
fn a_secret_comes_back_when_the_user_returns_to_its_service() {
    let secrets = BTreeMap::from([("password".to_string(), "hunter2".to_string())]);
    let original = config(
        "opendal:webdav",
        vec![
            option("endpoint", Some("https://dav.example.com"), false),
            option("password", None, true),
        ],
    );
    let mut form = BackendForm::from_config(&original, &secrets);

    form.switch_kind(BackendKind::OpenDal("pcloud"));
    assert!(form.secrets().is_empty(), "not shown under pCloud");
    form.switch_kind(BackendKind::OpenDal("webdav"));

    assert_eq!(form.secrets(), secrets);
    assert_eq!(
        option_set(form.options()),
        option_set(original.repository_options.clone())
    );
}

#[test]
fn remotes_note_says_why_there_is_nothing_to_choose() {
    let mut form = BackendForm::default();
    assert_eq!(
        form.remotes_note(),
        None,
        "nothing is said before the answer"
    );

    form.remotes = Remotes::Listed(Err(io::ErrorKind::NotFound.into()));
    assert!(form.remotes_note().unwrap().contains("not installed"));

    form.remotes = Remotes::Listed(Err(io::Error::other("config is locked")));
    let note = form.remotes_note().unwrap();
    assert!(note.contains("config is locked"), "{note}");
    assert!(!note.contains("not installed"));

    form.remotes = Remotes::Listed(Ok(Vec::new()));
    assert!(form.remotes_note().unwrap().contains("rclone config"));

    form.remotes = Remotes::Listed(Ok(vec!["nas".to_string()]));
    assert_eq!(form.remotes_note(), None);
}

#[test]
fn a_required_field_set_in_an_advanced_row_counts() {
    let mut form =
        BackendForm::from_config(&config("opendal:webdav", Vec::new()), &BTreeMap::new());
    form.push_option("endpoint", "https://dav.example.com", false);
    assert!(form.is_complete());
}

#[test]
fn a_key_the_backend_does_not_read_is_unknown() {
    let webdav = BackendKind::OpenDal("webdav");
    assert!(is_unknown_key(webdav, "disable_create_dirs"), "a typo");
    assert!(!is_unknown_key(webdav, "disable_create_dir"));
    assert!(
        !is_unknown_key(webdav, "Disable_Create_Dir"),
        "any letter case"
    );
    assert!(!is_unknown_key(webdav, " retry "), "rustic's key, trimmed");
    assert!(
        !is_unknown_key(BackendKind::OpenDal("s3"), "aws_region"),
        "an alias"
    );
    assert!(
        !is_unknown_key(webdav, "  "),
        "an empty row says nothing yet"
    );
    assert!(
        is_unknown_key(BackendKind::Local, "retry"),
        "another backend's key"
    );
}

#[test]
fn opendal_keys_clash_in_any_letter_case_and_by_alias() {
    let mut form = BackendForm::from_config(&config("opendal:s3", Vec::new()), &BTreeMap::new());
    form.set_named("region", "eu-west-1");
    form.push_option("AWS_REGION", "us-east-1", false);
    assert_eq!(
        form.duplicate_key().as_deref(),
        Some("region"),
        "alias, any case"
    );

    form.options.clear();
    form.push_option("custom", "1", false);
    form.push_option("Custom", "2", false);
    assert!(
        form.duplicate_key().is_some(),
        "opendal reads both as one key"
    );
    assert!(!form.is_complete());
}

#[test]
fn rustic_keys_are_exact_so_letter_case_does_not_clash() {
    let mut form =
        BackendForm::from_config(&config("rest:https://host", Vec::new()), &BTreeMap::new());
    form.push_option("retry", "1", false);
    form.push_option("Retry", "2", false);
    assert_eq!(form.duplicate_key(), None);

    let mut form = BackendForm::from_config(&config("opendal:s3", Vec::new()), &BTreeMap::new());
    form.push_option("retry", "1", false);
    form.push_option("Retry", "2", false);
    assert_eq!(
        form.duplicate_key(),
        None,
        "also next to an opendal service"
    );
}

#[test]
fn the_unknown_mark_follows_the_backend() {
    let mut form =
        BackendForm::from_config(&config("opendal:webdav", Vec::new()), &BTreeMap::new());
    form.push_option("post-create-command", "x", false);
    assert!(form.options[0].unknown);
    form.switch_kind(BackendKind::Local);
    assert!(!form.options[0].unknown, "a key of the local backend");
}

#[test]
fn duplicate_key_blocks_is_complete() {
    let mut form =
        BackendForm::from_config(&config("opendal:webdav", Vec::new()), &BTreeMap::new());
    form.set_named("endpoint", "https://dav.example.com");
    form.push_option("endpoint", "https://other.example.com", false);
    assert!(!form.is_complete());
    assert_eq!(form.duplicate_key().as_deref(), Some("endpoint"));

    form.options.clear();
    form.push_option("retry", "1", false);
    form.push_option("retry", "2", false);
    assert!(!form.is_complete(), "two Advanced rows clash too");
}

#[test]
fn an_added_row_ticks_secret_for_a_key_that_looks_secret() {
    let mut row = OptionRow::added();
    for (key, secret) in [("api_token", true), ("api_toke", false), ("Password", true)] {
        row.key = key.to_string();
        row.key_changed(BackendKind::Local);
        assert_eq!(row.secret, secret, "{key}");
    }
}

#[test]
fn a_row_the_user_set_keeps_its_secret_flag() {
    let mut added = OptionRow::added();
    added.key = "api_key".to_string();
    added.key_changed(BackendKind::Local);
    added.secret = false;
    added.follows_key = false;
    added.key = "api_keys".to_string();
    added.key_changed(BackendKind::Local);
    assert!(!added.secret, "unticked by the user");

    let loaded = config("opendal:s3", vec![option("my_token", Some("x"), false)]);
    let mut form = BackendForm::from_config(&loaded, &BTreeMap::new());
    form.options[0].key_changed(BackendKind::OpenDal("s3"));
    assert!(!form.options[0].secret, "a loaded row never flips");
}

#[test]
fn only_an_rclone_remote_with_use_password_false_warns_of_no_login() {
    let mut form =
        BackendForm::from_config(&config("rclone:remote:games", Vec::new()), &BTreeMap::new());
    assert!(
        !form.rclone_without_login(),
        "no option, so the default of true"
    );
    form.push_option("use-password", "true", false);
    assert!(!form.rclone_without_login());
    form.options[0].value = "false".to_string();
    assert!(form.rclone_without_login());

    form.switch_kind(BackendKind::Rest);
    assert!(!form.rclone_without_login(), "only rclone starts a server");
}

#[test]
fn a_password_in_the_rest_url_moves_to_the_password_field() {
    let loaded = config("rest:https://user:p%40ss@host/repo", Vec::new());
    let mut form = BackendForm::from_config(&loaded, &BTreeMap::new());
    assert_eq!(form.repository(), "rest:https://user@host/repo");
    assert_eq!(
        form.secrets(),
        BTreeMap::from([("rest-password".to_string(), "p@ss".to_string())])
    );
    assert_eq!(
        option_set(form.options()),
        option_set(vec![option("rest-password", None, true)])
    );

    // Pasting a URL with a password later does the same, and replaces the field's value.
    form.location = " https://other:new@host2/ ".to_string();
    form.take_pasted_password();
    assert_eq!(form.repository(), "rest:https://other@host2/");
    assert_eq!(form.secrets()["rest-password"], "new");
}

#[test]
fn the_rest_password_field_keeps_the_scope_and_stays_with_rest() {
    let saved = config(
        "rest:https://user@host/",
        vec![option("rest-password", None, true)],
    );
    let secrets = BTreeMap::from([("rest-password".to_string(), "hunter2".to_string())]);
    let mut form = BackendForm::from_config(&saved, &secrets);
    assert!(form.options.is_empty(), "the secret option is in the field");
    assert_eq!(form.secrets(), secrets);

    let mut applied = config(&form.repository(), Vec::new());
    applied.repository_options = form.options();
    assert_eq!(applied.secret_scope(), saved.secret_scope());

    form.switch_kind(BackendKind::Local);
    assert!(form.secrets().is_empty(), "never saved for another kind");
    form.switch_kind(BackendKind::Rest);
    assert_eq!(form.secrets(), secrets);

    form.push_option("rest-password", "again", true);
    assert!(!form.is_complete(), "set in the field and in Advanced");
}
