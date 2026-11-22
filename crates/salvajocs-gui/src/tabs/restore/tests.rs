use std::fs;
use std::path::Path;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::anyhow;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::backup::backup_games;
use salvajocs_core::config::{AppConfig, PasswordMode};
use salvajocs_core::crossbuild::{Choice, Confidence, MappingEntry, SnapshotRoot};
use salvajocs_core::launcher::{Launcher, LauncherPrefix};
use salvajocs_core::layout::{
    Base, Context, GameVars, LinuxFolders, PrefixOrigin, SNAPSHOT_FORMAT, SnapshotMeta, WinFolders,
};
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::manifest::{CustomGame, Integration, parse_yaml};
use salvajocs_core::registry::{RegData, RegValue, RegistryExport};
use salvajocs_core::restore::{
    self, GameInfo, RestorePlan, RestoreReport, RunError, SafetyBackup, StagedGame, Targets, Write,
};
use salvajocs_core::retention::{self, DeleteError};
use salvajocs_core::scan::{Environment, FoundFile, GameScan};
use salvajocs_core::storage::{Compression, RepoLocation, SnapshotInfo, Storage};

use super::delete::describe_delete_error;
use super::dirs::own_dir;
use super::plan::{Fetched, Prepared, Preview};
use super::run::{Restored, describe_run_error};
use super::*;
use crate::env::{Env, MemorySecrets};
use crate::state::Session;
use crate::theme;

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
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

#[test]
fn a_restore_report_shows_a_line_per_write_and_for_the_registry() {
    let write = |target: &str| {
        restore::Write::new(
            PathBuf::from("/staged/save.dat"),
            PathBuf::from(target),
            None,
            None,
        )
    };
    let plan = RestorePlan::new(
        vec![write("/saves/a.dat"), write("/saves/b.dat")],
        None,
        Vec::new(),
        false,
        Vec::new(),
    );
    let report = RestoreReport::new(
        SafetyBackup::NothingExisted,
        vec![Ok(()), Err(restore::ApplyError::TargetIsSymlink)],
        Some(Err(restore::ApplyError::NoRegistry)),
    );

    let restored = Restored::new("Mix".into(), &plan, report);

    assert!(restored.safety_backup.contains("none needed"));
    let failed: Vec<bool> = restored.lines.iter().map(|(_, failed)| *failed).collect();
    assert_eq!(failed, [false, true, true]);
    let a = Path::new("/saves/a.dat").display().to_string();
    assert!(restored.lines[0].0.starts_with(&a), "{:?}", restored.lines);
    assert!(restored.lines[2].0.starts_with("Registry"));
}

#[test]
fn a_failed_delete_says_how_many_it_removed() {
    let stopped = |forgotten| {
        let source = salvajocs_core::storage::StorageError::Backend("gone".into());
        let err = retention::CleanupError::new(forgotten, source);
        describe_delete_error(&DeleteError::Stopped(err).into())
    };
    let refused = describe_delete_error(&DeleteError::NotAGameBackup("abc".into()).into());

    assert!(
        stopped(2).starts_with("Removed 2 backups"),
        "{}",
        stopped(2)
    );
    // The cleanup's own count of 0 would repeat what the start says.
    let none = stopped(0);
    assert!(none.starts_with("Nothing was removed"), "{none}");
    assert!(!none.contains("cleanup"), "{none}");
    assert!(refused.contains("out of date"), "{refused}");
}

#[test]
fn a_cleanup_dropping_the_list_closes_the_delete_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), with_mix);
    harness.step();
    harness.get_by_label("Delete all backups of Mix").click();
    harness.step();
    assert!(harness.state().0.confirm_delete.is_some());

    harness.state_mut().0.drop_listing();
    harness.step();
    assert!(harness.state().0.confirm_delete.is_none());
    assert!(harness.query_by_label("Cancel").is_none());
}

