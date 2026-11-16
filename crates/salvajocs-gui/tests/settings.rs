//! Headless checks of the first-run flow, the startup prompt and the Settings tab.

mod common;

use std::sync::Arc;

use common::{
    env, env_with, harness, local_config, repo_dir, run_until, run_until_ready, save_config, shows,
};
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::config::{AppConfig, PasswordMode, RepoOption};
use salvajocs_gui::env::{MemorySecrets, SecretStore};
use salvajocs_gui::tabs::Tab;

fn fill(harness: &mut egui_kittest::Harness<'static, salvajocs_gui::App>, label: &str, text: &str) {
    let field = harness.get_by_label(label);
    field.focus();
    field.type_text(text);
    harness.run();
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
        !std::path::Path::new(&repository).join("config").exists(),
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
    assert!(std::path::Path::new(&repository).join("config").exists());
    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.repository, repository);
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
    harness.run();
    assert!(shows(&harness, "out of range"), "99 is not a zstd level");
    assert!(
        harness
            .get_by_label("Continue")
            .accesskit_node()
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
    fill(&mut harness, "Password", "pw-1");
    harness.get_by_label("Add option").click();
    harness.run();
    fill(&mut harness, "Option 1 key", "access_key");
    fill(&mut harness, "Option 1 value", "hunter2");
    harness.get_by_label("secret").click();
    harness.step();
    harness.get_by_label("Continue").click();
    run_until(&mut harness, "the confirmation", |h| {
        shows(h, "Create a new repository at")
    });
    harness.get_by_label("Create").click();
    run_until(&mut harness, "the dialog to close", |h| {
        !shows(h, "Welcome to salvajocs")
    });

    let yaml = std::fs::read_to_string(env.config_dir.join("config.yaml")).unwrap();
    assert!(yaml.contains("access_key"), "the key stays in the config");
    assert!(!yaml.contains("hunter2"), "the value does not");
    assert!(!yaml.contains("pw-1"), "neither does the password");
    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.password, PasswordMode::Keyring);
    let scope = saved.secret_scope();
    assert_eq!(
        secrets
            .get(
                &scope,
                &salvajocs_core::secret::SecretKey::Option("access_key".into())
            )
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
        !shows(&harness, "Repository password"),
        "only the missing secrets are asked for"
    );
}

#[test]
fn settings_shows_the_open_repository_compression() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    common::init_repo(&repository);
    save_config(&env, &local_config(&repository));
    let mut harness = harness(env);
    run_until_ready(&mut harness);

    harness.state_mut().set_tab(Tab::Settings);
    harness.run();

    assert!(shows(&harness, &format!("Current: {repository}")));
    assert!(shows(&harness, "Apply compression"));
}
