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

static RESTARTED: AtomicUsize = AtomicUsize::new(0);

fn fake_restart(_: &Path) -> Result<(), UpdateError> {
    RESTARTED.fetch_add(1, Ordering::SeqCst);
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
        environment: Some(Environment::default()),
        wineserver_running: salvajocs_core::prefix::wineserver_running,
        check_update: None,
        installed_by_installer: || true,
        run_installer: fake_installer,
        appimage: None,
        restart_appimage: fake_restart,
        rclone_remotes: || Err(std::io::ErrorKind::NotFound.into()),
        pick_folder: Arc::new(|| None),
        open_folder: Arc::new(|_| Ok(())),
    };
    let repository = dir.join("repo").to_str().unwrap().to_string();
    let location = RepoLocation::new(repository.clone(), Default::default());
    Storage::init(&location, "", Compression::Default).unwrap();
    AppConfig::new(repository, PasswordMode::None)
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
fn replaced_appimage_starts_on_exit_after_the_jobs_end() {
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
    app.updater
        .set_restarting(dir.path().join("salvajocs.AppImage"));
    for _ in 0..3 {
        harness.step();
        assert!(!closed(&harness));
    }

    drop(end_backup);
    // The jobs the backup's end starts hold the close too.
    let deadline = Instant::now() + Duration::from_secs(60);
    while !closed(&harness) {
        assert!(Instant::now() < deadline, "timed out waiting for the close");
        assert_eq!(RESTARTED.load(Ordering::SeqCst), 0);
        harness.step();
    }
    assert_eq!(RESTARTED.load(Ordering::SeqCst), 0);
    eframe::App::on_exit(harness.state_mut(), None);
    assert_eq!(RESTARTED.load(Ordering::SeqCst), 1);
}

static RESTARTED_AFTER_WAIT: AtomicUsize = AtomicUsize::new(0);

#[test]
fn restart_whose_close_was_cancelled_waits_for_the_job() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = ready_app(dir.path());
    harness.state_mut().state.env.restart_appimage = |_| {
        RESTARTED_AFTER_WAIT.fetch_add(1, Ordering::SeqCst);
        Ok(())
    };
    harness
        .state_mut()
        .updater
        .set_restarting(dir.path().join("salvajocs.AppImage"));
    let deadline = Instant::now() + Duration::from_secs(60);
    while !closed(&harness) {
        assert!(Instant::now() < deadline, "timed out waiting for the close");
        harness.step();
    }

    // A job started before the close arrived: the user picks Wait.
    let ctx = harness.ctx.clone();
    let end_backup = harness.state_mut().backup.hold_backup(&ctx);
    request_close(&mut harness);
    assert!(sent(&harness, egui::ViewportCommand::CancelClose));
    eframe::App::on_exit(harness.state_mut(), None);
    assert_eq!(
        RESTARTED_AFTER_WAIT.load(Ordering::SeqCst),
        0,
        "a later quit does not restart"
    );

    drop(end_backup);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !closed(&harness) {
        assert!(Instant::now() < deadline, "timed out waiting for the close");
        harness.step();
    }
    eframe::App::on_exit(harness.state_mut(), None);
    assert_eq!(RESTARTED_AFTER_WAIT.load(Ordering::SeqCst), 1);
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

/// Checks the bars of an app in the smallest window, after `setup` changed its updater: the
/// window buttons stay where they are without an update, and no text of the bottom bar overlaps
/// another or leaves the window. `setup` may return what keeps its update state alive.
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
    let window = harness.ctx.viewport_rect();
    let mut items: Vec<_> = [
        "Update available",
        "Downloading",
        "Update ready",
        "Manifest",
    ]
    .into_iter()
    .flat_map(|label| {
        harness
            .query_all_by_label_contains(label)
            .map(|node| (label, node.rect()))
            .collect::<Vec<_>>()
    })
    .collect();
    assert!(
        items.iter().any(|(label, _)| *label == "Update available"),
        "the notice is shown"
    );
    for (label, rect) in &items {
        assert!(
            window.contains_rect(*rect) && rect.top() > window.center().y,
            "{label} is in the bottom bar: {rect:?}"
        );
    }
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
    Release::new(
        "99.0.0".to_string(),
        "https://example.invalid/releases/tag/v99.0.0".to_string(),
        Vec::new(),
    )
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

/// An app with its repository open, driven as eframe does: `logic`, then `ui`, every frame.
fn ready_app(dir: &Path) -> Harness<'static, App> {
    driven(dir, true)
}

/// An app with its repository open that runs `logic` before `ui` only if `with_logic`.
fn driven(dir: &Path, with_logic: bool) -> Harness<'static, App> {
    let mut harness = Harness::builder()
        .with_size(Vec2::new(1000.0, 900.0))
        .build_ui_state(
            move |ui, app| {
                let mut frame = eframe::Frame::_new_kittest();
                if with_logic {
                    eframe::App::logic(app, ui.ctx(), &mut frame);
                }
                eframe::App::ui(app, ui, &mut frame);
            },
            app(dir),
        );
    step_until(&mut harness, "the repository to open", |app| {
        matches!(app.stage, Stage::Ready) && !app.busy()
    });
    harness
}

/// Steps one frame in which the window is asked to close, as the OS or the title bar's Close
/// does.
fn request_close(harness: &mut Harness<'static, App>) {
    harness
        .input_mut()
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    harness.step();
}

/// The commands of a `logic` pass alone, as eframe runs for a minimized window, with a close
/// request if `close`.
fn logic_only(harness: &mut Harness<'static, App>, close: bool) -> Vec<egui::ViewportCommand> {
    let ctx = harness.ctx.clone();
    let mut input = egui::RawInput::default();
    if close {
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
    }
    let app = harness.state_mut();
    let mut frame = eframe::Frame::_new_kittest();
    let mut output = ctx.run_logic(&input, |ctx| eframe::App::logic(app, ctx, &mut frame));
    output
        .viewport_commands
        .remove(&egui::ViewportId::ROOT)
        .unwrap_or_default()
}