#[test]
fn a_running_backup_disables_every_delete_and_leaves_the_list_browsable() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), with_mix);
    harness.step();
    harness.get_by_label("Delete all backups of Mix").click();
    harness.step();
    harness.step();
    assert!(!is_disabled(&harness, "Delete"));

    harness.state_mut().0.set_backing_up(true);
    harness.step();
    harness.step();
    assert!(is_disabled(&harness, "Delete"), "the confirm's button");
    let reason = harness.query_all_by_label_contains("Waiting for the running backup");
    assert!(reason.count() > 0);

    harness.get_by_label("Cancel").click();
    harness.step();
    harness.step();
    assert!(is_disabled(&harness, "Delete all backups of Mix"));
    assert!(is_disabled(&harness, "Load backups"));
    let mut rows = harness.query_all_by_label("Mix").peekable();
    assert!(rows.peek().is_some());
    assert!(
        rows.all(|row| !row.accesskit_node().is_disabled()),
        "the game list stays usable"
    );
}

/// A listing that fails after a restore or delete drops the game to choose again, so a later
/// listing does not choose it.
#[test]
fn a_failed_relisting_forgets_the_game_to_choose_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), with_mix);
    let ctx = harness.ctx.clone();
    {
        let tab = &mut harness.state_mut().0;
        tab.selected = None;
        tab.reselect = Some("mix".into());
        tab.listing = Some(Job::spawn(&ctx, || Err(anyhow!("cannot list"))));
    }
    step_until(&mut harness, "the listing to end", |h| {
        h.state().0.listing.is_none()
    });

    let tab = &harness.state().0;
    assert_eq!(tab.reselect, None);
    assert_eq!(tab.selected, None);
    assert!(tab.error.is_some());
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
        appimage: None,
        restart_appimage: |_| panic!("tests never restart the app"),
        rclone_remotes: || Err(std::io::ErrorKind::NotFound.into()),
        pick_folder: Arc::new(|| None),
        open_folder: Arc::new(|_| Ok(())),
    });
    state.config = Some(AppConfig::new(String::new(), PasswordMode::None));
    let location = RepoLocation::new(
        dir.join("repo").to_str().unwrap().to_string(),
        Default::default(),
    );
    let storage = Storage::init(&location, "", Compression::Default).unwrap();
    state.session = Some(Session {
        compression: storage.compression(),
        storage: Arc::new(Mutex::new(storage)),
        space: Default::default(),
    });
    state
}

fn is_disabled(harness: &Harness<'_, (RestoreTab, State)>, label: &str) -> bool {
    harness.get_by_label(label).accesskit_node().is_disabled()
}

#[test]
fn listing_waits_while_a_job_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
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
    let mut harness = theme::harness(
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
    let mut harness = theme::harness(
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
        listed: Arc::new(Listed::new(
            SnapshotInfo::new(
                "0".repeat(64),
                at(0),
                "pc".into(),
                "Mix".into(),
                vec!["salvajocs".into(), "game:mix".into()],
            ),
            StagedGame::new(
                PathBuf::from("/staged"),
                SnapshotMeta::new(SNAPSHOT_FORMAT, "Mix".into(), Vec::new()),
                Vec::new(),
                None,
            ),
            Vec::new(),
            None,
        )),
        targets: Targets {
            linux: None,
            win: None,
        },
        prefix_origin: None,
        reason: None,
        vars: GameVars::new("Mix", Vec::new(), &[], None),
        info: GameInfo::default(),
    })
}

/// Mix with one save, backed up into the repository of `storage` and read into a work dir in
/// `dir`, so a restore can download it; no targets.
fn backed_up(storage: &SharedStorage, dir: &Path) -> Arc<Fetched> {
    let real = dir.join("src/save.dat");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(&real, "save").unwrap();
    let mut scan = GameScan::default();
    scan.title = "Mix".into();
    scan.files = vec![FoundFile::new(
        Base::XdgData,
        "Mix/save.dat".into(),
        real,
        4,
        None,
    )];
    scan.roots = vec![(Base::XdgData, "Mix".into())];
    let mut repo = storage.lock().unwrap();
    backup_games(&mut repo, &[scan], &dir.join("stage"), &[], None, |_| {}).unwrap();
    let snapshot = restore::list(&repo).unwrap().remove(0).snapshots.remove(0);
    let listed = restore::read(&mut repo, &snapshot, &dir.join("work")).unwrap();
    Arc::new(Fetched {
        storage: storage.clone(),
        title: "Mix".into(),
        listed: Arc::new(listed),
        targets: Targets {
            linux: None,
            win: None,
        },
        prefix_origin: None,
        reason: None,
        vars: GameVars::new("Mix", Vec::new(), &[], None),
        info: GameInfo::default(),
    })
}

