use std::sync::{Mutex, mpsc};
use std::time::Duration;

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::config::{AppConfig, PasswordMode};
use salvajocs_core::crossbuild::{Confidence, MappingEntry};
use salvajocs_core::layout::{SNAPSHOT_FORMAT, SnapshotMeta};
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::registry::{RegData, RegValue, RegistryExport};
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
        wineserver_running: salvajocs_core::prefix::wineserver_running,
        check_update: None,
        installed_by_installer: || false,
        run_installer: |_| panic!("tests never start an installer"),
    });
    state.config = Some(AppConfig {
        repository: String::new(),
        repository_options: Vec::new(),
        password: PasswordMode::None,
        roots: Vec::new(),
        custom_games: Vec::new(),
        check_updates: true,
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

/// The listing is the one action before and after it runs, and an empty repository says so.
#[test]
fn an_empty_repository_says_it_has_no_backups_yet() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = Harness::new_ui_state(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (RestoreTab::default(), state(dir.path())),
    );
    harness.step();
    assert_eq!(harness.query_all_by_label("Load backups").count(), 1);
    assert!(harness.query_by_label("Restore files").is_none());

    harness.get_by_label("Load backups").click();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while harness.state().0.repository.is_none() || harness.state().0.listing.is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "the listing never ended"
        );
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.step();

    assert_eq!(harness.query_all_by_label("Load backups").count(), 1);
    let empty = harness.query_all_by_label_contains("has no backups yet");
    assert_eq!(empty.count(), 1);
}

/// A fetched game from `storage` with no files and no targets.
fn fetched(storage: &SharedStorage) -> Arc<Fetched> {
    Arc::new(Fetched {
        storage: storage.clone(),
        title: "Mix".into(),
        staged: StagedGame {
            root: PathBuf::from("/staged"),
            meta: SnapshotMeta {
                format: SNAPSHOT_FORMAT,
                title: "Mix".into(),
                roots: Vec::new(),
            },
            files: Vec::new(),
            registry: None,
        },
        targets: Targets {
            linux: None,
            win: None,
        },
        vars: GameVars::new("Mix", Vec::new(), &[], None),
        entry: None,
        pairs: Vec::new(),
        skips: Vec::new(),
    })
}

fn unknown_root() -> SnapshotRoot {
    (Base::WinAppData, "Mix".into())
}

/// The tab with the mapping dialog open over one unknown root of a game from `storage`.
fn with_dialog(storage: &SharedStorage) -> RestoreTab {
    let entry = MappingEntry {
        root: unknown_root(),
        files: 1,
        proposed: None,
        confidence: Confidence::Unknown,
        candidates: Vec::new(),
        notes: Vec::new(),
    };
    RestoreTab {
        mapping: Some((fetched(storage), Dialog::new(vec![entry]))),
        ..RestoreTab::default()
    }
}

/// A confirm job that hands back a plan for the unknown root once `hold` is released.
fn confirm_job(
    ctx: &egui::Context,
    hold: mpsc::Receiver<()>,
    storage: &SharedStorage,
) -> Job<Prepared> {
    let fetched = fetched(storage);
    Job::spawn(ctx, move || {
        let _ = hold.recv();
        Ok(Prepared {
            fetched,
            choices: vec![(unknown_root(), Choice::Template("<xdgData>/Mix".into()))],
            plan: Ok(RestorePlan {
                writes: Vec::new(),
                registry: None,
                skipped: Vec::new(),
                cross_build: true,
                notes: Vec::new(),
            }),
        })
    })
}

type TabHarness = Harness<'static, (RestoreTab, State)>;

/// The tab built by `tab` over the open repository of a state in `dir`.
fn tab_harness(dir: &Path, tab: impl FnOnce(&SharedStorage) -> RestoreTab) -> TabHarness {
    let state = state(dir);
    let tab = tab(&state.session.as_ref().unwrap().storage);
    Harness::new_ui_state(|ui, (tab, state)| tab.ui(ui, state, false), (tab, state))
}

fn storage(harness: &TabHarness) -> SharedStorage {
    harness.state().1.session.as_ref().unwrap().storage.clone()
}

/// Runs frames until the tab's preview or confirm job is done.
fn run_until_prepared(harness: &mut TabHarness) {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while harness.state().0.preparing.is_some() {
        assert!(std::time::Instant::now() < deadline, "the job never ended");
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Runs frames until the tab's restore job is done.
fn run_until_restored(harness: &mut TabHarness) {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while harness.state().0.restoring.is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "the restore never ended"
        );
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_dialog_waits_while_its_confirm_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), with_dialog);
    let (release, hold) = mpsc::channel();
    let job = confirm_job(&harness.ctx.clone(), hold, &storage(&harness));
    harness.state_mut().0.preparing = Some((0, job));
    harness.step();
    harness.step();

    assert!(is_disabled(&harness, "Cancel"));
    assert!(is_disabled(&harness, "Restore into a Wine prefix instead"));
    drop(release);
}

