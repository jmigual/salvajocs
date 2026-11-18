use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use eframe::egui::Vec2;
use egui_kittest::Harness;
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::scan::Environment;
use salvajocs_core::storage::{Compression, RepoLocation};
use salvajocs_core::update::UpdateError;

use crate::env::MemorySecrets;

use super::*;

static STARTED: AtomicUsize = AtomicUsize::new(0);

fn fake_installer(_: &Path) -> Result<(), UpdateError> {
    STARTED.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// An app on a new repository in `dir`, without a password.
fn app(dir: &Path) -> App {
    fn offline(_: &Path) -> Result<Freshness, FetchError> {
        Ok(Freshness::NotModified)
    }
    let env = Env {
        config_dir: dir.join("config"),
        cache_dir: dir.join("cache"),
        secrets: Arc::new(MemorySecrets::new()),
        refresh_manifest: offline,
        // The Settings tab looks for launcher prefixes; this keeps it off the real machine.
        environment: Some(Environment {
            native: None,
            steam_roots: Vec::new(),
            launcher_prefixes: Vec::new(),
            launcher_problems: Vec::new(),
        }),
        wineserver_running: salvajocs_core::prefix::wineserver_running,
        check_update: None,
        installed_by_installer: || true,
        run_installer: fake_installer,
        rclone_remotes: || Err(std::io::ErrorKind::NotFound.into()),
        pick_folder: Arc::new(|| None),
        open_folder: Arc::new(|_| Ok(())),
    };
    let repository = dir.join("repo").to_str().unwrap().to_string();
    let location = RepoLocation {
        repository: repository.clone(),
        options: Default::default(),
    };
    Storage::init(&location, "", Compression::Default).unwrap();
    AppConfig {
        repository,
        repository_options: Vec::new(),
        password: PasswordMode::None,
        roots: Vec::new(),
        custom_games: Vec::new(),
        check_updates: true,
        ignored_prefixes: Vec::new(),
    }
    .save(&env.config_dir)
    .unwrap();
    App::new(env)
}

fn step_until(harness: &mut Harness<'static, App>, what: &str, ready: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        harness.step();
        if ready(harness.state()) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn closed(harness: &Harness<'static, App>) -> bool {
    harness
        .output()
        .viewport_output
        .values()
        .any(|viewport| viewport.commands.contains(&egui::ViewportCommand::Close))
}

#[test]
fn downloaded_update_waits_for_every_running_job() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = Harness::builder()
        .with_size(Vec2::new(1000.0, 900.0))
        .build_ui_state(
            |ui, app| {
                let mut frame = eframe::Frame::_new_kittest();
                eframe::App::ui(app, ui, &mut frame);
            },
            app(dir.path()),
        );
    step_until(&mut harness, "the repository to open", |app| {
        matches!(app.stage, Stage::Ready) && !app.busy()
    });

    let ctx = harness.ctx.clone();
    let app = harness.state_mut();
    let end_backup = app.backup.hold_backup(&ctx);
    let end_compression = app.settings.hold_compression(&ctx);
    app.updater.set_ready(PathBuf::from("setup.exe"));
    for _ in 0..3 {
        harness.step();
        assert!(!closed(&harness));
    }
    assert_eq!(STARTED.load(Ordering::SeqCst), 0, "no installer while busy");

    drop(end_backup);
    step_until(&mut harness, "the backup job to end", |app| {
        !app.backup.busy()
    });
    harness.step();
    assert_eq!(
        STARTED.load(Ordering::SeqCst),
        0,
        "the compression change still runs"
    );
    assert!(!closed(&harness));

    harness.state_mut().set_tab(Tab::Settings);
    drop(end_compression);
    step_until(&mut harness, "the installer to start", |_| {
        STARTED.load(Ordering::SeqCst) == 1
    });
    assert!(
        closed(&harness),
        "the window closes once the installer started"
    );
}

#[test]
fn the_backup_tab_finishes_its_jobs_while_another_tab_is_shown() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = Harness::builder()
        .with_size(Vec2::new(1000.0, 900.0))
        .build_ui_state(
            |ui, app| {
                let mut frame = eframe::Frame::_new_kittest();
                eframe::App::ui(app, ui, &mut frame);
            },
            app(dir.path()),
        );
    step_until(&mut harness, "the repository to open", |app| {
        app.state.session.is_some()
    });
    // The Backup tab is in front for this frame, which starts loading what to back up.
    harness.step();
    harness.state_mut().set_tab(Tab::Settings);

    step_until(&mut harness, "the backup tab to finish its jobs", |app| {
        !app.backup.busy()
    });
}