fn sent(harness: &Harness<'static, App>, command: egui::ViewportCommand) -> bool {
    harness
        .output()
        .viewport_output
        .values()
        .any(|viewport| viewport.commands.contains(&command))
}

const RUNNING: &str = "A job is still running";

#[test]
fn closing_without_a_job_is_not_asked_about() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = ready_app(dir.path());
    request_close(&mut harness);
    assert!(!sent(&harness, egui::ViewportCommand::CancelClose));
    assert!(harness.query_by_label(RUNNING).is_none());
}

#[test]
fn closing_during_a_job_asks_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = ready_app(dir.path());
    let ctx = harness.ctx.clone();
    let _end_backup = harness.state_mut().backup.hold_backup(&ctx);
    request_close(&mut harness);
    assert!(sent(&harness, egui::ViewportCommand::CancelClose));
    assert!(sent(&harness, egui::ViewportCommand::Minimized(false)));
    harness.step();
    harness.get_by_label(RUNNING);
    assert!(!closed(&harness));
}

#[test]
fn closing_a_minimized_window_during_a_job_asks_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = ready_app(dir.path());
    let ctx = harness.ctx.clone();
    let _end_backup = harness.state_mut().backup.hold_backup(&ctx);
    let commands = logic_only(&mut harness, true);
    assert!(commands.contains(&egui::ViewportCommand::CancelClose));
    assert!(commands.contains(&egui::ViewportCommand::Minimized(false)));
}

#[test]
fn closing_a_minimized_window_after_the_job_ended_is_not_asked_about() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = ready_app(dir.path());
    let ctx = harness.ctx.clone();
    let end_backup = harness.state_mut().backup.hold_backup(&ctx);
    drop(end_backup);
    // No frame runs while minimized, so only `logic` can notice that the job is over.
    let deadline = Instant::now() + Duration::from_secs(60);
    while !harness.state_mut().backup.run_ended() {
        assert!(Instant::now() < deadline, "the backup job never ended");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(logic_only(&mut harness, true).is_empty());
}

#[test]
fn closing_while_a_repository_is_created_asks_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = ready_app(dir.path());
    let ctx = harness.ctx.clone();
    let _end_creation = harness.state_mut().settings.hold_finishing(&ctx);
    request_close(&mut harness);
    assert!(sent(&harness, egui::ViewportCommand::CancelClose));
}

#[test]
fn a_close_request_in_the_frame_a_job_starts_is_asked_about() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = driven(dir.path(), false);
    let ctx = harness.ctx.clone();
    let _end_backup = harness.state_mut().backup.hold_backup(&ctx);
    request_close(&mut harness);
    assert!(sent(&harness, egui::ViewportCommand::CancelClose));
}

#[test]
fn reading_jobs_do_not_hold_the_close() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = ready_app(dir.path());
    let ctx = harness.ctx.clone();
    let (_end_load, load) = crate::job::held(&ctx);
    harness.state_mut().manifest = Some(load);
    assert!(harness.state().busy());
    request_close(&mut harness);
    assert!(!sent(&harness, egui::ViewportCommand::CancelClose));
    assert!(harness.query_by_label(RUNNING).is_none());
}

#[test]
fn a_job_that_ended_on_another_tab_does_not_hold_the_close() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = ready_app(dir.path());
    let ctx = harness.ctx.clone();
    let end_compression = harness.state_mut().settings.hold_compression(&ctx);
    harness.step();
    drop(end_compression);
    step_until(&mut harness, "the compression job to end", |app| {
        !app.settings.busy()
    });
    request_close(&mut harness);
    assert!(!sent(&harness, egui::ViewportCommand::CancelClose));
}

#[test]
fn close_anyway_closes_despite_the_job() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = ready_app(dir.path());
    let ctx = harness.ctx.clone();
    let _end_backup = harness.state_mut().backup.hold_backup(&ctx);
    request_close(&mut harness);
    harness.step();
    harness.get_by_label("Close anyway").click();
    harness.step();
    assert!(closed(&harness));
    // eframe turns the Close command into a close request, which must not ask again.
    request_close(&mut harness);
    assert!(!sent(&harness, egui::ViewportCommand::CancelClose));
}

#[test]
fn waiting_keeps_the_window_until_the_job_ends() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = ready_app(dir.path());
    let ctx = harness.ctx.clone();
    let end_backup = harness.state_mut().backup.hold_backup(&ctx);
    request_close(&mut harness);
    harness.step();
    harness.get_by_label("Wait").click();
    // The click ends the dialog in this frame, which still draws it.
    harness.run_steps(2);
    assert!(harness.query_by_label(RUNNING).is_none());
    assert!(!closed(&harness));

    drop(end_backup);
    step_until(&mut harness, "the backup job to end", |app| {
        !app.backup.busy()
    });
    request_close(&mut harness);
    assert!(!sent(&harness, egui::ViewportCommand::CancelClose));
    assert!(harness.query_by_label(RUNNING).is_none());
}

#[test]
fn the_dialog_goes_away_when_the_job_ends() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = ready_app(dir.path());
    let ctx = harness.ctx.clone();
    let end_backup = harness.state_mut().backup.hold_backup(&ctx);
    request_close(&mut harness);
    harness.step();
    harness.get_by_label(RUNNING);

    drop(end_backup);
    step_until(&mut harness, "the dialog to go", |app| !app.confirm_close);
    harness.step();
    assert!(harness.query_by_label(RUNNING).is_none());
}
