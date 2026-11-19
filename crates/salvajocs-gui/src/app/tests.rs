use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use eframe::egui::Vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::scan::Environment;
use salvajocs_core::storage::{Compression, RepoLocation};
use salvajocs_core::update::{Release, UpdateError};

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

/// Checks the top bar of an app in the smallest window, after `setup` changed its updater: the
/// window buttons stay where they are without an update, and no text of the bar overlaps
/// another or the buttons. `setup` may return what keeps its update state alive.
fn assert_bar_fits_after(
    setup: impl FnOnce(&mut Updater, &egui::Context) -> Option<std::sync::mpsc::Sender<()>>,
) {
    let dir = tempfile::tempdir().unwrap();
    let [width, height] = crate::MIN_WINDOW_SIZE;
    let mut harness = Harness::builder()
        .with_size(Vec2::new(width, height))
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
    harness.step();
    let buttons = |harness: &Harness<'static, App>| {
        ["Minimize", "Maximize", "Close"].map(|label| harness.get_by_label(label).rect())
    };
    let resting = buttons(&harness);

    let ctx = harness.ctx.clone();
    let _held = setup(&mut harness.state_mut().updater, &ctx);
    for _ in 0..3 {
        harness.step();
    }
    assert_eq!(buttons(&harness), resting, "the window buttons stay put");
    let mut items: Vec<_> = Tab::ALL
        .iter()
        .map(|tab| tab.label())
        .chain([
            "Update available",
            "Downloading",
            "Update ready",
            "Manifest",
        ])
        .flat_map(|label| {
            harness
                .query_all_by_label_contains(label)
                .map(|node| (label, node.rect()))
                .collect::<Vec<_>>()
        })
        // Not the page's own text, which the bar's tab labels are also found in.
        .filter(|(_, rect)| rect.top() < 48.0)
        .collect();
    assert!(
        items.iter().any(|(label, _)| *label == "Update available"),
        "the notice is shown"
    );
    items.push(("the window buttons", resting[0]));
    items.sort_by(|a, b| a.1.left().total_cmp(&b.1.left()));
    for pair in items.windows(2) {
        assert!(
            pair[0].1.right() <= pair[1].1.left(),
            "{} and {} overlap",
            pair[0].0,
            pair[1].0
        );
    }
}

fn release() -> Release {
    Release {
        version: "99.0.0".to_string(),
        html_url: "https://example.invalid/releases/tag/v99.0.0".to_string(),
        assets: Vec::new(),
    }
}

#[test]
fn the_bar_fits_while_an_update_downloads() {
    assert_bar_fits_after(|updater, ctx| {
        updater.offer(release(), true);
        Some(updater.hold_download(ctx))
    });
}

#[test]
fn the_bar_fits_while_a_downloaded_update_waits() {
    assert_bar_fits_after(|updater, _| {
        updater.offer(release(), true);
        updater.set_ready(PathBuf::from("setup.exe"));
        None
    });
}
