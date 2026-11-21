use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::accesskit::{Role, Toggled};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::config::{AppConfig, PasswordMode};
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::storage::{Compression, RepoLocation, Storage};

use super::*;
use crate::env::{Env, MemorySecrets};
use crate::state::{Session, SharedStorage};

const KEEP_LAST: &str = "Keep the last";

/// The section, the state it draws from, and whether it is drawn (its tab is in front).
type SectionHarness = Harness<'static, (RetentionSection, State, bool)>;

fn location(dir: &Path) -> RepoLocation {
    RepoLocation::new(
        dir.join("repo").to_str().unwrap().to_string(),
        Default::default(),
    )
}

/// The section over a new repository in `dir`.
fn harness(dir: &Path) -> SectionHarness {
    harness_on(state(dir))
}

/// The section drawn from `state`, whose jobs are polled as the app does. Building the harness
/// draws the first frame, which starts the load.
fn harness_on(state: State) -> SectionHarness {
    theme::harness(
        |ui, (section, state, shown): &mut (RetentionSection, State, bool)| {
            state.poll(ui.ctx());
            if *shown {
                section.ui(ui, state);
            }
        },
        (RetentionSection::default(), state, true),
    )
}

/// A state with a new repository in `dir` open.
fn state(dir: &Path) -> State {
    fn offline(_: &Path) -> Result<Freshness, FetchError> {
        Ok(Freshness::NotModified)
    }
    let mut state = State::new(Env {
        config_dir: dir.join("config"),
        cache_dir: dir.join("cache"),
        secrets: Arc::new(MemorySecrets::new()),
        refresh_manifest: offline,
        environment: None,
        wineserver_running: salvajocs_core::prefix::wineserver_running,
        check_update: None,
        installed_by_installer: || false,
        run_installer: |_| panic!("tests never start an installer"),
        appimage: None,
        restart_appimage: |_| panic!("tests never restart the app"),
        rclone_remotes: || Err(std::io::ErrorKind::NotFound.into()),
        pick_folder: Arc::new(|| None),
        open_folder: Arc::new(|_| Ok(())),
    });
    let storage = Storage::init(&location(dir), "", Compression::Default).unwrap();
    state.session = Some(Session {
        compression: storage.compression(),
        storage: Arc::new(Mutex::new(storage)),
        space: Default::default(),
    });
    let repository = location(dir).repository;
    state.config = Some(AppConfig::new(repository, PasswordMode::None));
    state
}

fn storage(state: &State) -> SharedStorage {
    state.session.as_ref().unwrap().storage.clone()
}

/// The policy in the repository, read by a session of its own.
fn stored(dir: &Path) -> Retention {
    retention::load(&mut Storage::open(&location(dir), "").unwrap()).unwrap()
}

fn step_until(harness: &mut SectionHarness, what: &str, ready: impl Fn(&SectionHarness) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        harness.step();
        if ready(harness) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn editable(harness: &SectionHarness) -> bool {
    !harness
        .get_by_role_and_label(Role::CheckBox, KEEP_LAST)
        .accesskit_node()
        .is_disabled()
}

fn keeps_last(harness: &SectionHarness) -> bool {
    harness
        .get_by_role_and_label(Role::CheckBox, KEEP_LAST)
        .accesskit_node()
        .toggled()
        == Some(Toggled::True)
}

#[test]
fn controls_stay_disabled_while_the_policy_loads() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let storage = storage(&state);
    let held = storage.lock().unwrap();
    let mut harness = harness_on(state);
    for _ in 0..3 {
        harness.step();
    }
    assert!(harness.state().0.loading.is_some());
    assert!(!editable(&harness));

    harness
        .get_by_role_and_label(Role::CheckBox, KEEP_LAST)
        .click();
    harness.step();
    drop(held);
    step_until(&mut harness, "the policy to load", editable);
    assert!(!keeps_last(&harness));
    assert_eq!(unsent(&harness), None);
    assert!(stored(dir.path()).is_off());
}

fn unsent(harness: &SectionHarness) -> Option<retention::Unsent> {
    harness.state().1.config.as_ref().unwrap().retention_unsent
}

fn keep_last(n: u32) -> Retention {
    let mut policy = Retention::default();
    policy.keep_last = NonZeroU32::new(n);
    policy
}