fn unknown_root() -> SnapshotRoot {
    (Base::WinAppData, "Mix".into())
}

/// The tab with the mapping dialog open over one unknown root of a game from `storage`.
fn with_dialog(storage: &SharedStorage) -> RestoreTab {
    let entry = MappingEntry::new(
        unknown_root(),
        1,
        None,
        Confidence::Unknown,
        Vec::new(),
        Vec::new(),
    );
    RestoreTab {
        mapping: Some((fetched(storage), Dialog::new(vec![entry]))),
        ..RestoreTab::default()
    }
}

/// Tab moves through the dialog's buttons in the order they appear, left to right.
#[test]
fn the_dialog_buttons_take_focus_left_to_right() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), |storage| {
        let entry = MappingEntry::new(
            unknown_root(),
            1,
            Some("<xdgData>/Game".into()),
            Confidence::Unknown,
            Vec::new(),
            Vec::new(),
        );
        RestoreTab {
            mapping: Some((fetched(storage), Dialog::new(vec![entry]))),
            ..RestoreTab::default()
        }
    });
    harness.run();
    harness
        .get_by_label("Restore into a Wine prefix instead")
        .focus();
    harness.run();
    for next in ["Cancel", "Use these folders"] {
        harness.key_press(egui::Key::Tab);
        harness.run();
        assert!(harness.get_by_label(next).is_focused(), "{next} is next");
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
        let plan = RestorePlan::new(Vec::new(), None, Vec::new(), true, Vec::new());
        Ok(Prepared {
            plan: fetched.labelled(Ok(plan)),
            fetched,
            choices: vec![(unknown_root(), Choice::Template("<xdgData>/Mix".into()))],
        })
    })
}

/// The plan `plan` of `fetched`, as the plan job hands it to the tab.
fn preview(fetched: Arc<Fetched>, plan: RestorePlan) -> Preview {
    let labelled = fetched.labelled(Ok(plan)).unwrap();
    Preview::new(fetched, labelled)
}

type TabHarness = Harness<'static, (RestoreTab, State)>;

/// The tab built by `tab` over the open repository of a state in `dir`.
fn tab_harness(dir: &Path, tab: impl FnOnce(&SharedStorage) -> RestoreTab) -> TabHarness {
    let state = state(dir);
    let tab = tab(&state.session.as_ref().unwrap().storage);
    theme::harness(|ui, (tab, state)| tab.ui(ui, state, false), (tab, state))
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

/// A tab listing Mix with one snapshot, chosen.
fn with_mix(storage: &SharedStorage) -> RestoreTab {
    let snapshot = SnapshotInfo::new(
        "0".repeat(64),
        at(0),
        "pc".into(),
        "Mix".into(),
        vec!["salvajocs".into(), "game:mix".into()],
    );
    RestoreTab {
        repository: Some(storage.clone()),
        games: vec![GameBackups::new(
            "mix".into(),
            "Mix".into(),
            vec![snapshot],
            Some(0),
        )],
        selected: Some("mix".into()),
        snapshot: Some(0),
        ..RestoreTab::default()
    }
}

/// A game `Mix` whose snapshots are `ids`, newest first.
fn mix_snapshots(ids: &[&str]) -> GameBackups {
    let snapshots = ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let time = at(1_000 - index as u64);
            let tags = vec!["salvajocs".into(), "game:mix".into()];
            SnapshotInfo::new(id.repeat(64), time, "pc".into(), "Mix".into(), tags)
        })
        .collect();
    GameBackups::new("mix".into(), "Mix".into(), snapshots, Some(0))
}

