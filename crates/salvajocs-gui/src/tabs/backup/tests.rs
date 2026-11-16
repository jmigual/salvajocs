use std::path::PathBuf;

use salvajocs_core::backup::BackupError;
use salvajocs_core::layout::Base;
use salvajocs_core::scan::{FoundFile, RegistrySource};
use salvajocs_core::storage::SnapshotInfo;

use super::*;

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
