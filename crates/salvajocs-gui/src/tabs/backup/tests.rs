use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::backup::BackupError;
use salvajocs_core::config::{AppConfig, PasswordMode};
use salvajocs_core::layout::Base;
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::scan::{FoundFile, RegistrySource};
use salvajocs_core::selection::{Change, Pending};
use salvajocs_core::storage::{Compression, RepoLocation, SnapshotInfo, Storage};

use crate::env::{Env, MemorySecrets};
use crate::theme;

use super::*;
use crate::state::Session;

const KEY: &str = "HKEY_CURRENT_USER/Software/Game";

fn game(files: &[u64], registry: bool, unreadable: usize) -> GameScan {
    GameScan {
        title: "Game".into(),
        files: files
            .iter()
            .map(|size| FoundFile {
                base: Base::Home,
                tail: "save".into(),
                real: PathBuf::from("/save"),
                size: *size,
            })
            .collect(),
        roots: Vec::new(),
        registry_keys: registry.then(|| KEY.to_owned()).into_iter().collect(),
        registry_source: registry.then_some(RegistrySource::Windows),
        prefixes: Vec::new(),
        unreadable: vec![PathBuf::from("/locked"); unreadable],
    }
}

fn backup(result: Result<Option<SnapshotInfo>, BackupError>) -> GameBackup {
    GameBackup {
        title: "Game".into(),
        slug: "game".into(),
        result,
    }
}

#[test]
fn sizes_use_binary_units() {
    assert_eq!(format_size(0), "0 B");
    assert_eq!(format_size(1023), "1023 B");
    assert_eq!(format_size(1024), "1.0 KiB");
    assert_eq!(format_size(1536 * 1024), "1.5 MiB");
    assert_eq!(format_size(u64::MAX), "16777216.0 TiB");
}

#[test]
fn summary_names_files_registry_and_unreadable_paths() {
    let all = GameSettings::default();
    assert_eq!(
        summary(&game(&[1024, 2048], true, 0), (2, 3072), &all),
        "2 files, 3.0 KiB, plus registry keys"
    );
    assert_eq!(
        summary(&game(&[], false, 2), (0, 0), &all),
        "0 files, 0 B, 2 unreadable paths (the files may be incomplete)"
    );
}

#[test]
fn summary_counts_one_file_and_one_path_in_the_singular() {
    let all = GameSettings::default();
    assert_eq!(summary(&game(&[3], false, 0), (1, 3), &all), "1 file, 3 B");
    assert_eq!(
        summary(&game(&[3], false, 1), (1, 3), &all),
        "1 file, 3 B, 1 unreadable path (the files may be incomplete)"
    );
}

#[test]
fn summary_says_what_the_settings_exclude() {
    let game = game(&[1024, 2048, 4096], true, 0);
    let mut settings = GameSettings::default();

    assert_eq!(
        summary(&game, (1, 1024), &settings),
        "1 file, 1.0 KiB, 2 excluded, plus registry keys"
    );
    settings.set_registry(KEY, false);
    assert_eq!(
        summary(&game, (1, 1024), &settings),
        "1 file, 1.0 KiB, 2 excluded"
    );
}

#[test]
fn a_failed_game_carries_its_error_chain() {
    let failed = backup(Err(BackupError::UnsafeTail("../x".into())));
    let GameResult { title, outcome } = summarize(&failed);
    assert_eq!(title, "Game");
    assert!(matches!(outcome, Outcome::Failed(text) if text.contains("../x")));
    assert_eq!(summarize(&backup(Ok(None))).outcome, Outcome::Unchanged);
}