#[test]
fn a_new_snapshot_in_the_list_keeps_the_chosen_older_one_and_the_plan() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), |storage| RestoreTab {
        repository: Some(storage.clone()),
        games: vec![mix_snapshots(&["b", "a"])],
        selected: Some("mix".into()),
        snapshot: Some(1),
        preview: Some(preview(
            fetched(storage),
            RestorePlan::new(Vec::new(), None, Vec::new(), false, Vec::new()),
        )),
        ..RestoreTab::default()
    });
    let older = harness.state().0.chosen().unwrap().id.clone();
    let ctx = harness.ctx.clone();
    let state = &mut harness.state_mut();
    state.0.relist(&ctx, &state.1);
    assert!(state.0.preview.is_some(), "a backup removes no snapshot");
    // The new list has a snapshot in front, which shifts the older one's index.
    state.0.listing = Some(Job::spawn(&ctx, || {
        Ok(vec![mix_snapshots(&["c", "b", "a"])])
    }));
    step_until(&mut harness, "the list", |h| h.state().0.listing.is_none());

    let tab = &harness.state().0;
    assert_eq!(tab.snapshot, Some(2));
    assert_eq!(tab.chosen().unwrap().id, older);
}

#[test]
fn deleting_waits_while_a_restore_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), with_mix);
    harness.step();
    assert!(!is_disabled(&harness, "Delete snapshot"));
    assert!(!is_disabled(&harness, "Delete all backups of Mix"));

    let (release, restore) = crate::job::held(&harness.ctx.clone());
    harness.state_mut().0.restoring = Some(restore);
    harness.step();
    harness.step();
    assert!(is_disabled(&harness, "Delete snapshot"));
    assert!(is_disabled(&harness, "Delete all backups of Mix"));
    drop(release);
}

#[test]
fn a_running_delete_keeps_the_tab_busy() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), with_mix);
    let (release, delete) = crate::job::held(&harness.ctx.clone());
    harness.state_mut().0.deleting = Some(delete);
    harness.step();
    harness.step();
    assert!(harness.state().0.busy());
    assert!(is_disabled(&harness, "Load backups"));
    drop(release);
}

#[test]
fn a_new_repository_clears_the_list_and_the_plan() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), |storage| RestoreTab {
        repository: Some(storage.clone()),
        games: vec![GameBackups::new(
            "mix".into(),
            "Mix".into(),
            Vec::new(),
            None,
        )],
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

    use super::dirs::remove_dir;

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
fn a_running_restore_says_it_downloads_and_restores() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), |storage| RestoreTab {
        preview: Some(preview(
            fetched(storage),
            RestorePlan::new(Vec::new(), None, Vec::new(), false, Vec::new()),
        )),
        ..RestoreTab::default()
    });
    let (release, hold) = mpsc::channel::<()>();
    let job = Job::spawn(&harness.ctx.clone(), move || {
        let _ = hold.recv();
        Err(anyhow!("released"))
    });
    harness.state_mut().0.restoring = Some(job);
    harness.step();
    harness.step();

    let shown = harness.query_all_by_label_contains("Downloading and restoring");
    assert!(shown.count() > 0);
    assert!(is_disabled(&harness, "Restore files"));
    drop(release);
}

#[test]
fn the_plan_stays_when_a_wineserver_blocks_the_restore() {
    let dir = tempfile::tempdir().unwrap();
    // A plan with something ticked, which an empty plan lacks, so Restore files can enable.
    let mut harness = registry_restore(dir.path(), || false);
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

#[test]
fn a_running_backup_disables_restore_files_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = registry_restore(dir.path(), || false);
    assert!(!is_disabled(&harness, "Restore files"));

    harness.state_mut().0.set_backing_up(true);
    harness.step();
    harness.step();

    assert!(is_disabled(&harness, "Restore files"));
    let reason = harness.query_all_by_label_contains("Waiting for the running backup");
    assert!(reason.count() > 0);

    harness.state_mut().0.set_backing_up(false);
    harness.step();
    harness.step();
    assert!(!is_disabled(&harness, "Restore files"));
}

