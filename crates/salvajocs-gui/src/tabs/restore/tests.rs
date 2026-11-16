use std::sync::{Mutex, mpsc};
use std::time::Duration;

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::config::{AppConfig, PasswordMode};
use salvajocs_core::crossbuild::{Confidence, MappingEntry};
use salvajocs_core::layout::{SNAPSHOT_FORMAT, SnapshotMeta};
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