#[test]
fn every_problem_names_its_path() {
    let path = PathBuf::from("/prefix");
    for kind in [
        ProblemKind::AmbiguousPrefix(vec!["a".into(), "b".into()]),
        ProblemKind::NoUser,
        ProblemKind::NotAPrefix,
        ProblemKind::Unreadable("denied".into()),
    ] {
        let text = describe_problem(&ScanProblem {
            path: path.clone(),
            kind,
        });
        assert!(text.starts_with("/prefix: "), "{text}");
    }
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
        rclone_remotes: || Err(std::io::ErrorKind::NotFound.into()),
        pick_folder: Arc::new(|| None),
        open_folder: Arc::new(|_| Ok(())),
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

fn is_disabled(harness: &Harness<'_, (BackupTab, State)>, label: &str) -> bool {
    harness.get_by_label(label).accesskit_node().is_disabled()
}

/// A tab listing a game of one file for each of `titles`.
fn listing(titles: &[&str]) -> BackupTab {
    let mut tab = BackupTab::default();
    tab.adopt(ScanReport {
        games: titles
            .iter()
            .map(|title| GameScan {
                title: (*title).into(),
                ..game(&[1], false, 0)
            })
            .collect(),
        problems: Vec::new(),
    });
    tab
}

/// Steps until the tab's jobs, such as loading what to back up, are done.
fn settle(harness: &mut Harness<'_, (BackupTab, State)>) {
    let deadline = Instant::now() + Duration::from_secs(60);
    harness.step();
    while harness.state().0.busy() {
        assert!(Instant::now() < deadline, "the tab's jobs never ended");
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
    harness.step();
}

#[test]
fn scan_and_backup_wait_while_a_backup_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (listing(&["Game"]), state(dir.path())),
    );
    settle(&mut harness);
    assert!(!is_disabled(&harness, "Scan for games"));
    assert!(!is_disabled(&harness, "Back up 1 game"));

    let (release, hold) = mpsc::channel::<()>();
    let report = ScanReport {
        games: Vec::new(),
        problems: Vec::new(),
    };
    harness.state_mut().0.running = Some(Job::spawn(&harness.ctx.clone(), move || {
        let _ = hold.recv();
        Ok(Backed {
            report,
            error: None,
            saving: SaveProblems::default(),
            after: AfterBackup {
                source: std::sync::Weak::new(),
                pending: None,
                written: None,
                games: None,
            },
            cleanup: None,
        })
    }));
    harness.step();
    harness.step();
    assert!(is_disabled(&harness, "Scan for games"));
    assert!(is_disabled(&harness, "Back up 1 game"));
    drop(release);
}

#[test]
fn scan_waits_for_the_manifest_to_load() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
        |ui, (tab, state)| tab.ui(ui, state, true),
        (BackupTab::default(), state(dir.path())),
    );
    harness.step();
    assert!(is_disabled(&harness, "Scan for games"));
}

/// Before a scan the empty state's scan is the one action: no second scan button, and no
/// backup button for zero games.
#[test]
fn before_a_scan_the_only_action_is_one_enabled_scan() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (BackupTab::default(), state(dir.path())),
    );
    harness.step();
    assert!(harness.query_by_label("Find your saves").is_some());
    assert_eq!(harness.query_all_by_label("Scan for games").count(), 1);
    assert!(!is_disabled(&harness, "Scan for games"));
    assert!(
        harness
            .query_all_by_label_contains("Back up")
            .next()
            .is_none()
    );
}

#[test]
fn a_folder_that_cannot_be_opened_is_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    state.env.open_folder = Arc::new(|_| Err(std::io::ErrorKind::NotFound.into()));
    let mut harness = theme::harness(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (listing(&["Game"]), state),
    );
    settle(&mut harness);
    harness.get_by_label("1 file, 1 B").click();
    harness.step();

    harness
        .get_by_label("Show Home in the file manager")
        .click();
    harness.step();

    assert_eq!(harness.state().1.notices.len(), 1);
}

#[test]
fn a_scan_that_found_nothing_says_where_to_add_games() {
    let tab = BackupTab {
        report: Some(ScanReport {
            games: Vec::new(),
            problems: Vec::new(),
        }),
        ..BackupTab::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (tab, state(dir.path())),
    );
    harness.step();
    assert_eq!(harness.query_all_by_label("Scan for games").count(), 1);
    assert!(harness.query_by_label("0 games found").is_some());
    assert!(
        harness
            .query_all_by_label_contains("No saves found")
            .next()
            .is_some()
    );
}

#[test]
fn select_all_and_deselect_all_change_every_game() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (listing(&["Alpha", "Beta"]), state(dir.path())),
    );
    settle(&mut harness);
    let skipped = |harness: &Harness<'_, (BackupTab, State)>| {
        ["Alpha", "Beta"].map(|title| harness.state().0.choices.game(title).unwrap().skip)
    };

    harness.get_by_label("Deselect all").click();
    settle(&mut harness);
    assert_eq!(skipped(&harness), [true, true]);
    let pending = Pending::load(&dir.path().join("config")).unwrap();
    let games = pending.repositories.values().next().unwrap();
    assert_eq!(games["Alpha"], [Change::Skip(true)]);
    assert_eq!(games["Beta"], [Change::Skip(true)]);

    harness.get_by_label("Select all").click();
    settle(&mut harness);
    assert_eq!(skipped(&harness), [false, false]);
}

