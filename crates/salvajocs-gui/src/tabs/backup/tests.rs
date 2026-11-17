use std::path::{Path, PathBuf};
use std::sync::mpsc;

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::backup::BackupError;
use salvajocs_core::config::{AppConfig, PasswordMode};
use salvajocs_core::layout::Base;
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::scan::{FoundFile, RegistrySource};
use salvajocs_core::storage::{Compression, RepoLocation, SnapshotInfo, Storage};

use crate::env::{Env, MemorySecrets};
use crate::theme;

use super::*;
use crate::state::Session;

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
        registry_keys: Vec::new(),
        registry_source: registry.then_some(RegistrySource::Windows),
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
    assert_eq!(
        summary(&game(&[1024, 2048], true, 0)),
        "2 files, 3.0 KiB, plus registry keys"
    );
    assert_eq!(
        summary(&game(&[], false, 2)),
        "0 files, 0 B, 2 unreadable paths (the files may be incomplete)"
    );
}

#[test]
fn summary_counts_one_file_and_one_path_in_the_singular() {
    assert_eq!(summary(&game(&[3], false, 0)), "1 file, 3 B");
    assert_eq!(
        summary(&game(&[3], false, 1)),
        "1 file, 3 B, 1 unreadable path (the files may be incomplete)"
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

#[test]
fn scan_and_backup_wait_while_a_backup_runs() {
    let tab = BackupTab {
        report: Some(ScanReport {
            games: vec![game(&[1], false, 0)],
            problems: Vec::new(),
        }),
        selected: BTreeSet::from(["Game".to_owned()]),
        ..BackupTab::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (tab, state(dir.path())),
    );
    harness.step();
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
    let named = |title: &str| GameScan {
        title: title.into(),
        ..game(&[1], false, 0)
    };
    let tab = BackupTab {
        report: Some(ScanReport {
            games: vec![named("Alpha"), named("Beta")],
            problems: Vec::new(),
        }),
        ..BackupTab::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (tab, state(dir.path())),
    );
    harness.run();
    harness.get_by_label("Select all").click();
    harness.run();
    let all = BTreeSet::from(["Alpha".to_owned(), "Beta".to_owned()]);
    assert_eq!(harness.state().0.selected, all);
    harness.get_by_label("Deselect all").click();
    harness.run();
    assert!(harness.state().0.selected.is_empty());
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
