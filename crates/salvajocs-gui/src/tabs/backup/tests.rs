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
        "2 files, 3.0 KiB, registry: yes"
    );
    assert_eq!(
        summary(&game(&[], false, 2)),
        "0 files, 0 B, registry: no, 2 unreadable paths (the files may be incomplete)"
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
    let mut harness = Harness::new_ui_state(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (tab, state(dir.path())),
    );
    harness.step();
    assert!(!is_disabled(&harness, "Scan for games"));
    assert!(!is_disabled(&harness, "Back up selected (1)"));

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
    assert!(is_disabled(&harness, "Back up selected (1)"));
    drop(release);
}

#[test]
fn scan_waits_for_the_manifest_to_load() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = Harness::new_ui_state(
        |ui, (tab, state)| tab.ui(ui, state, true),
        (BackupTab::default(), state(dir.path())),
    );
    harness.step();
    assert!(is_disabled(&harness, "Scan for games"));
}