#[test]
fn an_edit_stays_on_this_pc_based_on_the_loaded_rules() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let mut other = Storage::open(&location(dir.path()), "").unwrap();
    retention::save(&mut other, &keep_last(5), &dir.path().join("stage")).unwrap();
    let mut harness = harness_on(state);
    step_until(&mut harness, "the policy to load", editable);
    assert!(keeps_last(&harness));

    harness
        .get_by_role_and_label(Role::SpinButton, KEEP_LAST)
        .set_value_accesskit(7.0);
    harness.step();
    harness
        .get_by_role_and_label(Role::SpinButton, KEEP_LAST)
        .set_value_accesskit(8.0);
    harness.step();
    // The earliest base wins.
    let expected = retention::Unsent::new(keep_last(8), keep_last(5));
    assert_eq!(unsent(&harness), Some(expected));
    assert_eq!(
        stored(dir.path()),
        keep_last(5),
        "the repository is unchanged"
    );
    assert!(
        harness
            .query_by_label("Saved to the repository with the next Back up.")
            .is_some()
    );
    let config_dir = dir.path().join("config");
    step_until(&mut harness, "the config to be saved", |_| {
        AppConfig::load(&config_dir)
            .ok()
            .flatten()
            .is_some_and(|config| config.retention_unsent == Some(expected))
    });
}

#[test]
fn the_unsent_rules_show_over_the_loaded_ones() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    let unsent = retention::Unsent::new(keep_last(3), Retention::default());
    state.config.as_mut().unwrap().retention_unsent = Some(unsent);
    let mut harness = harness_on(state);
    step_until(&mut harness, "the policy to load", editable);
    assert!(keeps_last(&harness));
    let shown = harness
        .get_by_role_and_label(Role::SpinButton, KEEP_LAST)
        .accesskit_node()
        .numeric_value();
    assert_eq!(shown, Some(3.0));
    assert!(stored(dir.path()).is_off());
}

#[test]
fn a_failed_load_can_be_retried() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let storage = storage(&state);
    // A poisoned lock fails the load the way an unreachable repository would.
    let poisoner = storage.clone();
    let _ = std::thread::spawn(move || {
        let _held = poisoner.lock().unwrap();
        panic!("poison the repository lock");
    })
    .join();
    let mut harness = harness_on(state);
    step_until(&mut harness, "the load to fail", |h| {
        h.query_by_label("Retry").is_some()
    });
    assert!(!editable(&harness));

    storage.clear_poison();
    harness.get_by_label("Retry").click();
    step_until(&mut harness, "the policy to load", editable);
}

#[test]
fn showing_the_tab_again_reloads_the_policy() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(dir.path());
    step_until(&mut harness, "the policy to load", editable);
    assert!(!keeps_last(&harness));

    // Another machine changes the policy while the tab is hidden.
    let mut policy = Retention::default();
    policy.keep_last = NonZeroU32::new(3);
    let mut other = Storage::open(&location(dir.path()), "").unwrap();
    retention::save(&mut other, &policy, &dir.path().join("stage")).unwrap();
    harness.state_mut().2 = false;
    harness.step();
    harness.state_mut().2 = true;
    step_until(&mut harness, "the new policy", |h| {
        editable(h) && keeps_last(h)
    });
}

#[test]
fn unsent_rules_over_a_newer_versions_can_be_discarded() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    // Rules a newer salvajocs stored, which this PC's can never overwrite.
    let staged = dir.path().join("newer");
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::write(staged.join("retention.json"), r#"{ "format": 2 }"#).unwrap();
    let tags = ["salvajocs-settings", "settings:retention"];
    let mut other = Storage::open(&location(dir.path()), "").unwrap();
    let written = other.backup_dir(&staged, "/salvajocs-settings/retention", &tags, "Rules");
    written.unwrap().unwrap();
    let changed = retention::Unsent::new(keep_last(3), Retention::default());
    state.config.as_mut().unwrap().retention_unsent = Some(changed);
    let mut harness = harness_on(state);
    step_until(&mut harness, "the load to fail", |h| {
        h.query_by_label_contains("could not be read").is_some()
    });
    assert!(!editable(&harness));

    harness.get_by_label("Discard this PC's changes").click();
    harness.step();
    assert_eq!(unsent(&harness), None);
    harness.step();
    assert!(
        harness
            .query_by_label("Discard this PC's changes")
            .is_none()
    );
}

#[test]
fn discarding_shows_the_loaded_rules_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    let changed = retention::Unsent::new(keep_last(3), Retention::default());
    state.config.as_mut().unwrap().retention_unsent = Some(changed);
    let mut harness = harness_on(state);
    step_until(&mut harness, "the policy to load", editable);
    assert!(keeps_last(&harness));

    // Not while a Back up may be sending these rules.
    harness.state_mut().0.backing_up = true;
    harness.step();
    let button = harness.get_by_label("Discard this PC's changes");
    assert!(button.accesskit_node().is_disabled());
    harness.state_mut().0.backing_up = false;
    harness.step();

    harness.get_by_label("Discard this PC's changes").click();
    harness.step();
    harness.step();
    assert!(!keeps_last(&harness), "the repository keeps every backup");
    assert_eq!(unsent(&harness), None);
}
