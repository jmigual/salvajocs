//! Headless checks of the update notice, with a fake update check (ADR-0013).

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};

use common::{
    env, harness, init_repo, local_config, other_repo_dir, replace_text, repo_dir, run_settled,
    run_until, run_until_ready, save_config, shows,
};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use salvajocs_core::config::AppConfig;
use salvajocs_core::update::{Release, UpdateError};
use salvajocs_gui::tabs::Tab;
use salvajocs_gui::{App, Env};
use tempfile::TempDir;

fn newer(_current: &str) -> Result<Option<Release>, UpdateError> {
    Ok(Some(Release {
        version: "99.0.0".to_string(),
        html_url: "https://example.invalid/releases/tag/v99.0.0".to_string(),
        assets: Vec::new(),
    }))
}

/// An environment with an open repository and the fake check.
fn update_env(tmp: &TempDir, installed: fn() -> bool) -> Env {
    let mut env = env(tmp);
    env.check_update = Some(newer);
    env.installed_by_installer = installed;
    let repository = repo_dir(tmp);
    init_repo(&repository);
    save_config(&env, &local_config(&repository));
    env
}

fn wait_for_notice(harness: &mut Harness<'static, App>) {
    run_until(harness, "the update notice", |h| {
        shows(h, "Update available: v99.0.0")
    });
}

#[test]
fn installed_app_offers_update_now() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = harness(update_env(&tmp, || true));
    wait_for_notice(&mut harness);
    assert!(shows(&harness, "Update now"));
    assert!(!shows(&harness, "Release page"));
}

#[test]
fn app_not_from_the_installer_links_the_release_page() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = harness(update_env(&tmp, || false));
    wait_for_notice(&mut harness);
    assert!(shows(&harness, "Release page"));
    assert!(!shows(&harness, "Update now"));
}

static CHECKS: AtomicUsize = AtomicUsize::new(0);

fn counted(current: &str) -> Result<Option<Release>, UpdateError> {
    CHECKS.fetch_add(1, Ordering::SeqCst);
    newer(current)
}

#[test]
fn check_turned_off_in_the_config_never_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let mut env = update_env(&tmp, || true);
    env.check_update = Some(counted);
    let mut config = AppConfig::load(&env.config_dir).unwrap().unwrap();
    config.check_updates = false;
    save_config(&env, &config);
    let mut harness = harness(env);
    run_until_ready(&mut harness);
    run_settled(&mut harness);
    assert_eq!(CHECKS.load(Ordering::SeqCst), 0);
    assert!(!shows(&harness, "Update available"));
}

#[test]
fn unticked_checkbox_is_saved_and_survives_a_new_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let env = update_env(&tmp, || false);
    let config_dir = env.config_dir.clone();
    let mut harness = harness(env);
    run_until_ready(&mut harness);
    harness.state_mut().set_tab(Tab::Settings);
    run_settled(&mut harness);

    harness.get_by_label("Check for updates at start").click();
    let saved = |dir: &std::path::Path| AppConfig::load(dir).ok().flatten();
    run_until(&mut harness, "the config to be saved", |_| {
        saved(&config_dir).is_some_and(|config| !config.check_updates)
    });

    let other = other_repo_dir(&tmp);
    replace_text(&mut harness, "Location", &other);
    harness.get_by_label("Apply").click();
    run_until(&mut harness, "the confirmation", |h| {
        shows(h, "Create a new repository at")
    });
    harness.get_by_label("Create").click();
    run_until(&mut harness, "the new repository to be saved", |_| {
        saved(&config_dir).is_some_and(|config| config.repository == other)
    });
    assert!(!saved(&config_dir).unwrap().check_updates);
}

#[test]
fn update_notice_is_in_the_first_run_dialog_and_works() {
    let tmp = tempfile::tempdir().unwrap();
    let mut env = env(&tmp);
    env.check_update = Some(newer);
    env.installed_by_installer = || true;
    let mut harness = harness(env);
    wait_for_notice(&mut harness);
    assert!(shows(&harness, "Welcome to salvajocs"));
    assert_eq!(
        harness.query_all_by_label("Update now").count(),
        1,
        "the top bar does not repeat the dialog's button"
    );

    harness.get_by_label("Update now").click();
    // The fake release has no installer asset, so the download fails at once.
    // Under the dialog's update line, not in the top bar that the backdrop dims.
    run_until(&mut harness, "the failure in the dialog", |h| {
        let notice = h.get_by_label_contains("Update available").rect().min.y;
        h.query_all_by_label_contains("The update failed")
            .any(|node| node.rect().min.y > notice)
    });
}