/// The tab previewing a registry restore into a prefix under `dir`, where the wineserver check
/// answers `wineserver_running`.
fn registry_restore(dir: &Path, wineserver_running: fn() -> bool) -> TabHarness {
    let prefix = dir.join("pfx");
    fs::create_dir_all(prefix.join("drive_c/users/me")).unwrap();
    fs::write(prefix.join("user.reg"), "WINE REGISTRY Version 2\n").unwrap();
    let mut export = RegistryExport::default();
    export.keys.insert(
        r"HKEY_CURRENT_USER\Software\Foo".into(),
        vec![RegValue::new("v".into(), RegData::Sz("restored".into()))],
    );
    let folders = WinFolders::for_prefix(&prefix, "me");
    let registry = Some((export, Context::WinePrefix { prefix, folders }));
    let mut harness = tab_harness(dir, |storage| RestoreTab {
        preview: Some(preview(
            backed_up(storage, dir),
            RestorePlan::new(Vec::new(), registry, Vec::new(), false, Vec::new()),
        )),
        ..RestoreTab::default()
    });
    harness.state_mut().1.env.wineserver_running = wineserver_running;
    harness.step();
    harness
}

#[test]
fn a_running_wineserver_blocks_a_registry_restore_into_a_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = registry_restore(dir.path(), || true);

    harness.get_by_label("Restore files").click();
    harness.step();
    harness.step();
    run_until_restored(&mut harness);

    let tab = &harness.state().0;
    assert!(tab.restored.is_none());
    assert!(tab.error.as_deref().unwrap().contains("Close the game"));
    let user_reg = fs::read_to_string(dir.path().join("pfx/user.reg")).unwrap();
    assert!(!user_reg.contains("Foo"));
}

/// The tab over the repository of a state in `dir`, on a Linux machine whose home is in `dir`,
/// with Mix backed up from `base`, declared by `template`, ticked and planned until its confirm
/// or mapping dialog shows. Its save already exists at the native target, so a run would make a
/// safety backup.
fn planned_batch(dir: &Path, base: Base, template: &str) -> TabHarness {
    let home = dir.join("home");
    let mut state = state(dir);
    let mut environment = Environment::default();
    environment.native = Some(Context::Linux(LinuxFolders::new(
        "me".into(),
        home.clone(),
        home.join(".local/share"),
        home.join(".config"),
    )));
    state.env.environment = Some(environment);
    let yaml = format!("Mix:\n  files:\n    {template}: {{tags: [save]}}\n");
    let entry = parse_yaml(&yaml).unwrap().games.remove("Mix").unwrap();
    state.config.as_mut().unwrap().custom_games =
        vec![CustomGame::new("Mix".into(), entry, Integration::Override)];
    let real = dir.join("src/save.dat");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(&real, "new").unwrap();
    let target = home.join(".local/share/Mix/save.dat");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, "old").unwrap();
    let mut scan = GameScan::default();
    scan.title = "Mix".into();
    scan.files = vec![FoundFile::new(base, "Mix/save.dat".into(), real, 3, None)];
    scan.roots = vec![(base, "Mix".into())];
    let storage = state.session.as_ref().unwrap().storage.clone();
    let stage = dir.join("stage");
    backup_games(
        &mut storage.lock().unwrap(),
        &[scan],
        &stage,
        &[],
        None,
        |_| {},
    )
    .unwrap();
    let games = restore::list(&storage.lock().unwrap()).unwrap();
    let tab = RestoreTab {
        repository: Some(storage),
        checked: games.iter().map(|game| game.slug.clone()).collect(),
        games,
        ..RestoreTab::default()
    };
    let mut harness = theme::harness(|ui, (tab, state)| tab.ui(ui, state, false), (tab, state));
    harness.step();
    harness.get_by_label("Restore 1 game").click();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while !harness.state().0.batch.open() {
        assert!(
            std::time::Instant::now() < deadline,
            "the batch was never planned"
        );
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.step();
    harness
}

