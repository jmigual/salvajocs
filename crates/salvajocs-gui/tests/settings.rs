//! Headless checks of the first-run flow, the startup prompt and the Settings tab.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::{
    MemorySecrets, env, env_with, harness, init_repo, init_repo_with, local_config, other_repo_dir,
    replace_text, repo_dir, run_settled, run_until, run_until_ready, save_config, shows,
};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use salvajocs_core::config::{AppConfig, PasswordMode, RepoOption};
use salvajocs_core::layout::RootKind;
use salvajocs_core::secret::SecretKey;
use salvajocs_core::storage::Compression;
use salvajocs_gui::env::SecretStore;
use salvajocs_gui::tabs::Tab;
use salvajocs_gui::{App, Env};

fn fill(harness: &mut Harness<'static, App>, label: &str, text: &str) {
    let field = harness.get_by_label(label);
    field.focus();
    field.type_text(text);
    run_settled(harness);
}

/// Waits until the saved config satisfies `ready`; saving is a background job.
fn run_until_saved(
    harness: &mut Harness<'static, App>,
    env: &Env,
    ready: impl Fn(&AppConfig) -> bool,
) {
    let config_dir = env.config_dir.clone();
    run_until(harness, "the config to be saved", move |_| {
        AppConfig::load(&config_dir)
            .ok()
            .flatten()
            .is_some_and(|config| ready(&config))
    });
}

/// An app with the repository at `repo_dir` open and the Settings tab in front.
fn open_settings(env: Env) -> Harness<'static, App> {
    let mut harness = harness(env);
    run_until_ready(&mut harness);
    harness.state_mut().set_tab(Tab::Settings);
    run_settled(&mut harness);
    harness
}

/// Applies `repository` as the new location from the Settings form, confirms its creation and
/// waits until the app saved it.
fn apply_new_location(harness: &mut Harness<'static, App>, env: &Env, repository: &str) {
    replace_text(harness, "Location", repository);
    harness.get_by_label("Apply").click();
    run_until(harness, "the confirmation", |h| {
        shows(h, "Create a new repository at")
    });
    harness.get_by_label("Create").click();
    run_until_saved(harness, env, |config| config.repository == repository);
    // The stage changes in the frame that adopts the repository and is drawn in the next one.
    run_settled(harness);
}

#[test]
fn missing_repo_asks_before_init() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    let mut harness = harness(env.clone());
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    fill(&mut harness, "Location", &repository);
    harness.get_by_label("Continue").click();
    run_until(&mut harness, "the confirmation", |h| {
        shows(h, "Create a new repository at")
    });

    assert!(
        shows(&harness, &repository),
        "the question names the location"
    );
    assert!(
        !Path::new(&repository).join("config").exists(),
        "nothing is created before the user confirms"
    );
    assert!(
        !env.config_dir.join("config.yaml").exists(),
        "no config is saved before the repository exists"
    );

    harness.get_by_label("Create").click();
    run_until(&mut harness, "the dialog to close", |h| {
        !shows(h, "Welcome to salvajocs")
    });
    assert!(Path::new(&repository).join("config").exists());
    run_until_saved(&mut harness, &env, |config| config.repository == repository);
}

#[test]
fn compression_field_shows_error_for_out_of_range_level() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = harness(env(&tmp));
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    fill(&mut harness, "Location", &repo_dir(&tmp));
    assert!(!shows(&harness, "out of range"));

    fill(&mut harness, "Compression (new repository)", "99");
    assert!(shows(&harness, "out of range"), "99 is not a zstd level");
    assert!(
        egui_kittest::kittest::NodeT::accesskit_node(&harness.get_by_label("Continue"))
            .is_disabled(),
        "an invalid level cannot be submitted"
    );
}

#[test]
fn backend_option_marked_secret_not_written_to_config() {
    let tmp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let env = env_with(&tmp, secrets.clone());
    let repository = repo_dir(&tmp);
    let mut harness = harness(env.clone());
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    fill(&mut harness, "Location", &repository);
    fill(&mut harness, "Encryption password", "pw-1");
    harness.get_by_label("Add option").click();
    run_settled(&mut harness);
    fill(&mut harness, "Option 1 key", "access_key");
    fill(&mut harness, "Option 1 value", "hunter2");
    harness.get_by_label("secret").click();
    run_settled(&mut harness);
    harness.get_by_label("Continue").click();
    run_until(&mut harness, "the confirmation", |h| {
        shows(h, "Create a new repository at")
    });
    harness.get_by_label("Create").click();
    run_until(&mut harness, "the dialog to close", |h| {
        !shows(h, "Welcome to salvajocs")
    });
    run_until_saved(&mut harness, &env, |_| true);

    let yaml = std::fs::read_to_string(env.config_dir.join("config.yaml")).unwrap();
    assert!(yaml.contains("access_key"), "the key stays in the config");
    assert!(!yaml.contains("hunter2"), "the value does not");
    assert!(!yaml.contains("pw-1"), "neither does the password");
    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.password, PasswordMode::Keyring);
    let scope = saved.secret_scope();
    assert_eq!(
        secrets
            .get(&scope, &SecretKey::Option("access_key".into()))
            .unwrap()
            .as_deref(),
        Some("hunter2")
    );
}