/// Confirms a choice in the dialog; with `close`, the dialog closes while the job runs, as
/// Cancel or a change of targets does.
fn confirm(close: bool) -> (RestoreTab, State) {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), with_dialog);
    let (release, hold) = mpsc::channel();
    let job = confirm_job(&harness.ctx.clone(), hold, &storage(&harness));
    harness.state_mut().0.preparing = Some((0, job));
    harness.step();
    if close {
        harness.state_mut().0.forget_plan();
    }
    drop(release);
    run_until_prepared(&mut harness);
    harness.into_state()
}

#[test]
fn a_confirm_ending_after_the_dialog_closed_is_dropped() {
    let (tab, state) = confirm(false);
    assert!(tab.preview.is_some());
    let games = &state.config.as_ref().unwrap().custom_games;
    assert_eq!(games.len(), 1, "the choice is saved");

    let (tab, state) = confirm(true);
    assert!(tab.preview.is_none());
    assert!(tab.mapping.is_none());
    let games = &state.config.as_ref().unwrap().custom_games;
    assert!(games.is_empty(), "nothing is saved: {games:?}");
}

#[test]
fn a_new_repository_clears_the_list_and_the_plan() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), |storage| RestoreTab {
        repository: Some(storage.clone()),
        games: vec![GameBackups {
            slug: "mix".into(),
            title: "Mix".into(),
            snapshots: Vec::new(),
            default: None,
        }],
        selected: Some("mix".into()),
        ..with_dialog(storage)
    });
    harness.step();
    assert_eq!(harness.state().0.games.len(), 1);

    // Settings opens another repository.
    let other = tempfile::tempdir().unwrap();
    harness.state_mut().1.session = state(other.path()).session;
    harness.step();

    let tab = &harness.state().0;
    assert!(tab.games.is_empty());
    assert!(tab.selected.is_none());
    assert!(tab.mapping.is_none());
}

#[test]
fn a_repository_switch_keeps_a_running_restore_and_drops_a_running_preview() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), |storage| RestoreTab {
        repository: Some(storage.clone()),
        ..RestoreTab::default()
    });
    let ctx = harness.ctx.clone();
    let (release_preview, hold) = mpsc::channel();
    let preview = confirm_job(&ctx, hold, &storage(&harness));
    let (release_restore, hold) = mpsc::channel::<()>();
    let restore = Job::spawn(&ctx, move || {
        let _ = hold.recv();
        Ok(Restored {
            title: "Mix".into(),
            safety_backup: "Safety backup: snapshot 12345678 created".into(),
            lines: vec![("/target: failed: disk full".into(), true)],
        })
    });
    let tab = &mut harness.state_mut().0;
    tab.preparing = Some((tab.generation, preview));
    tab.restoring = Some(restore);
    harness.step();

    let other = tempfile::tempdir().unwrap();
    harness.state_mut().1.session = state(other.path()).session;
    harness.step();
    assert!(is_disabled(&harness, "Load backups"), "the jobs still run");
    drop((release_preview, release_restore));
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while harness.state().0.busy() {
        assert!(std::time::Instant::now() < deadline, "the jobs never ended");
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.step();

    assert!(
        harness.state().0.preview.is_none(),
        "the old preview is dropped"
    );
    assert!(harness.query_by_label("Restored Mix").is_some());
    assert!(
        harness
            .query_by_label("/target: failed: disk full")
            .is_some()
    );
}

#[cfg(unix)]
#[test]
fn clearing_the_work_dir_removes_read_only_dirs_without_following_links() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let tmp = tempfile::tempdir().unwrap();
    let (work, outside) = (tmp.path().join("work"), tmp.path().join("outside"));
    let locked = work.join("files/Home/save");
    fs::create_dir_all(&locked).unwrap();
    fs::write(locked.join("slot.sav"), "x").unwrap();
    fs::create_dir(&outside).unwrap();
    symlink(&outside, work.join("files/link")).unwrap();
    for dir in [&locked, &outside] {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
    }

    remove_dir(&work).unwrap();

    assert!(!work.exists());
    let mode = fs::metadata(&outside).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o555, "a linked dir is left alone");
    remove_dir(&work).unwrap();
}