/// Steps `harness` until `done` holds.
fn step_until(harness: &mut TabHarness, what: &str, done: impl Fn(&TabHarness) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while !done(harness) {
        assert!(
            std::time::Instant::now() < deadline,
            "{what} never happened"
        );
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_pick_edited_while_detection_runs_drops_what_the_default_said() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let home = dir.join("home");
    let mut state = state(dir);
    let bottle = dir.join("Generic-Game");
    fs::create_dir_all(bottle.join("drive_c/users/steamuser")).unwrap();
    fs::write(bottle.join("user.reg"), "WINE REGISTRY Version 2\n").unwrap();
    let mut found = LauncherPrefix::new(Launcher::Bottles, bottle, None);
    found.name = "Generic-Game".into();
    let mut environment = Environment::default();
    environment.native = Some(Context::Linux(LinuxFolders::new(
        "me".into(),
        home.clone(),
        home.join(".local/share"),
        home.join(".config"),
    )));
    environment.launcher_prefixes = vec![found];
    state.env.environment = Some(environment);
    let yaml = "Mix:\n  files:\n    <winAppData>/Mix: {tags: [save]}\n";
    let entry = parse_yaml(yaml).unwrap().games.remove("Mix").unwrap();
    state.config.as_mut().unwrap().custom_games =
        vec![CustomGame::new("Mix".into(), entry, Integration::Override)];
    let real = dir.join("src/save.dat");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(&real, "save").unwrap();
    let mut scan = GameScan::default();
    scan.title = "Mix".into();
    scan.files = vec![FoundFile::new(
        Base::WinAppData,
        "Mix/save.dat".into(),
        real,
        4,
        None,
    )];
    scan.roots = vec![(Base::WinAppData, "Mix".into())];
    let storage = state.session.as_ref().unwrap().storage.clone();
    // An older snapshot from no prefix, then the newest from the bottle.
    for origin in [None, Some(PrefixOrigin::Bottles("Generic-Game".into()))] {
        scan.prefix_origin = origin;
        let mut repo = storage.lock().unwrap();
        backup_games(
            &mut repo,
            &[scan.clone()],
            &dir.join("stage"),
            &[],
            None,
            |_| {},
        )
        .unwrap();
    }
    let games = restore::list(&storage.lock().unwrap()).unwrap();
    let tab = RestoreTab {
        repository: Some(storage.clone()),
        games,
        ..RestoreTab::default()
    };
    let mut harness = theme::harness(|ui, (tab, state)| tab.ui(ui, state, false), (tab, state));
    harness.step();
    harness.get_by_label("Mix").click();
    step_until(&mut harness, "the plan", |h| h.state().0.preview.is_some());
    let from_backup = |h: &TabHarness| h.query_all_by_label_contains("from the backup").count();
    assert!(from_backup(&harness) > 0);

    // The older snapshot, whose read waits for the repository while the pick is changed.
    let held = storage.lock().unwrap();
    harness.get_by_label("Snapshot").click();
    harness.step();
    harness.step();
    let mut options = harness.query_all_by_label_contains(" UTC, ");
    options.nth(1).expect("both snapshots are listed").click();
    drop(options);
    harness.step();
    assert!(harness.state().0.detecting.is_some());
    harness.get_by_label("Native Linux build").click();
    harness.step();
    drop(held);
    step_until(&mut harness, "the plan", |h| {
        h.state().0.detecting.is_none() && h.state().0.preview.is_some()
    });

    assert_eq!(from_backup(&harness), 0);
}

/// Opens another repository, as Settings does, and checks the batch is gone without writing.
fn switch_repository_and_check_nothing_ran(harness: &mut TabHarness, dir: &Path) {
    let old = storage(harness);
    let fetched = own_dir(&harness.state().1.env.cache_dir, "restore-batch");
    let other = tempfile::tempdir().unwrap();
    harness.state_mut().1.session = state(other.path()).session;
    harness.step();
    assert!(!harness.state().0.batch.busy());
    assert!(!fetched.exists(), "nothing was downloaded");

    assert!(!harness.state().0.batch.open());
    assert!(harness.query_by_label("Cancel").is_none());
    assert!(
        old.lock()
            .unwrap()
            .snapshots("pre-restore")
            .unwrap()
            .is_empty()
    );
    let target = dir.join("home/.local/share/Mix/save.dat");
    assert_eq!(fs::read_to_string(target).unwrap(), "old");
}

#[test]
fn a_new_repository_closes_the_batch_confirm() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = planned_batch(dir.path(), Base::XdgData, "<xdgData>/Mix");
    assert!(
        harness.query_by_label("Cancel").is_some(),
        "the confirm shows"
    );

    switch_repository_and_check_nothing_ran(&mut harness, dir.path());
}