#[test]
fn choices_wait_for_the_repository_to_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    let session = state.session.take();
    let mut harness = theme::harness(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (listing(&["Game"]), state),
    );
    settle(&mut harness);
    assert!(is_disabled(&harness, "Game"));
    assert!(is_disabled(&harness, "Deselect all"));
    assert!(
        harness
            .query_all_by_label_contains("Open the repository to choose what to back up")
            .next()
            .is_some()
    );

    harness.state_mut().1.session = session;
    settle(&mut harness);
    assert!(!is_disabled(&harness, "Game"));
    assert!(!is_disabled(&harness, "Deselect all"));
}

/// The save bar marks the repository encrypted exactly when it has a password.
#[test]
fn the_save_bar_says_whether_the_repository_is_encrypted() {
    for (password, shown, hidden) in [
        (PasswordMode::None, "Not encrypted", "Encrypted"),
        (PasswordMode::Keyring, "Encrypted", "Not encrypted"),
        (PasswordMode::Ask, "Encrypted", "Not encrypted"),
    ] {
        let tab = BackupTab {
            report: Some(ScanReport {
                games: Vec::new(),
                problems: Vec::new(),
            }),
            ..BackupTab::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let mut state = state(dir.path());
        state.config.as_mut().unwrap().password = password;
        let mut harness = theme::harness(|ui, (tab, state)| tab.ui(ui, state, false), (tab, state));
        harness.step();
        assert!(harness.query_by_label(shown).is_some(), "{password:?}");
        assert!(harness.query_by_label(hidden).is_none(), "{password:?}");
    }
}

#[test]
fn back_up_waits_while_what_to_back_up_loads() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (listing(&["Game"]), state(dir.path())),
    );
    settle(&mut harness);
    assert!(!is_disabled(&harness, "Back up 1 game"));

    let ctx = harness.ctx.clone();
    let (tab, state) = harness.state_mut();
    let storage = state.session.as_ref().unwrap().storage.clone();
    let release = tab.choices.hold_load(&ctx, &storage);
    harness.step();
    harness.step();
    assert!(is_disabled(&harness, "Back up 1 game"));
    assert!(is_disabled(&harness, "Game"));
    drop(release);
}

#[test]
fn back_up_clicked_during_a_pending_write_starts_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (listing(&["Game"]), state(dir.path())),
    );
    settle(&mut harness);
    let ctx = harness.ctx.clone();
    let release = harness.state_mut().0.choices.hold_write(&ctx);
    harness.step();

    harness.get_by_label("Back up 1 game").click();
    harness.step();
    harness.step();
    assert!(harness.state().0.running.is_none(), "waits for the write");
    assert!(harness.state().0.backup_requested);

    drop(release);
    let deadline = Instant::now() + Duration::from_secs(60);
    while harness.state().0.running.is_none() {
        assert!(Instant::now() < deadline, "the backup never started");
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
    settle(&mut harness);
}

