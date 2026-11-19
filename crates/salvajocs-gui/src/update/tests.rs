use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::update::UpdateError;

use crate::env::{Env, MemorySecrets};
use crate::theme;

use super::*;

static STARTED: AtomicUsize = AtomicUsize::new(0);

fn fake_installer(_: &Path) -> Result<(), UpdateError> {
    STARTED.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

static RESTARTED: AtomicUsize = AtomicUsize::new(0);

fn fake_restart(_: &Path) -> Result<(), UpdateError> {
    RESTARTED.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

fn state(dir: &Path) -> State {
    fn offline(_: &Path) -> Result<Freshness, FetchError> {
        Ok(Freshness::NotModified)
    }
    State::new(Env {
        config_dir: dir.join("config"),
        cache_dir: dir.join("cache"),
        secrets: Arc::new(MemorySecrets::new()),
        refresh_manifest: offline,
        environment: None,
        wineserver_running: salvajocs_core::prefix::wineserver_running,
        check_update: None,
        installed_by_installer: || true,
        run_installer: fake_installer,
        appimage: None,
        restart_appimage: |_| panic!("tests never restart the app"),
        rclone_remotes: || Err(std::io::ErrorKind::NotFound.into()),
        pick_folder: Arc::new(|| None),
        open_folder: Arc::new(|_| Ok(())),
    })
}

fn closed(harness: &Harness<'_, (Updater, State, bool)>) -> bool {
    harness
        .output()
        .viewport_output
        .values()
        .any(|viewport| viewport.commands.contains(&egui::ViewportCommand::Close))
}

#[test]
fn downloaded_update_waits_for_backup_or_restore_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
        |ui, (updater, state, busy)| {
            updater.poll(state);
            updater.ui(ui, state);
            updater.install_when_idle(ui.ctx(), state, *busy);
        },
        (Updater::default(), state(dir.path()), true),
    );
    let ctx = harness.ctx.clone();
    let updater = &mut harness.state_mut().0;
    updater.installed = true;
    updater.release = Some(Release::new(
        "99.0.0".to_string(),
        "https://example.invalid/releases/tag/v99.0.0".to_string(),
        Vec::new(),
    ));
    updater.downloading = Some(Job::spawn(&ctx, || Ok(PathBuf::from("setup.exe"))));

    let deadline = Instant::now() + Duration::from_secs(60);
    while harness.state().0.downloading.is_some() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the download"
        );
        harness.step();
    }
    harness.step();
    assert!(
        harness
            .query_by_label_contains("installs when the running task finishes")
            .is_some()
    );
    assert_eq!(STARTED.load(Ordering::SeqCst), 0, "no installer while busy");
    assert!(!closed(&harness), "the window stays open while busy");

    harness.state_mut().2 = false;
    harness.step();
    assert_eq!(STARTED.load(Ordering::SeqCst), 1);
    assert!(
        closed(&harness),
        "the window closes once the installer started"
    );
}

#[test]
fn replaced_appimage_restarts_once_idle_and_closed() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    state.env.restart_appimage = fake_restart;
    state.env.run_installer = |_| panic!("no installer");
    let mut harness = theme::harness(
        |ui, (updater, state, busy)| {
            updater.poll(state);
            updater.ui(ui, state);
            updater.install_when_idle(ui.ctx(), state, *busy);
        },
        (Updater::default(), state, true),
    );
    let ctx = harness.ctx.clone();
    let updater = &mut harness.state_mut().0;
    let appimage = dir.path().join("salvajocs.AppImage");
    updater.appimage = Some(appimage.clone());
    updater.release = Some(Release::new(
        "99.0.0".to_string(),
        "https://example.invalid/releases/tag/v99.0.0".to_string(),
        Vec::new(),
    ));
    updater.downloading = Some(Job::spawn(&ctx, move || Ok(appimage)));

    let deadline = Instant::now() + Duration::from_secs(60);
    while harness.state().0.downloading.is_some() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the download"
        );
        harness.step();
    }
    harness.step();
    harness.get_by_label("Restart now").click();
    harness.step();
    harness.step();
    assert!(
        harness
            .query_by_label_contains("Restarts when the running task finishes")
            .is_some()
    );
    assert!(!closed(&harness), "the window stays open while busy");

    harness.state_mut().2 = false;
    harness.step();
    assert!(closed(&harness), "the window closes once idle");
    assert_eq!(
        RESTARTED.load(Ordering::SeqCst),
        0,
        "the new AppImage starts on exit, after the last writes"
    );
    let (updater, state, _) = harness.state_mut();
    updater.restart(state);
    updater.restart(state);
    assert_eq!(RESTARTED.load(Ordering::SeqCst), 1);
}