#[test]
fn a_cleanup_dropping_the_list_closes_the_batch_confirm() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = planned_batch(dir.path(), Base::XdgData, "<xdgData>/Mix");
    assert!(harness.query_by_label("Cancel").is_some());

    harness.state_mut().0.drop_listing();
    harness.step();
    assert!(harness.query_by_label("Cancel").is_none());
}

#[test]
fn a_running_backup_disables_the_batch_restore_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = planned_batch(dir.path(), Base::XdgData, "<xdgData>/Mix");
    assert!(!is_disabled(&harness, "Restore 1 game"));

    harness.state_mut().0.set_backing_up(true);
    harness.step();
    harness.step();

    assert!(is_disabled(&harness, "Restore 1 game"));
    let reason = harness.query_all_by_label_contains("Waiting for the running backup");
    assert!(reason.count() > 0);

    // Without the confirm, the sidebar's button that plans the batch waits too.
    harness.state_mut().0.batch.close();
    harness.step();
    harness.step();
    assert!(is_disabled(&harness, "Restore 1 game"));
    harness.state_mut().0.set_backing_up(false);
    harness.step();
    harness.step();
    assert!(!is_disabled(&harness, "Restore 1 game"));
}

#[test]
fn a_new_repository_closes_a_batch_mapping_dialog() {
    let dir = tempfile::tempdir().unwrap();
    // A Windows save with no Windows target and no rule needs the dialog.
    let mut harness = planned_batch(dir.path(), Base::WinAppData, "<winAppData>/Mix");
    assert!(
        harness.query_by_label("Skip this game").is_some(),
        "the dialog shows"
    );

    switch_repository_and_check_nothing_ran(&mut harness, dir.path());
    assert!(harness.query_by_label("Skip this game").is_none());
}

#[test]
fn a_running_plan_says_it_reads_the_file_list_and_keeps_nothing_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), with_mix);
    let (release, hold) = mpsc::channel();
    let job = confirm_job(&harness.ctx.clone(), hold, &storage(&harness));
    harness.state_mut().0.preparing = Some((0, job));
    harness.step();
    harness.step();

    let reading = harness.query_all_by_label_contains("Reading the backup's file list");
    assert!(reading.count() > 0);
    assert!(is_disabled(&harness, "Restore files"));
    assert!(!is_disabled(&harness, "Load backups"));
    drop(release);
}

#[test]
fn a_plan_for_an_older_choice_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), with_mix);
    let (release, hold) = mpsc::channel();
    let job = confirm_job(&harness.ctx.clone(), hold, &storage(&harness));
    let tab = &mut harness.state_mut().0;
    tab.generation = 1;
    tab.preparing = Some((0, job));
    drop(release);
    run_until_prepared(&mut harness);
    harness.step();

    let tab = &harness.state().0;
    assert!(tab.preview.is_none());
    assert!(tab.mapping.is_none());
}

#[test]
fn forgetting_the_plan_closes_the_open_details() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = tab_harness(dir.path(), |_| RestoreTab::default());
    let storage = storage(&harness);
    let fetched = backed_up(&storage, dir.path());
    let source = fetched
        .listed
        .staged
        .root
        .join("files/XdgData/Mix/save.dat");
    let write = Write::new(source, dir.path().join("target/save.dat"), None, None);
    let copy = fetched.listed.backed_up(&write).unwrap();
    let key = "XdgData/Mix/save.dat".to_owned();
    let ctx = harness.ctx.clone();
    let details = Details::start(&ctx, &storage, "mix".into(), key, write.target, copy);
    harness.state_mut().0.details = Some(details);
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while harness.query_by_label("Close details").is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the dialog never showed"
        );
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }

    harness.state_mut().0.forget_plan();
    harness.step();

    assert!(harness.state().0.details.is_none());
    assert!(harness.query_by_label("Close details").is_none());
}
