use std::sync::{Mutex, mpsc};
use std::time::Duration;

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::config::{AppConfig, PasswordMode};
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::storage::{Compression, RepoLocation, Storage};

use super::*;
use crate::env::{Env, MemorySecrets};
use crate::state::Session;

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

#[test]
fn times_show_as_utc_dates() {
    assert_eq!(format_time(at(0)), "1970-01-01 00:00 UTC");
    assert_eq!(format_time(at(1_709_210_096)), "2024-02-29 12:34 UTC");
    assert_eq!(format_time(at(1_798_761_540)), "2026-12-31 23:59 UTC");
}

#[test]
fn a_running_wineserver_asks_to_close_wine() {
    let running = describe_run_error(&RunError::WineserverRunning.into());
    let other = describe_run_error(&anyhow!("disk full"));

    assert!(
        running.contains("Close the game and Wine first"),
        "{running}"
    );
    assert!(!other.contains("Close the game"), "{other}");
}

/// A state whose repository, in `dir`, is open.
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
    });
    state.config = Some(AppConfig {
        repository: String::new(),
        repository_options: Vec::new(),
        password: PasswordMode::None,
        roots: Vec::new(),
        custom_games: Vec::new(),
    });
    let location = RepoLocation {
        repository: dir.join("repo").to_str().unwrap().to_string(),
        options: Default::default(),
    };
    let storage = Storage::init(&location, "", Compression::Default).unwrap();
    state.session = Some(Session {
        compression: storage.compression(),
        storage: Arc::new(Mutex::new(storage)),
    });
    state
}

fn is_disabled(harness: &Harness<'_, (RestoreTab, State)>, label: &str) -> bool {
    harness.get_by_label(label).accesskit_node().is_disabled()
}

#[test]
fn listing_waits_while_a_job_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = Harness::new_ui_state(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (RestoreTab::default(), state(dir.path())),
    );
    harness.step();
    assert!(!is_disabled(&harness, "Load backups"));

    let (release, hold) = mpsc::channel::<()>();
    harness.state_mut().0.restoring = Some(Job::spawn(&harness.ctx.clone(), move || {
        let _ = hold.recv();
        Err(anyhow!("released"))
    }));
    harness.step();
    harness.step();
    assert!(is_disabled(&harness, "Load backups"));
    drop(release);
}

#[test]
fn listing_waits_for_the_manifest_to_load() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = Harness::new_ui_state(
        |ui, (tab, state)| tab.ui(ui, state, true),
        (RestoreTab::default(), state(dir.path())),
    );
    harness.step();
    assert!(is_disabled(&harness, "Load backups"));
}