#[test]
fn startup_prompts_for_missing_secret_option() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let mut config = local_config(&repo_dir(&tmp));
    config.repository_options.push(RepoOption {
        key: "access_key".into(),
        value: None,
        secret: true,
    });
    save_config(&env, &config);
    let mut harness = harness(env);

    run_until(&mut harness, "the prompt", |h| {
        shows(h, "Repository secrets")
    });

    assert!(shows(&harness, "Option access_key"));
    assert!(
        !shows(&harness, "Encryption password"),
        "only the missing secrets are asked for"
    );
}

#[test]
fn a_wrong_keyring_password_prompts_instead_of_looping() {
    let tmp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let env = env_with(&tmp, secrets.clone());
    let repository = repo_dir(&tmp);
    init_repo_with(&repository, "right", Compression::Default);
    let mut config = local_config(&repository);
    config.password = PasswordMode::Keyring;
    let scope = config.secret_scope();
    secrets.set(&scope, &SecretKey::Password, "bad").unwrap();
    save_config(&env, &config);
    let mut harness = harness(env);

    run_until(&mut harness, "the password prompt", |h| {
        shows(h, "Wrong encryption password")
    });
    fill(&mut harness, "Encryption password", "right");
    harness.get_by_label("Open repository").click();

    run_until(&mut harness, "the repository to open", |h| {
        !shows(h, "Repository secrets") && !shows(h, "Opening the repository")
    });
    assert!(!shows(&harness, "could not be opened"));
    let store = secrets.clone();
    run_until(
        &mut harness,
        "the keyring to hold the right password",
        move |_| store.get(&scope, &SecretKey::Password).unwrap().as_deref() == Some("right"),
    );
}

#[test]
fn unreadable_config_blocks_settings_and_is_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    std::fs::create_dir_all(&env.config_dir).unwrap();
    let path = env.config_dir.join("config.yaml");
    std::fs::write(&path, "repository: [unclosed").unwrap();
    let mut harness = harness(env);
    harness.state_mut().set_tab(Tab::Settings);
    run_settled(&mut harness);

    assert!(shows(&harness, "cannot be read"));
    assert!(!shows(&harness, "Location"), "no form can overwrite it");
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "repository: [unclosed"
    );
}

#[test]
fn settings_shows_the_open_repository_compression() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo_with(&repository, "", Compression::Level(19));
    save_config(&env, &local_config(&repository));
    let harness = open_settings(env);

    assert!(shows(&harness, &format!("Current: {repository}")));
    // The heading and the field share the label; only the field has the level as its value.
    assert!(
        harness
            .query_all_by_label("Compression")
            .any(|node| node.value().as_deref() == Some("19")),
        "the field shows the repository's level"
    );
}

#[test]
fn without_roots_settings_says_which_folders_are_found_anyway() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    save_config(&env, &local_config(&repository));
    let mut harness = open_settings(env.clone());
    assert!(shows(&harness, "No roots yet."));

    fill(&mut harness, "Root folder", "/games");
    harness.get_by_label("Add root").click();
    run_settled(&mut harness);
    assert!(!shows(&harness, "No roots yet."));
}

#[test]
fn applying_a_new_location_keeps_roots_added_since_the_form_was_built() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    save_config(&env, &local_config(&repository));
    let mut harness = open_settings(env.clone());

    fill(&mut harness, "Root folder", "/games");
    harness.get_by_label("Add root").click();
    run_settled(&mut harness);
    run_until_saved(&mut harness, &env, |config| config.roots.len() == 1);

    let other = other_repo_dir(&tmp);
    apply_new_location(&mut harness, &env, &other);

    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.roots.len(), 1, "the root added before Apply stays");
    assert_eq!(saved.roots[0].kind, RootKind::Other);
}

#[test]
fn changing_location_offers_to_delete_stale_keyring_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let env = env_with(&tmp, secrets.clone());
    let repository = repo_dir(&tmp);
    init_repo_with(&repository, "pw", Compression::Default);
    let mut config = local_config(&repository);
    config.password = PasswordMode::Keyring;
    let old_scope = config.secret_scope();
    secrets.set(&old_scope, &SecretKey::Password, "pw").unwrap();
    save_config(&env, &config);
    let mut harness = open_settings(env.clone());

    let other = other_repo_dir(&tmp);
    apply_new_location(&mut harness, &env, &other);
    run_until(&mut harness, "the cleanup offer", |h| {
        shows(h, "Delete the keyring entries")
    });
    assert!(
        secrets
            .get(&old_scope, &SecretKey::Password)
            .unwrap()
            .is_some(),
        "nothing is deleted before the user agrees"
    );

    harness.get_by_label("Delete entries").click();
    let check = secrets.clone();
    run_until(&mut harness, "the old entry to go", move |_| {
        check
            .get(&old_scope, &SecretKey::Password)
            .unwrap()
            .is_none()
    });
}

#[test]
fn choosing_a_repository_in_settings_clears_the_open_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    // Nothing was ever created there, so opening it fails.
    save_config(&env, &local_config(&repo_dir(&tmp)));
    let mut harness = harness(env.clone());
    run_until(&mut harness, "the failure", |h| {
        shows(h, "could not be opened")
    });
    harness.state_mut().set_tab(Tab::Settings);
    run_settled(&mut harness);

    apply_new_location(&mut harness, &env, &other_repo_dir(&tmp));

    assert!(!shows(&harness, "could not be opened"));
    assert!(!shows(&harness, "Retry"));
}