#[test]
fn the_plan_stays_when_a_wineserver_blocks_the_restore() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), |storage| RestoreTab {
        preview: Some(Preview {
            fetched: fetched(storage),
            plan: RestorePlan {
                writes: Vec::new(),
                registry: None,
                skipped: Vec::new(),
                cross_build: false,
                notes: Vec::new(),
            },
            overrides: Vec::new(),
            override_error: None,
        }),
        ..RestoreTab::default()
    });
    let job = Job::spawn(&harness.ctx.clone(), || {
        Err(RunError::WineserverRunning.into())
    });
    harness.state_mut().0.restoring = Some(job);
    run_until_restored(&mut harness);
    harness.step();

    let tab = &harness.state().0;
    assert!(tab.preview.is_some(), "the plan is kept for another try");
    assert!(tab.error.as_deref().unwrap().contains("Close the game"));
    assert!(!is_disabled(&harness, "Restore files"));
}

/// The tab previewing a registry restore into a prefix under `dir`, where the wineserver check
/// answers `wineserver_running`.
fn registry_restore(dir: &Path, wineserver_running: fn() -> Option<bool>) -> TabHarness {
    let prefix = dir.join("pfx");
    fs::create_dir_all(prefix.join("drive_c/users/me")).unwrap();
    fs::write(prefix.join("user.reg"), "WINE REGISTRY Version 2\n").unwrap();
    let mut export = RegistryExport::default();
    export.keys.insert(
        r"HKEY_CURRENT_USER\Software\Foo".into(),
        vec![RegValue {
            name: "v".into(),
            data: RegData::Sz("restored".into()),
        }],
    );
    let folders = WinFolders::for_prefix(&prefix, "me");
    let registry = Some((export, Context::WinePrefix { prefix, folders }));
    let mut harness = tab_harness(dir, |storage| RestoreTab {
        preview: Some(Preview {
            fetched: fetched(storage),
            plan: RestorePlan {
                writes: Vec::new(),
                registry,
                skipped: Vec::new(),
                cross_build: false,
                notes: Vec::new(),
            },
            overrides: Vec::new(),
            override_error: None,
        }),
        ..RestoreTab::default()
    });
    harness.state_mut().1.env.wineserver_running = wineserver_running;
    harness.step();
    harness
}

fn click_and_step(harness: &mut TabHarness, label: &str) {
    harness.get_by_label(label).click();
    harness.step();
    harness.step();
}

const ASK_TO_CLOSE_WINE: &str = "cannot check if Wine or Proton runs";

#[test]
fn a_sandbox_asks_to_close_wine_before_a_registry_restore_into_a_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let user_reg = dir.path().join("pfx/user.reg");
    let mut harness = registry_restore(dir.path(), || None);

    click_and_step(&mut harness, "Restore files");
    assert!(harness.query_by_label_contains(ASK_TO_CLOSE_WINE).is_some());
    click_and_step(&mut harness, "Cancel");
    assert!(harness.query_by_label_contains(ASK_TO_CLOSE_WINE).is_none());
    let tab = &harness.state().0;
    assert!(tab.restoring.is_none() && tab.restored.is_none());
    assert!(!fs::read_to_string(&user_reg).unwrap().contains("Foo"));

    click_and_step(&mut harness, "Restore files");
    click_and_step(&mut harness, "Confirm");
    run_until_restored(&mut harness);
    let tab = &harness.state().0;
    assert_eq!(tab.error, None);
    let restored = tab.restored.as_ref().expect("the restore ran");
    assert!(
        restored
            .lines
            .contains(&("Registry: restored".into(), false))
    );
    assert!(fs::read_to_string(&user_reg).unwrap().contains("Foo"));
}

#[test]
fn a_running_wineserver_on_the_host_blocks_without_asking() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = registry_restore(dir.path(), || Some(true));

    click_and_step(&mut harness, "Restore files");
    assert!(harness.query_by_label_contains(ASK_TO_CLOSE_WINE).is_none());
    run_until_restored(&mut harness);

    let tab = &harness.state().0;
    assert!(tab.restored.is_none());
    assert!(tab.error.as_deref().unwrap().contains("Close the game"));
    let user_reg = fs::read_to_string(dir.path().join("pfx/user.reg")).unwrap();
    assert!(!user_reg.contains("Foo"));
}

#[test]
fn a_sandbox_restores_without_asking_when_no_registry_goes_into_a_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = registry_restore(dir.path(), || None);
    harness
        .state_mut()
        .0
        .preview
        .as_mut()
        .unwrap()
        .plan
        .registry = None;
    // Lays out the button where the plan without the registry line puts it.
    harness.step();

    click_and_step(&mut harness, "Restore files");
    assert!(harness.query_by_label_contains(ASK_TO_CLOSE_WINE).is_none());
    run_until_restored(&mut harness);

    let tab = &harness.state().0;
    assert_eq!(tab.error, None);
    assert!(tab.restored.is_some());
}