#[test]
fn each_repository_keeps_its_own_unsaved_choices() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (listing(&["Game"]), state(dir.path())),
    );
    settle(&mut harness);
    harness.get_by_label("Game").click();
    settle(&mut harness);
    let skipped = |harness: &Harness<'_, (BackupTab, State)>| {
        harness.state().0.choices.game("Game").unwrap().skip
    };
    assert!(skipped(&harness));

    // Open another repository, as Settings does.
    let first = {
        let state = &mut harness.state_mut().1;
        let other = dir.path().join("other").to_str().unwrap().to_owned();
        let location = RepoLocation {
            repository: other.clone(),
            options: Default::default(),
        };
        let storage = Storage::init(&location, "", Compression::Default).unwrap();
        let first = state.session.replace(Session {
            compression: storage.compression(),
            storage: Arc::new(Mutex::new(storage)),
        });
        let config = state.config.as_mut().unwrap();
        let first_repository = std::mem::replace(&mut config.repository, other);
        (first, first_repository)
    };
    settle(&mut harness);
    assert!(!skipped(&harness), "the other repository has no changes");
    harness.get_by_label("Game").click();
    settle(&mut harness);
    harness.get_by_label("Game").click();
    settle(&mut harness);
    assert!(!skipped(&harness));

    let (session, repository) = first;
    let state = &mut harness.state_mut().1;
    state.session = session;
    state.config.as_mut().unwrap().repository = repository;
    settle(&mut harness);
    assert!(skipped(&harness), "the first repository keeps its change");
    let pending = Pending::load(&dir.path().join("config")).unwrap();
    let changes: Vec<_> = pending
        .repositories
        .values()
        .map(|games| &games["Game"])
        .collect();
    assert_eq!(
        changes,
        [
            &vec![Change::Skip(true)],
            &vec![Change::Skip(true), Change::Skip(false)]
        ]
    );
}

#[test]
fn closing_gives_up_on_a_stuck_pending_write() {
    let dir = tempfile::tempdir().unwrap();
    let mut choices = Choices::default();
    let _release = choices.hold_write(&egui::Context::default());

    assert!(choices.flush(dir.path()).is_err());
}

/// A game whose one save file, under `dir`, holds `content`.
fn saved_game(dir: &Path, title: &str, content: &str) -> GameScan {
    let real = dir.join(title);
    std::fs::write(&real, content).unwrap();
    GameScan {
        title: title.into(),
        files: vec![FoundFile {
            base: Base::Home,
            tail: "save".into(),
            real,
            size: content.len() as u64,
        }],
        roots: Vec::new(),
        registry_keys: Vec::new(),
        registry_source: None,
        prefixes: Vec::new(),
        unreadable: Vec::new(),
    }
}

/// A new repository in `dir`, shared as a session shares it.
fn shared_storage(dir: &Path) -> SharedStorage {
    let location = RepoLocation {
        repository: dir.join("repo").to_str().unwrap().into(),
        options: Default::default(),
    };
    let storage = Storage::init(&location, "", Compression::Default).unwrap();
    Arc::new(Mutex::new(storage))
}

/// The ids of the snapshots of `title`, newest first.
fn snapshot_ids(storage: &SharedStorage, title: &str) -> Vec<String> {
    let tag = format!("game:{}", salvajocs_core::slug::slug(title));
    let snapshots = lock_storage(storage).unwrap().snapshots(&tag).unwrap();
    snapshots.into_iter().map(|snapshot| snapshot.id).collect()
}

#[test]
fn back_up_returns_only_the_new_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let storage = shared_storage(dir.path());
    let stage = dir.path().join("stage");
    let same = saved_game(dir.path(), "Same", "a");
    let games = [same.clone(), saved_game(dir.path(), "Changed", "b")];
    back_up(&storage, &games, &stage, |_| {}).unwrap();
    let failing = saved_game(dir.path(), "Failing", "c");
    // A file where its staging dir belongs makes staging it fail.
    std::fs::write(stage.join(salvajocs_core::slug::slug("Failing")), "").unwrap();

    let games = [same, saved_game(dir.path(), "Changed", "b2"), failing];
    let made = back_up(&storage, &games, &stage, |_| {}).unwrap();

    assert_eq!(made, snapshot_ids(&storage, "Changed")[..1]);
}

#[test]
fn clean_up_never_forgets_the_snapshots_it_is_given() {
    let dir = tempfile::tempdir().unwrap();
    let storage = shared_storage(dir.path());
    let stage = dir.path().join("stage");
    for content in ["old", "new"] {
        let games = [saved_game(dir.path(), "Game", content)];
        back_up(&storage, &games, &stage, |_| {}).unwrap();
    }
    let ids = snapshot_ids(&storage, "Game");
    let policy = Retention {
        keep_last: std::num::NonZeroU32::new(1),
        keep_within: None,
    };
    retention::save(&mut lock_storage(&storage).unwrap(), &policy, &stage).unwrap();

    let cleaned = clean_up(&storage, std::slice::from_ref(&ids[1]));

    assert!(cleaned.error.is_none());
    assert_eq!(cleaned.done.forgotten, 0);
    assert_eq!(snapshot_ids(&storage, "Game"), ids);
}
