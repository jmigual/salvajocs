use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::backup::{BackupError, GameBackup, Stored};
use salvajocs_core::changes::{self, Comparison, FileState, Mapped, Mapper};
use salvajocs_core::config::{AppConfig, PasswordMode, SortKey};
use salvajocs_core::custom_games;
use salvajocs_core::layout::{Base, Hashes};
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::manifest::{CustomGame, Integration, Manifest};
use salvajocs_core::retention::{self, Retention};
use salvajocs_core::scan::{FoundFile, GameScan, RegistrySource};
use salvajocs_core::selection::{Change, Pending};
use salvajocs_core::storage::{Compression, RepoLocation, SnapshotInfo, Storage};
use salvajocs_core::sync;

use crate::env::{Env, MemorySecrets};
use crate::state::lock_storage;
use crate::status_bar;
use crate::tabs::format_date;
use crate::tabs::tree::Badge;
use crate::theme;

use super::preview::{
    Baseline, Header, Next, Tone, change_count, header_of, hint, next_backup, preview,
    preview_text, sentence, summary,
};
use super::results::{Level, summarize};
use super::run::{back_up, clean_up};
use super::unsent::conflict_lines;
use super::*;
use crate::state::Session;

const KEY: &str = "HKEY_CURRENT_USER/Software/Game";

fn game(files: &[u64], registry: bool, unreadable: usize) -> GameScan {
    let mut game = GameScan::default();
    game.title = "Game".into();
    game.files = files
        .iter()
        .map(|size| {
            FoundFile::new(
                Base::Home,
                "save".into(),
                PathBuf::from("/save"),
                *size,
                None,
            )
        })
        .collect();
    game.registry_keys = registry.then(|| KEY.to_owned()).into_iter().collect();
    game.registry_source = registry.then_some(RegistrySource::Windows);
    game.unreadable = vec![PathBuf::from("/locked"); unreadable];
    game
}

fn backup(result: Result<Stored, BackupError>) -> GameBackup {
    GameBackup::new("Game".into(), "game".into(), result)
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
    let unchanged = Stored::UpToDate { elsewhere: None };
    assert_eq!(
        summarize(&backup(Ok(unchanged))).outcome,
        Outcome::Unchanged
    );
}

fn snapshot_from(host: &str) -> SnapshotInfo {
    SnapshotInfo::new(
        format!("{host}-id"),
        SystemTime::UNIX_EPOCH,
        host.into(),
        "Game".into(),
        Vec::new(),
    )
}

/// The mapped outcome of a backup that stored `stored`.
fn outcome_of(stored: Stored) -> Outcome {
    summarize(&backup(Ok(stored))).outcome
}

#[test]
fn each_sync_state_has_its_own_result() {
    let newest = || snapshot_from("there");
    let date = format_date(SystemTime::UNIX_EPOCH);
    assert_eq!(
        outcome_of(Stored::Behind { newest: newest() }),
        Outcome::Behind {
            host: "there".into(),
            date
        }
    );
    assert_eq!(
        outcome_of(Stored::ChangedOnBoth { newest: newest() }),
        Outcome::ChangedOnBoth("there".into())
    );
    assert_eq!(
        outcome_of(Stored::UnknownBase { newest: newest() }),
        Outcome::UnknownBase("there".into())
    );
    assert_eq!(
        outcome_of(Stored::UpToDate {
            elsewhere: Some(newest())
        }),
        Outcome::SameAsNewest("there".into())
    );
}

#[test]
fn a_game_that_was_not_backed_up_is_not_a_failure() {
    let date = "2026-11-25".to_owned();
    let behind = Outcome::Behind {
        host: "there".into(),
        date,
    };
    let both = Outcome::ChangedOnBoth("there".into());
    let unknown = Outcome::UnknownBase("there".into());
    assert_eq!(
        (behind.icon(), behind.level()),
        (icons::ARROW_DOWN, Level::Warn)
    );
    assert_eq!((both.icon(), both.level()), (icons::WARNING, Level::Warn));
    assert_eq!(
        (unknown.icon(), unknown.level()),
        (icons::WARNING, Level::Warn)
    );
    let failed = Outcome::Failed("x".into());
    assert_eq!(
        (failed.icon(), failed.level()),
        (icons::X_CIRCLE, Level::Error)
    );
    for outcome in [
        Outcome::Saved,
        Outcome::Unchanged,
        Outcome::SameAsNewest("there".into()),
    ] {
        assert_eq!(
            (outcome.icon(), outcome.level()),
            (icons::CHECK_CIRCLE, Level::Good)
        );
    }
}

#[test]
fn the_result_lines_say_why_nothing_was_backed_up() {
    let behind = Outcome::Behind {
        host: "there".into(),
        date: "2026-11-25".into(),
    };
    assert_eq!(
        behind.line("Game"),
        "Game: not backed up: there has newer saves, from 2026-11-25, and this PC has not changed \
         them since it last synced. Restore them on the Restore tab."
    );
    let both = Outcome::ChangedOnBoth("there".into()).line("Game");
    assert!(
        both.contains("both this PC and there changed the saves"),
        "{both}"
    );
    let unknown = Outcome::UnknownBase("there".into()).line("Game");
    assert!(
        unknown.contains("does not know which backup this PC is in sync with"),
        "{unknown}"
    );
    for line in [both, unknown] {
        assert!(line.starts_with("Game: not backed up: "), "{line}");
        assert!(!line.contains("failed"), "{line}");
    }
    assert_eq!(
        Outcome::SameAsNewest("there".into()).line("Game"),
        "Game: same as the newest backup, from there"
    );
    assert_eq!(Outcome::Unchanged.line("Game"), "Game: up to date");
}

/// A preview of `game(&[1], ..)` against a backup from `there` in which its one file is `state`,
/// and this PC's base is that backup, so a change is its own.
fn compared(state: FileState) -> Preview {
    compared_on(state, Some(sync::Base::new("there-id".into(), None)))
}

/// A base that is not the newest backup and has neither a hash list nor a readable snapshot.
fn forgotten_base() -> sync::Base {
    sync::Base::new("forgotten".into(), None)
}

/// As [`compared`], with this PC's `base`, whose snapshot the repository lists at the epoch.
fn compared_on(state: FileState, base: Option<sync::Base>) -> Preview {
    let hashes = Hashes::from_json(br#"{"format": 1, "meta": "", "files": {}}"#);
    let newest = changes::Baseline::new(snapshot_from("there"), Vec::new(), hashes, None);
    Ok(Some(Baseline {
        newest,
        comparison: Comparison::new(vec![state.clone()], Vec::new()),
        states: HashMap::from([("Home/save".to_owned(), state)]),
        base,
        base_time: Some(SystemTime::UNIX_EPOCH),
        base_backup: None,
    }))
}

/// `preview` as if its newest backup were of the other build, with `unplaced` files left in
/// the folders `unplaced_roots`.
fn of_the_other_build(
    mut preview: Preview,
    unplaced_roots: Vec<(Base, String)>,
    unplaced: &[&str],
) -> Preview {
    if let Ok(Some(baseline)) = &mut preview {
        let hashes = baseline.newest.hashes.clone();
        let unplaced = unplaced.iter().map(|&key| key.to_owned()).collect();
        let sources = BTreeMap::new();
        let mapped = Mapped::new(
            Vec::new(),
            hashes,
            sources,
            unplaced_roots,
            unplaced,
            Vec::new(),
        );
        baseline.newest.mapped = Some(mapped);
    }
    preview
}

#[test]
fn a_backup_of_the_other_build_says_how_its_folders_were_matched() {
    let (game, all) = (game(&[1], false, 0), GameSettings::default());
    let date = "1970-01-01 00:00 UTC";

    let matched = of_the_other_build(compared(FileState::Same), Vec::new(), &[]);
    assert_eq!(
        preview_text(&game, &matched, &all).unwrap(),
        format!(
            "Up to date with the backup from there, {date}; no backup will be made. The backup \
             is from the other build; its folders were matched to this machine's."
        )
    );
    let saves = (Base::WinAppData, "Factorio/saves".to_owned());
    let unplaced = ["WinAppData/Factorio/saves/a.zip"];
    let preview = of_the_other_build(compared(FileState::Changed), vec![saves], &unplaced);
    assert_eq!(
        preview_text(&game, &preview, &all).unwrap(),
        format!(
            "1 changed since the backup from there, {date}. The newest backup is from the other \
             build; these folders have no known place here: <winAppData>/Factorio/saves."
        )
    );
    assert_eq!(
        hint(Some(&preview), unplaced[0], Badge::Removed),
        "Removed: the newest backup is from the other build, and this file has no place here \
         (its folder is unknown, or another file takes its path)"
    );
    assert_eq!(
        hint(Some(&preview), "Home/save", Badge::Removed),
        "Removed: the next backup will not hold it"
    );
    // A file another one pushed aside, in a folder that has a place.
    let unplaced = ["WinDocuments/G/save"];
    let preview = of_the_other_build(compared(FileState::Same), Vec::new(), &unplaced);
    assert!(preview_text(&game, &preview, &all).unwrap().ends_with(
        ". The newest backup is from the other build; some of its files have no place here."
    ));
}

#[test]
fn registry_keys_across_builds_promise_a_backup() {
    let mut game = game(&[1], true, 0);
    let all = GameSettings::default();
    let preview = of_the_other_build(compared(FileState::Same), Vec::new(), &[]);
    let next = next_backup(&game, &preview, &all).unwrap();
    assert_eq!(next.glyph(), Some(icons::EQUALS));
    assert!(sentence(&next).starts_with(
        "No changes to the files since the backup from there, 1970-01-01 00:00 UTC, but a backup \
         will be made, as that backup is from the other build and the registry keys are not \
         compared"
    ));
    // Every key unticked: the backup stores no registry, so the skip is promised.
    let mut unticked = GameSettings::default();
    unticked.registry.insert(KEY.into(), false);
    let next = next_backup(&game, &preview, &unticked).unwrap();
    assert_eq!(next.glyph(), Some(icons::CHECK_CIRCLE));
    game.registry_source = None;
    let next = next_backup(&game, &preview, &all).unwrap();
    assert_eq!(next.glyph(), Some(icons::CHECK_CIRCLE));
}

#[test]
fn the_preview_says_what_the_next_backup_would_do() {
    let (game, all) = (game(&[1], false, 0), GameSettings::default());
    let text = |preview: &Preview| preview_text(&game, preview, &all);
    let date = "1970-01-01 00:00 UTC";

    assert_eq!(
        text(&compared(FileState::Changed)).unwrap(),
        format!("1 changed since the backup from there, {date}")
    );
    assert_eq!(
        text(&compared(FileState::Same)).unwrap(),
        format!("Up to date with the backup from there, {date}; no backup will be made")
    );
    assert_eq!(
        text(&compared_on(FileState::Changed, Some(forgotten_base()))).unwrap(),
        format!(
            "Changed on this PC and on there since this PC last synced ({date}): 1 changed \
             against the backup from there, {date}. Back up asks what to do."
        )
    );
    assert_eq!(
        text(&compared_on(FileState::Changed, None)).unwrap(),
        format!(
            "This PC's saves differ from the newest backup, from there, {date}: 1 changed. \
             salvajocs has no record of this PC syncing this game. Back up asks what to do."
        )
    );
    let newest = snapshot_from("there");
    let mut counts = changes::Counts::default();
    counts.changed = 1;
    assert_eq!(
        sentence(&Next::Behind {
            newest: &newest,
            counts
        }),
        format!(
            "there has newer saves, from {date}: 1 changed. This PC has not changed them since \
             it last synced, so no backup will be made. Restore them on the Restore tab."
        )
    );
    let none = Next::Behind {
        newest: &newest,
        counts: changes::Counts::default(),
    };
    assert!(sentence(&none).starts_with(&format!("there has newer saves, from {date}. This PC")));
    assert_eq!(text(&Ok(None)).unwrap(), "No backup yet");
    assert_eq!(
        text(&Err("offline".into())).unwrap(),
        "Could not compare: offline"
    );

    let mut skipped = GameSettings::default();
    skipped.skip = true;
    assert_eq!(preview_text(&game, &Ok(None), &skipped), None);
    let preview = compared(FileState::Changed);
    assert_eq!(preview_text(&game, &preview, &skipped), None);
}

#[test]
fn without_a_hash_list_the_preview_does_not_promise_a_skip() {
    let game = game(&[1], false, 0);
    let mut preview = compared(FileState::Same);
    if let Ok(Some(baseline)) = &mut preview {
        baseline.newest.hashes = None;
    }
    assert_eq!(
        preview_text(&game, &preview, &GameSettings::default()).unwrap(),
        "No changes since the backup from there, 1970-01-01 00:00 UTC, but a backup will be made, \
         as it has no list of hashes"
    );
}

/// Files equal to a backup that is made again anyway are still judged against the base: this PC
/// may be behind, changed on both, or have no base.
#[test]
fn a_backup_made_anyway_is_judged_against_the_base_too() {
    let (game, all) = (game(&[1], false, 0), GameSettings::default());
    let unhashed = |base| {
        let mut preview = compared_on(FileState::Same, base);
        if let Ok(Some(baseline)) = &mut preview {
            baseline.newest.hashes = None;
        }
        preview
    };
    let date = "1970-01-01 00:00 UTC";

    let ahead = unhashed(Some(sync::Base::new("there-id".into(), None)));
    assert!(matches!(
        next_backup(&game, &ahead, &all),
        Some(Next::Same {
            blocker: Some(_),
            ..
        })
    ));
    let both = unhashed(Some(forgotten_base()));
    let next = next_backup(&game, &both, &all).unwrap();
    assert!(matches!(next, Next::ChangedOnBoth { .. }));
    assert_eq!(
        sentence(&next),
        format!(
            "Changed on this PC and on there since this PC last synced ({date}): no file differs \
             from the backup from there, {date}. Back up asks what to do."
        )
    );
    let unknown = unhashed(None);
    let next = next_backup(&game, &unknown, &all).unwrap();
    assert!(matches!(next, Next::UnknownBase { .. }));
    assert_eq!(
        sentence(&next),
        format!(
            "This PC's files match the newest backup, from there, {date}. salvajocs has no record \
             of this PC syncing this game. Back up asks what to do."
        )
    );
    assert_eq!(change_count(&game, &unknown, &all), Some(0));
}

#[test]
fn a_change_judged_by_mtime_says_so() {
    let game = game(&[1], false, 0);
    let all = GameSettings::default();
    let mut preview = compared(FileState::Changed);
    assert_eq!(
        hint(Some(&preview), "Home/save", Badge::Changed),
        "Changed since the backup"
    );
    assert_eq!(
        preview_text(&game, &preview, &all).unwrap(),
        "1 changed since the backup from there, 1970-01-01 00:00 UTC"
    );

    if let Ok(Some(baseline)) = &mut preview {
        baseline.newest.hashes = None;
    }
    let note = "(by modified time: this backup has no list of hashes)";
    assert_eq!(
        hint(Some(&preview), "Home/save", Badge::Changed),
        format!("Changed since the backup {note}")
    );
    assert_eq!(
        preview_text(&game, &preview, &all).unwrap(),
        format!("1 changed since the backup from there, 1970-01-01 00:00 UTC {note}")
    );
}

/// The header's icon matches what its sentence says the next backup does.
#[test]
fn each_header_state_has_its_icon() {
    let (game, all) = (game(&[1], false, 0), GameSettings::default());
    let glyph = |preview: &Preview| next_backup(&game, preview, &all).unwrap().glyph();

    assert_eq!(glyph(&Ok(None)), Some(icons::PLUS_CIRCLE));
    let same = compared(FileState::Same);
    assert_eq!(glyph(&same), Some(icons::CHECK_CIRCLE));
    let mut unhashed = compared(FileState::Same);
    if let Ok(Some(baseline)) = &mut unhashed {
        baseline.newest.hashes = None;
    }
    assert_eq!(glyph(&unhashed), Some(icons::EQUALS));
    let changed = compared(FileState::Changed);
    assert!(matches!(
        next_backup(&game, &changed, &all),
        Some(Next::Changes { .. })
    ));
    assert_eq!(glyph(&changed), None);
    assert_eq!(glyph(&Err("offline".into())), None);
}

/// Changed on both and an unknown base each have their own mark, neither the check nor Behind's.
#[test]
fn a_game_that_asks_has_its_own_mark_and_never_the_check() {
    let (game, all) = (game(&[1], false, 0), GameSettings::default());

    let both = compared_on(FileState::Changed, Some(forgotten_base()));
    let next = next_backup(&game, &both, &all).unwrap();
    assert!(matches!(next, Next::ChangedOnBoth { .. }));
    assert_eq!(next.glyph(), Some(icons::WARNING));
    let unknown = compared_on(FileState::Changed, None);
    let next = next_backup(&game, &unknown, &all).unwrap();
    assert!(matches!(next, Next::UnknownBase { .. }));
    assert_eq!(next.glyph(), Some(icons::QUESTION));

    let newest = snapshot_from("there");
    let behind = Next::Behind {
        newest: &newest,
        counts: changes::Counts::default(),
    };
    assert_eq!(behind.glyph(), Some(icons::ARROW_DOWN));

    // Each warns and keeps the chips of its differences.
    for preview in [&both, &unknown] {
        let header = header_of(&game, Some(preview), &all);
        let Header::Mark { tone, counts, .. } = header else {
            panic!("a mark");
        };
        assert!(matches!(tone, Tone::Warn));
        assert_eq!(counts.changed, 1);
    }
}

#[test]
fn a_file_that_could_not_be_compared_is_shown_and_never_skipped() {
    let game = game(&[1], false, 0);
    let all = GameSettings::default();
    let preview = compared(FileState::Unknown("denied".into()));
    assert_eq!(
        preview_text(&game, &preview, &all).unwrap(),
        "1 could not be compared since the backup from there, 1970-01-01 00:00 UTC"
    );
    assert_eq!(
        badges(&game, Some(&preview), &all)("Home/save"),
        Some(Badge::Unknown)
    );
    assert_eq!(
        hint(Some(&preview), "Home/save", Badge::Unknown),
        "Could not be compared: denied"
    );
}

#[test]
fn a_replaced_repository_is_compared_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(&dir.path().join("first"));
    let ctx = egui::Context::default();
    let mut tab = listing(&["Game"]);
    tab.start_preview(&ctx, &state);
    tab.previews = vec![compared(FileState::Same)];

    let replaced = self::state(&dir.path().join("second")).session;
    state.session = replaced;
    tab.poll(&ctx, &mut state);

    let current = &state.session.as_ref().unwrap().storage;
    assert!(Arc::ptr_eq(tab.compared_in.as_ref().unwrap(), current));
    assert!(tab.previews.is_empty());
    assert!(tab.previewing.is_some());
}

/// A header is worked out once, not every frame: a preview swapped behind the tab's back shows
/// only once a change to the game's settings invalidates it.
#[test]
fn a_header_is_worked_out_again_only_when_its_game_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut tab = listing(&["Game"]);
    tab.previews = vec![compared(FileState::Changed)];
    let mut harness = theme::harness(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (tab, state(dir.path())),
    );
    settle(&mut harness);
    let shows = |harness: &Harness<'_, (BackupTab, State)>, text| {
        harness.query_all_by_label_contains(text).next().is_some()
    };
    assert!(shows(&harness, "1 changed since the backup from there"));

    harness.state_mut().0.previews = vec![compared(FileState::Same)];
    harness.step();
    assert!(shows(&harness, "1 changed since the backup from there"));

    // Unticking its one file leaves nothing to back up, so the header goes; ticking it again
    // shows the swapped preview.
    harness.get_by_label("1 file, 1 B").click();
    harness.step();
    harness.get_by_label("Home").click();
    settle(&mut harness);
    assert!(!shows(&harness, "since the backup from there"));
    harness.get_by_label("Home").click();
    settle(&mut harness);
    assert!(shows(&harness, "Up to date with the backup from there"));
}

/// A header drawn with the default settings while the repository's settings load is worked
/// out again once they are loaded.
#[test]
fn loaded_settings_work_the_headers_out_again() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let storage = state.session.as_ref().unwrap().storage.clone();
    let stage = dir.path().join("stage");
    let skip = [Change::Skip(true)];
    selection::save(&mut lock_storage(&storage).unwrap(), "Game", &skip, &stage).unwrap();
    let mut tab = listing(&["Game"]);
    tab.previews = vec![compared(FileState::Changed)];
    let mut harness = theme::harness(|ui, (tab, state)| tab.ui(ui, state, false), (tab, state));
    let shows = |harness: &Harness<'_, (BackupTab, State)>, text| {
        harness.query_all_by_label_contains(text).next().is_some()
    };

    // The first frame starts the load, so it draws the header with every file included.
    harness.step();
    assert!(shows(&harness, "1 changed since the backup from there"));
    settle(&mut harness);
    assert!(!shows(&harness, "since the backup from there"));
}

#[test]
fn an_unticked_file_in_the_backup_is_badged_removed_unless_staging_keeps_it() {
    let preview = compared(FileState::Same);
    let mut settings = GameSettings::default();
    settings.set_path("Home/save", false);
    let game = game(&[1], false, 0);
    assert_eq!(
        badges(&game, Some(&preview), &settings)("Home/save"),
        Some(Badge::Removed)
    );
    let unreadable = self::game(&[1], false, 1);
    assert_eq!(
        badges(&unreadable, Some(&preview), &settings)("Home/save"),
        None
    );
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
        let text = describe_problem(&ScanProblem::new(path.clone(), kind));
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

fn is_disabled(harness: &Harness<'_, (BackupTab, State)>, label: &str) -> bool {
    harness.get_by_label(label).accesskit_node().is_disabled()
}

/// Draws the tab and its part of the bottom bar, as the app does.
fn with_bar(ui: &mut Ui, tab: &mut BackupTab, state: &mut State, manifest_loading: bool) {
    let has_bar = tab.has_bar(state);
    let bar = status_bar::reserve(ui, has_bar);
    tab.ui(ui, state, manifest_loading);
    if has_bar {
        tab.bar_ui(ui, bar, state, manifest_loading, |_| {}, |_| {});
    }
}

/// A tab listing a game of one file for each of `titles`.
fn listing(titles: &[&str]) -> BackupTab {
    let mut tab = BackupTab::default();
    tab.adopt(ScanReport::new(
        titles
            .iter()
            .map(|title| {
                let mut game = game(&[1], false, 0);
                game.title = (*title).into();
                game
            })
            .collect(),
        Vec::new(),
    ));
    tab
}

#[test]
fn the_order_is_built_again_only_when_the_report_the_figures_the_sort_or_the_previews_under_changes_change()
 {
    let mut tab = listing(&["B", "A"]);
    let name = ListSort::default();
    tab.sort_rows(name);
    assert_eq!(tab.order, [1, 0]);
    // A frame with nothing changed keeps the order it has.
    tab.order = vec![9];
    tab.sort_rows(name);
    assert_eq!(tab.order, [9]);

    let report = listing(&["C", "B", "A"]).report.unwrap();
    tab.adopt(report);
    tab.sort_rows(name);
    assert_eq!(tab.order, [2, 1, 0]);

    let newest = ListSort::new(SortKey::LastBackup, true);
    tab.sort_rows(newest);
    tab.order = vec![9];
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    state.session = None;
    let backed_up = |title: &str, seconds| {
        let tags = vec!["salvajocs".into(), format!("game:{}", slug(title))];
        let time = std::time::UNIX_EPOCH + Duration::from_secs(seconds);
        SnapshotInfo::new(title.into(), time, "pc".into(), title.into(), tags)
    };
    let figures = stats::by_game(&[backed_up("B", 2), backed_up("C", 1)]);
    tab.adopt_stats(&egui::Context::default(), &state, figures);
    tab.sort_rows(newest);
    assert_eq!(
        tab.order,
        [1, 0, 2],
        "newest first, the game without a backup last"
    );
}

/// Hands `tab` the result of a comparison job that ends with `previews`, and polls it once.
fn deliver_previews(
    tab: &mut BackupTab,
    ctx: &egui::Context,
    state: &mut State,
    previews: Vec<Preview>,
) {
    tab.previewing = Some(Job::spawn(ctx, move || Ok(previews)));
    let deadline = Instant::now() + Duration::from_secs(60);
    while tab.previewing.as_mut().is_some_and(|job| job.is_running()) {
        assert!(Instant::now() < deadline, "the previews never arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
    tab.poll(ctx, state);
    assert!(tab.previewing.is_none());
}

#[test]
fn the_order_by_changes_follows_the_previews_and_the_settings_but_not_a_tick() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    let most = ListSort::new(SortKey::Changes, true);
    state.config.as_mut().unwrap().backup_sort = most;
    let mut tab = listing(&["B", "A", "C"]);
    let ctx = egui::Context::default();
    tab.sort_rows(most);
    assert_eq!(tab.order, [1, 0, 2], "no preview yet: by title");

    // B has a changed file, A is unchanged, C has no backup yet. The settings load starts with
    // this poll and ends in a later one.
    let previews = vec![
        compared(FileState::Changed),
        compared(FileState::Same),
        Ok(None),
    ];
    deliver_previews(&mut tab, &ctx, &mut state, previews);
    assert!(!tab.choices.loaded());
    tab.sort_rows(most);
    assert_eq!(tab.order, [0, 2, 1], "C's one new file ties B, A has none");
    tab.sort_rows(ListSort::new(SortKey::Changes, false));
    assert_eq!(tab.order, [1, 0, 2]);
    tab.sort_rows(most);

    // The settings loading rebuilds the order, which was built with every file included.
    let deadline = Instant::now() + Duration::from_secs(60);
    while !tab.choices.loaded() {
        assert!(Instant::now() < deadline, "the settings never loaded");
        std::thread::sleep(Duration::from_millis(5));
        tab.poll(&ctx, &mut state);
    }
    tab.order = vec![9];
    tab.sort_rows(most);
    assert_eq!(tab.order, [0, 2, 1]);

    // A tick does not move a game under the cursor; the next rebuild counts it.
    tab.choices.change(&ctx, &state, "B", Change::Skip(true));
    tab.sort_rows(most);
    assert_eq!(tab.order, [0, 2, 1]);
    tab.generation += 1;
    tab.sort_rows(most);
    assert_eq!(tab.order, [2, 1, 0], "B is skipped now");
    while tab.choices.busy() {
        std::thread::sleep(Duration::from_millis(5));
        tab.poll(&ctx, &mut state);
    }
}

#[test]
fn the_previews_arriving_do_not_reorder_another_sort() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    let by_space = ListSort::new(SortKey::SpaceUsed, true);
    state.config.as_mut().unwrap().backup_sort = by_space;
    let mut tab = listing(&["B", "A"]);
    tab.sort_rows(by_space);
    tab.order = vec![9];

    let previews = vec![compared(FileState::Changed), Ok(None)];
    deliver_previews(&mut tab, &egui::Context::default(), &mut state, previews);
    tab.sort_rows(by_space);
    assert_eq!(
        tab.order,
        [9],
        "the order is kept until every space figure is in"
    );
}

#[test]
fn change_count_counts_what_the_header_counts() {
    let all = GameSettings::default();
    let count = |game: &GameScan, preview: &Preview, settings: &GameSettings| {
        change_count(game, preview, settings)
    };
    let one = game(&[1], false, 0);
    assert_eq!(count(&one, &Err("broken".into()), &all), None);
    let mut skipped = GameSettings::default();
    skipped.skip = true;
    assert_eq!(count(&one, &compared(FileState::Changed), &skipped), None);
    assert_eq!(count(&one, &compared(FileState::Same), &all), Some(0));
    // Those that ask, and a game that is behind, are counted against the newest.
    let both = compared_on(FileState::Changed, Some(forgotten_base()));
    assert_eq!(count(&one, &both, &all), Some(1));
    assert_eq!(
        count(&one, &compared_on(FileState::Changed, None), &all),
        Some(1)
    );
    assert_eq!(count(&one, &compared(FileState::Changed), &all), Some(1));

    // No backup yet: the included files, and the registry as one.
    let mut two = game(&[1, 2], false, 0);
    two.files[1] = FoundFile::new(Base::Home, "other".into(), PathBuf::from("/other"), 2, None);
    assert_eq!(count(&two, &Ok(None), &all), Some(2));
    let mut excluded = GameSettings::default();
    excluded.set_path("Home/other", false);
    assert_eq!(count(&two, &Ok(None), &excluded), Some(1));
    assert_eq!(count(&game(&[], true, 0), &Ok(None), &all), Some(1));
    assert_eq!(count(&game(&[1], true, 0), &Ok(None), &all), Some(2));
    let mut no_registry = GameSettings::default();
    no_registry.set_registry(KEY, false);
    assert_eq!(
        count(&game(&[1], true, 0), &Ok(None), &no_registry),
        Some(1)
    );
    assert_eq!(count(&game(&[], true, 0), &Ok(None), &no_registry), None);
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
        |ui, (tab, state)| with_bar(ui, tab, state, false),
        (listing(&["Game"]), state(dir.path())),
    );
    settle(&mut harness);
    assert!(!is_disabled(&harness, "Scan for games"));
    assert!(!is_disabled(&harness, "Back up 1 game"));

    let (release, hold) = mpsc::channel::<()>();
    let report = ScanReport::new(Vec::new(), Vec::new());
    harness.state_mut().0.running = Some(Job::spawn(&harness.ctx.clone(), move || {
        let _ = hold.recv();
        Ok(Backed {
            source: std::sync::Weak::new(),
            shared: Shared::default(),
            report: Some(report),
            error: None,
            saving: SaveProblems::default(),
            after: Some(AfterBackup {
                source: std::sync::Weak::new(),
                pending: None,
                written: None,
                games: None,
            }),
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

#[test]
fn a_scan_counts_the_games_it_checked() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    let manifest = salvajocs_core::manifest::parse_yaml("Bar: {}\nFoo: {}\n").unwrap();
    state.manifest = Some(Arc::new(manifest));
    state.env.environment = Some(Environment::default());
    let mut harness = theme::harness(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (BackupTab::default(), state),
    );
    harness.step();
    harness.get_by_label("Scan for games").click();
    settle(&mut harness);
    assert_eq!(*lock(&harness.state().0.checked), (2, 2));
}

#[test]
fn a_running_scan_shows_a_bar_of_the_games_checked() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
        |ui, (tab, state)| tab.ui(ui, state, false),
        (BackupTab::default(), state(dir.path())),
    );
    let (release, job) = crate::job::held(&harness.ctx.clone());
    let tab = &mut harness.state_mut().0;
    tab.scanning = Some(job);
    *lock(&tab.checked) = (3, 10);
    harness.step();
    let bar = harness.get_by_label("Checked 3 of 10 games");
    assert_eq!(bar.accesskit_node().role(), Role::ProgressIndicator);
    drop(release);
}

#[test]
fn a_backup_shows_the_games_its_scan_checked() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
        |ui, (tab, state)| with_bar(ui, tab, state, false),
        (listing(&["Game"]), state(dir.path())),
    );
    settle(&mut harness);
    let ctx = harness.ctx.clone();
    let release = harness.state_mut().0.hold_backup(&ctx);
    *lock(&harness.state().0.checked) = (3, 10);
    harness.step();
    assert!(harness.query_by_label("Checked 3 of 10 games").is_some());

    *lock(&harness.state().0.checked) = (10, 10);
    harness.step();
    assert!(harness.query_by_label("Backed up 0 of 0 games").is_some());
    drop(release);
}

/// Before a scan the empty state's scan is the one action: no second scan button, and no
/// backup button for zero games.
#[test]
fn before_a_scan_the_only_action_is_one_enabled_scan() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
        |ui, (tab, state)| with_bar(ui, tab, state, false),
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
        report: Some(ScanReport::new(Vec::new(), Vec::new())),
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

/// The bottom bar marks the repository encrypted exactly when it has a password.
#[test]
fn the_bottom_bar_says_whether_the_repository_is_encrypted() {
    for (password, shown, hidden) in [
        (PasswordMode::None, "Not encrypted", "Encrypted"),
        (PasswordMode::Keyring, "Encrypted", "Not encrypted"),
        (PasswordMode::Ask, "Encrypted", "Not encrypted"),
    ] {
        let tab = BackupTab {
            report: Some(ScanReport::new(Vec::new(), Vec::new())),
            ..BackupTab::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let mut state = state(dir.path());
        state.config.as_mut().unwrap().password = password;
        let mut harness = theme::harness(
            |ui, (tab, state)| with_bar(ui, tab, state, false),
            (tab, state),
        );
        harness.step();
        assert!(harness.query_by_label(shown).is_some(), "{password:?}");
        assert!(harness.query_by_label(hidden).is_none(), "{password:?}");
    }
}

#[test]
fn back_up_waits_while_what_to_back_up_loads() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = theme::harness(
        |ui, (tab, state)| with_bar(ui, tab, state, false),
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
        |ui, (tab, state)| with_bar(ui, tab, state, false),
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
        let location = RepoLocation::new(other.clone(), Default::default());
        let storage = Storage::init(&location, "", Compression::Default).unwrap();
        let first = state.session.replace(Session {
            compression: storage.compression(),
            storage: Arc::new(Mutex::new(storage)),
            space: Default::default(),
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
    let modified = std::fs::metadata(&real).unwrap().modified().ok();
    let mut game = GameScan::default();
    game.title = title.into();
    game.files = vec![FoundFile::new(
        Base::Home,
        "save".into(),
        real,
        content.len() as u64,
        modified,
    )];
    game
}

/// A new repository in `dir`, shared as a session shares it.
fn shared_storage(dir: &Path) -> SharedStorage {
    let location = RepoLocation::new(
        dir.join("repo").to_str().unwrap().into(),
        Default::default(),
    );
    let storage = Storage::init(&location, "", Compression::Default).unwrap();
    Arc::new(Mutex::new(storage))
}

/// What a [`Mapper`] that maps nothing is made of: no manifest, custom games or folders.
fn no_mapping() -> (Manifest, AppConfig, Environment) {
    let config = AppConfig::new("/repo".into(), PasswordMode::None);
    (Manifest::default(), config, Environment::default())
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
    let (manifest, config, env) = no_mapping();
    let mapper = Mapper::new(&manifest, &config, &env);
    let same = saved_game(dir.path(), "Same", "a");
    let games = [same.clone(), saved_game(dir.path(), "Changed", "b")];
    back_up(
        &storage,
        &games,
        &stage,
        &mapper,
        dir.path(),
        "repo",
        |_| {},
    )
    .unwrap();
    let failing = saved_game(dir.path(), "Failing", "c");
    // A file where its staging dir belongs makes staging it fail.
    std::fs::write(stage.join(salvajocs_core::slug::slug("Failing")), "").unwrap();

    let games = [same, saved_game(dir.path(), "Changed", "b2"), failing];
    let (made, _) = back_up(
        &storage,
        &games,
        &stage,
        &mapper,
        dir.path(),
        "repo",
        |_| {},
    )
    .unwrap();

    assert_eq!(made, snapshot_ids(&storage, "Changed")[..1]);
}

/// The repository in `dir` with two backups of "Game", of the contents `a` then `bb`, and their
/// ids, the older first. The bases they record go to a folder the tests do not read, so each test
/// sets this PC's base.
fn two_backups(dir: &Path) -> (SharedStorage, Vec<String>) {
    let storage = shared_storage(dir);
    let stage = dir.join("stage");
    let (manifest, config, env) = no_mapping();
    let mapper = Mapper::new(&manifest, &config, &env);
    for content in ["a", "bb"] {
        let games = [saved_game(dir, "Game", content)];
        let recorded = dir.join("setup-bases");
        back_up(&storage, &games, &stage, &mapper, &recorded, "repo", |_| {}).unwrap();
    }
    let mut ids = snapshot_ids(&storage, "Game");
    ids.reverse();
    (storage, ids)
}

/// What the Backup tab shows of "Game" holding some content.
struct Shown {
    glyph: Option<&'static str>,
    text: String,
    /// The count it sorts by.
    count: Option<usize>,
    warns: bool,
    /// Whether the preview job read the base's snapshot.
    read_base: bool,
}

/// What the Backup tab shows of "Game" holding `content` when this PC's `base` is recorded.
fn shown(dir: &Path, storage: &SharedStorage, base: Option<sync::Base>, content: &str) -> Shown {
    let config_dir = dir.join("config");
    if let Some(base) = base {
        sync::record(&config_dir, "repo", [(slug("Game"), base)]).unwrap();
    }
    let game = saved_game(dir, "Game", content);
    let (manifest, config, env) = no_mapping();
    let mapper = Mapper::new(&manifest, &config, &env);
    let games = std::slice::from_ref(&game);
    let previews = preview(storage, games, &mapper, (&config_dir, "repo"), |_| {}).unwrap();
    let all = GameSettings::default();
    let next = next_backup(&game, &previews[0], &all).unwrap();
    let header = header_of(&game, Some(&previews[0]), &all);
    let Ok(Some(baseline)) = &previews[0] else {
        panic!("the game has a backup");
    };
    Shown {
        glyph: next.glyph(),
        text: sentence(&next),
        count: change_count(&game, &previews[0], &all),
        warns: matches!(
            header,
            Header::Mark {
                tone: Tone::Warn,
                ..
            }
        ),
        read_base: baseline.base_backup.is_some(),
    }
}

/// A base on the snapshot `id` without a hash list.
fn base_on(id: &str) -> sync::Base {
    sync::Base::new(id.to_owned(), None)
}

/// A base on the snapshot `id` of "Game" that holds the hash list of that backup, as one set by
/// a Back up.
fn base_with_list(dir: &Path, storage: &SharedStorage, id: &str) -> sync::Base {
    let game = saved_game(dir, "Game", "a");
    let (manifest, config, env) = no_mapping();
    let mapper = Mapper::new(&manifest, &config, &env);
    let mut storage = lock_storage(storage).unwrap();
    let snapshots = storage.snapshots("salvajocs").unwrap();
    let info = snapshots.iter().find(|snapshot| snapshot.id == id).unwrap();
    let list = mapper.baseline(&mut storage, info, &game).unwrap().hashes;
    sync::Base::new(id.to_owned(), list)
}

/// The reported case: another PC's newer backup differs and this PC's saves are those it last
/// synced, so the header warns and offers the restore instead of showing a check.
#[test]
fn saves_this_pc_has_not_changed_since_a_newer_backup_elsewhere_are_behind() {
    let dir = tempfile::tempdir().unwrap();
    let (storage, ids) = two_backups(dir.path());

    let shown = shown(dir.path(), &storage, Some(base_on(&ids[0])), "a");

    assert_eq!(shown.glyph, Some(icons::ARROW_DOWN));
    assert!(shown.warns);
    let text = shown.text;
    assert!(text.contains(" has newer saves, from "), "{text}");
    assert!(text.ends_with("no backup will be made. Restore them on the Restore tab."));
    assert_eq!(shown.count, Some(1));
    assert!(shown.read_base, "a base without a list needs its backup");
}

#[test]
fn a_base_with_a_hash_list_is_not_read_from_the_repository() {
    let dir = tempfile::tempdir().unwrap();
    let (storage, ids) = two_backups(dir.path());
    let base = base_with_list(dir.path(), &storage, &ids[0]);

    let shown = shown(dir.path(), &storage, Some(base), "a");

    assert_eq!(shown.glyph, Some(icons::ARROW_DOWN));
    assert!(!shown.read_base);
}

#[test]
fn saves_changed_since_the_base_on_top_of_a_newer_backup_are_changed_on_both() {
    let dir = tempfile::tempdir().unwrap();
    let (storage, ids) = two_backups(dir.path());

    let shown = shown(dir.path(), &storage, Some(base_on(&ids[0])), "ccc");

    assert_eq!(shown.glyph, Some(icons::WARNING));
    assert!(shown.warns);
    assert!(shown.text.starts_with("Changed on this PC and on "));
    assert!(shown.text.ends_with("Back up asks what to do."));
    assert_eq!(shown.count, Some(1));
}

#[test]
fn saves_that_differ_from_the_newest_without_a_base_ask_with_an_unknown_base() {
    let dir = tempfile::tempdir().unwrap();
    let (storage, _) = two_backups(dir.path());

    let shown = shown(dir.path(), &storage, None, "ccc");

    assert_eq!(shown.glyph, Some(icons::QUESTION));
    assert!(shown.warns);
    let text = shown.text;
    assert!(
        text.contains("has no record of this PC syncing this game"),
        "{text}"
    );
    assert_eq!(shown.count, Some(1));
}

#[test]
fn saves_changed_since_a_base_that_is_the_newest_backup_are_ahead() {
    let dir = tempfile::tempdir().unwrap();
    let (storage, ids) = two_backups(dir.path());

    let shown = shown(dir.path(), &storage, Some(base_on(&ids[1])), "ccc");

    assert_eq!(shown.glyph, None);
    assert!(!shown.warns);
    let text = shown.text;
    assert!(
        text.starts_with("1 changed since the backup from "),
        "{text}"
    );
    assert_eq!(shown.count, Some(1));
    assert!(!shown.read_base);
}

#[test]
fn saves_equal_to_the_newest_backup_are_up_to_date_whatever_the_base() {
    let dir = tempfile::tempdir().unwrap();
    let (storage, ids) = two_backups(dir.path());

    let shown = shown(dir.path(), &storage, Some(base_on(&ids[0])), "bb");

    assert_eq!(shown.glyph, Some(icons::CHECK_CIRCLE));
    assert!(shown.text.starts_with("Up to date with the backup from "));
    assert!(shown.text.ends_with("; no backup will be made"));
    assert_eq!(shown.count, Some(0));
    assert!(!shown.read_base);
}

#[test]
fn clean_up_never_forgets_the_snapshots_it_is_given() {
    let dir = tempfile::tempdir().unwrap();
    let storage = shared_storage(dir.path());
    let stage = dir.path().join("stage");
    let (manifest, config, env) = no_mapping();
    let mapper = Mapper::new(&manifest, &config, &env);
    for content in ["old", "new"] {
        let games = [saved_game(dir.path(), "Game", content)];
        back_up(
            &storage,
            &games,
            &stage,
            &mapper,
            dir.path(),
            "repo",
            |_| {},
        )
        .unwrap();
    }
    let ids = snapshot_ids(&storage, "Game");
    let mut policy = Retention::default();
    policy.keep_last = std::num::NonZeroU32::new(1);
    retention::save(&mut lock_storage(&storage).unwrap(), &policy, &stage).unwrap();

    let cleaned = clean_up(&storage, std::slice::from_ref(&ids[1]), false);

    assert!(cleaned.error.is_none());
    assert_eq!(cleaned.done.forgotten, 0);
    assert_eq!(snapshot_ids(&storage, "Game"), ids);
}

#[test]
fn a_new_scan_closes_the_open_details() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let storage = state.session.as_ref().unwrap().storage.clone();
    let (manifest, config, env) = no_mapping();
    let mapper = Mapper::new(&manifest, &config, &env);
    let game = saved_game(dir.path(), "Game", "old");
    let stage = dir.path().join("stage");
    let games = std::slice::from_ref(&game);
    back_up(&storage, games, &stage, &mapper, dir.path(), "repo", |_| {}).unwrap();
    let previews = preview(&storage, games, &mapper, (dir.path(), "repo"), |_| {}).unwrap();
    let Ok(Some(baseline)) = &previews[0] else {
        panic!("the game has a backup");
    };
    let backed_up = baseline.newest.backed_up("Home/save").unwrap();
    let report = ScanReport::new(vec![game.clone()], Vec::new());
    let mut tab = BackupTab::default();
    tab.adopt(report.clone());
    let mut harness = theme::harness(|ui, (tab, state)| tab.ui(ui, state, false), (tab, state));
    let ctx = harness.ctx.clone();
    let key = selection::path_key(&game.files[0]);
    let local = game.files[0].real.clone();
    let details = Details::start(&ctx, &storage, slug(&game.title), key, local, backed_up);
    harness.state_mut().0.details = Some(details);
    let deadline = Instant::now() + Duration::from_secs(60);
    while harness.query_by_label("Close details").is_none() {
        assert!(Instant::now() < deadline, "the dialog never showed");
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }

    harness.state_mut().0.adopt(report);
    harness.step();

    assert!(harness.state().0.details.is_none());
    assert!(harness.query_by_label("Close details").is_none());
}

type TabHarness = Harness<'static, (BackupTab, State)>;

/// A custom game `name` whose one skipped folder is `<xdgData>/<skip>`.
fn custom(name: &str, skip: &str) -> CustomGame {
    let mut game = CustomGame::new(name.into(), Default::default(), Integration::Extend);
    game.skips = vec![format!("<xdgData>/{skip}")];
    game
}

fn without_prefix(game: &CustomGame) -> CustomGame {
    let mut game = game.clone();
    game.wine_prefix = None;
    game
}

/// The repository in `dir` as another PC opens it.
fn other_pc(dir: &Path) -> Storage {
    let location = RepoLocation::new(
        dir.join("repo").to_str().unwrap().to_string(),
        Default::default(),
    );
    Storage::open(&location, "").unwrap()
}

/// Saves `game` (`None`: removes it) over `based_on` in the repository in `dir`, as another PC.
fn save_elsewhere(dir: &Path, based_on: Option<&CustomGame>, game: Option<&CustomGame>) {
    let name = (game.or(based_on)).unwrap().name.clone();
    let stage = dir.join("elsewhere");
    custom_games::save(&mut other_pc(dir), &name, based_on, game, &stage).unwrap();
}

fn settings_snapshots(dir: &Path) -> usize {
    other_pc(dir).snapshots("salvajocs-settings").unwrap().len()
}

fn stored_game(dir: &Path, name: &str) -> Option<custom_games::Stored> {
    let mut listed = custom_games::load_all(&mut other_pc(dir)).unwrap();
    listed.remove(name).map(Result::unwrap)
}

fn keep_last(n: u32) -> Retention {
    let mut policy = Retention::default();
    policy.keep_last = std::num::NonZeroU32::new(n);
    policy
}

fn step_until(harness: &mut TabHarness, what: &str, ready: impl Fn(&TabHarness) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        harness.step();
        if ready(harness) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The tab of a scan that found no game, with the bottom bar, over `state`, its jobs settled.
fn no_games_found(state: State) -> TabHarness {
    let mut harness = theme::harness(
        |ui, (tab, state)| with_bar(ui, tab, state, false),
        (listing(&[]), state),
    );
    settle(&mut harness);
    harness
}

/// The enabled button `label`, so a dialog's button is told from the disabled one behind it.
fn click_enabled(harness: &TabHarness, label: &str) {
    let button = harness
        .query_all_by_label(label)
        .find(|node| !node.accesskit_node().is_disabled());
    button
        .unwrap_or_else(|| panic!("no enabled {label}"))
        .click();
}

fn asking(harness: &TabHarness) -> bool {
    harness.query_by_label("Changed on another PC").is_some()
}

fn backed_up(harness: &TabHarness) -> bool {
    let tab = &harness.state().0;
    tab.running.is_none() && tab.checking.is_none() && !tab.backup_requested
}

/// A state where this PC edited the custom game Game from `base` to `mine`, and another PC
/// changed it to `theirs` since; with the three versions.
fn game_conflict(dir: &Path) -> (State, CustomGame, CustomGame, CustomGame) {
    let mut state = state(dir);
    let base = custom("Game", "Base");
    let theirs = custom("Game", "Theirs");
    save_elsewhere(dir, None, Some(&base));
    save_elsewhere(dir, Some(&base), Some(&theirs));
    let mut mine = custom("Game", "Mine");
    mine.wine_prefix = Some("/prefixes/game".into());
    let config = state.config.as_mut().unwrap();
    config.custom_games = vec![mine.clone()];
    config.custom_games_unsent = vec![custom_games::Unsent::new("Game".into(), Some(base))];
    (state, mine, theirs, custom("Game", "Base"))
}

#[test]
fn a_change_made_on_another_pc_is_asked_and_cancel_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _, _, base) = game_conflict(dir.path());
    let before = settings_snapshots(dir.path());
    let mut harness = no_games_found(state);

    harness.get_by_label("Back up").click();
    step_until(&mut harness, "the question", asking);
    for line in ["<xdgData>/Mine", "<xdgData>/Theirs"] {
        let shown = format!("Skipped folder: {line}");
        assert!(harness.query_by_label(&shown).is_some(), "{shown}");
    }
    assert!(
        harness
            .query_all_by_label("Back up")
            .all(|node| node.accesskit_node().is_disabled()),
        "nothing is chosen yet"
    );

    harness.get_by_label("Cancel").click();
    settle(&mut harness);
    assert!(!asking(&harness));
    assert!(harness.state().0.running.is_none());
    assert_eq!(settings_snapshots(dir.path()), before);
    let config = harness.state().1.config.as_ref().unwrap();
    assert_eq!(
        config.custom_games_unsent,
        [custom_games::Unsent::new("Game".into(), Some(base))]
    );
}

#[test]
fn keeping_this_pcs_version_saves_it_over_the_other_pcs() {
    let dir = tempfile::tempdir().unwrap();
    let (state, mine, _, _) = game_conflict(dir.path());
    let mut harness = no_games_found(state);
    harness.get_by_label("Back up").click();
    step_until(&mut harness, "the question", asking);

    harness.get_by_label("Keep this PC's").click();
    harness.step();
    click_enabled(&harness, "Back up");
    step_until(&mut harness, "the backup", backed_up);
    settle(&mut harness);

    assert!(!asking(&harness));
    let expected = custom_games::Stored::Game(Box::new(without_prefix(&mine)));
    assert_eq!(stored_game(dir.path(), "Game"), Some(expected));
    let config = harness.state().1.config.as_ref().unwrap();
    assert_eq!(config.custom_games, [mine]);
    assert!(config.custom_games_unsent.is_empty());
    let saved = "Saved 1 custom game to the repository";
    assert!(harness.query_by_label(saved).is_some());
}

#[test]
fn keeping_the_repositorys_version_takes_it_with_this_pcs_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let (state, mine, theirs, _) = game_conflict(dir.path());
    let before = settings_snapshots(dir.path());
    let mut harness = no_games_found(state);
    harness.get_by_label("Back up").click();
    step_until(&mut harness, "the question", asking);

    harness.get_by_label("Keep the repository's").click();
    harness.step();
    click_enabled(&harness, "Back up");
    step_until(&mut harness, "the backup", backed_up);
    settle(&mut harness);

    let mut expected = theirs.clone();
    expected.wine_prefix = mine.wine_prefix;
    let config = harness.state().1.config.as_ref().unwrap();
    assert_eq!(config.custom_games, [expected]);
    assert!(config.custom_games_unsent.is_empty());
    assert_eq!(
        settings_snapshots(dir.path()),
        before,
        "nothing was written"
    );
}

#[test]
fn rules_changed_on_another_pc_are_asked_in_the_same_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    retention::save(
        &mut other_pc(dir.path()),
        &keep_last(5),
        &dir.path().join("x"),
    )
    .unwrap();
    let unsent = retention::Unsent::new(keep_last(3), Retention::default());
    state.config.as_mut().unwrap().retention_unsent = Some(unsent);
    let mut harness = no_games_found(state);
    harness.get_by_label("Back up").click();
    step_until(&mut harness, "the question", asking);
    for line in [
        "Keep the last 3 backups of each game",
        "Keep the last 5 backups of each game",
    ] {
        assert!(harness.query_by_label(line).is_some(), "{line}");
    }

    harness.get_by_label("Keep this PC's").click();
    harness.step();
    click_enabled(&harness, "Back up");
    step_until(&mut harness, "the backup", backed_up);
    settle(&mut harness);

    assert_eq!(
        retention::load(&mut other_pc(dir.path())).unwrap(),
        keep_last(3)
    );
    assert_eq!(
        harness.state().1.config.as_ref().unwrap().retention_unsent,
        None
    );
    assert!(
        harness
            .query_by_label("Saved the Old backups rules")
            .is_some()
    );
}

#[test]
fn with_no_game_chosen_back_up_only_saves_the_unsent_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    let mine = custom("Mine", "Mine");
    let config = state.config.as_mut().unwrap();
    config.custom_games = vec![mine.clone()];
    config.custom_games_unsent = vec![custom_games::Unsent::new("Mine".into(), None)];
    // No scan yet: the bar shows for the unsent change alone.
    let mut harness = theme::harness(
        |ui, (tab, state)| with_bar(ui, tab, state, false),
        (BackupTab::default(), state),
    );
    settle(&mut harness);

    assert!(!is_disabled(&harness, "Back up"));
    harness.get_by_label("Back up").click();
    step_until(&mut harness, "the backup", backed_up);
    settle(&mut harness);

    let expected = custom_games::Stored::Game(Box::new(mine));
    assert_eq!(stored_game(dir.path(), "Mine"), Some(expected));
    assert!(
        other_pc(dir.path())
            .snapshots("salvajocs")
            .unwrap()
            .is_empty()
    );
    let config = harness.state().1.config.as_ref().unwrap();
    assert!(config.custom_games_unsent.is_empty());
    let saved = AppConfig::load(&dir.path().join("config"))
        .unwrap()
        .unwrap();
    assert!(saved.custom_games_unsent.is_empty(), "the config is saved");
    assert!(
        harness
            .query_all_by_label_contains("Back up")
            .next()
            .is_none()
    );
}

#[test]
fn the_backup_after_the_check_waits_for_a_pending_write() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    let config = state.config.as_mut().unwrap();
    config.custom_games = vec![custom("Mine", "Mine")];
    config.custom_games_unsent = vec![custom_games::Unsent::new("Mine".into(), None)];
    let mut harness = no_games_found(state);
    let ctx = harness.ctx.clone();
    let storage = harness.state().1.session.as_ref().unwrap().storage.clone();

    let held = storage.lock().unwrap();
    harness.get_by_label("Back up").click();
    step_until(&mut harness, "the check", |h| {
        h.state().0.checking.is_some()
    });
    // A choice made while the check runs is being written when it ends.
    let release = harness.state_mut().0.choices.hold_write(&ctx);
    harness.step();
    assert!(
        is_disabled(&harness, "Scan for games"),
        "while the check runs"
    );
    drop(held);
    step_until(&mut harness, "the check to end", |h| {
        h.state().0.checking.is_none()
    });
    harness.step();
    assert!(harness.state().0.running.is_none(), "waits for the write");
    assert!(harness.state().0.cleared.is_some());
    assert!(
        is_disabled(&harness, "Scan for games"),
        "while the backup waits"
    );

    drop(release);
    step_until(&mut harness, "the backup", |h| {
        h.state().0.cleared.is_none() && backed_up(h)
    });
    settle(&mut harness);
    assert!(stored_game(dir.path(), "Mine").is_some());
}

/// Writes rules a newer salvajocs would store into the repository in `dir`.
fn newer_rules(dir: &Path) {
    let staged = dir.join("newer");
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::write(staged.join("retention.json"), r#"{ "format": 2 }"#).unwrap();
    let tags = ["salvajocs-settings", "settings:retention"];
    let as_path = "/salvajocs-settings/retention";
    let written = other_pc(dir).backup_dir(&staged, as_path, &tags, "Retention policy", true);
    written.unwrap().unwrap();
}

#[test]
fn rules_changed_again_while_the_question_is_open_are_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    let stage = dir.path().join("x");
    retention::save(&mut other_pc(dir.path()), &keep_last(5), &stage).unwrap();
    let unsent = retention::Unsent::new(keep_last(3), Retention::default());
    state.config.as_mut().unwrap().retention_unsent = Some(unsent);
    let mut harness = no_games_found(state);
    harness.get_by_label("Back up").click();
    step_until(&mut harness, "the question", asking);

    retention::save(&mut other_pc(dir.path()), &keep_last(7), &stage).unwrap();
    harness.get_by_label("Keep this PC's").click();
    harness.step();
    click_enabled(&harness, "Back up");
    step_until(&mut harness, "the backup", backed_up);
    settle(&mut harness);

    assert_eq!(
        retention::load(&mut other_pc(dir.path())).unwrap(),
        keep_last(7)
    );
    let config = harness.state().1.config.as_ref().unwrap();
    let rebased = retention::Unsent::new(keep_last(3), keep_last(5));
    assert_eq!(
        config.retention_unsent,
        Some(rebased),
        "asked again next time"
    );
    let said = "The Old backups rules were not saved to the repository: another PC changed them \
                again; the next Back up asks about them";
    assert!(harness.query_by_label(said).is_some());
}

#[test]
fn rules_over_a_newer_versions_are_not_saved_and_say_why() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    newer_rules(dir.path());
    let unsent = retention::Unsent::new(keep_last(3), Retention::default());
    state.config.as_mut().unwrap().retention_unsent = Some(unsent);
    let mut harness = no_games_found(state);
    harness.get_by_label("Back up").click();
    step_until(&mut harness, "the backup", backed_up);
    settle(&mut harness);

    let config = harness.state().1.config.as_ref().unwrap();
    assert_eq!(config.retention_unsent, Some(unsent));
    let said = "The Old backups rules were not saved to the repository: the rules there are from \
                a newer salvajocs or cannot be read";
    assert!(harness.query_by_label(said).is_some());
}

#[test]
fn game_lines_list_old_style_files_and_steam_id() {
    let mut game = custom("Game", "Same");
    let rule = salvajocs_core::manifest::FileRule::default();
    game.entry.files.insert("<home>/.game".into(), rule);
    let mut steam = salvajocs_core::manifest::Steam::default();
    steam.id = Some(42);
    game.entry.steam = Some(steam);
    let lines = unsent::game_lines(Some(&game));
    assert!(
        lines.contains(&"Old-style file: <home>/.game".to_owned()),
        "{lines:?}"
    );
    assert!(lines.contains(&"Steam id: 42".to_owned()), "{lines:?}");
}

#[test]
fn a_difference_the_lines_do_not_show_gets_a_line_of_its_own() {
    let mut here = custom("Game", "Same");
    here.wine_prefix = Some("/prefixes/game".into());
    let mut theirs = custom("Game", "Same");
    // A rule of the manifest entry, which the dialog does not list.
    theirs.entry.steam = Some(Default::default());
    let (mine, other) = conflict_lines(Some(&here), Some(&theirs));
    assert_eq!(mine, other);
    assert!(
        mine.iter()
            .any(|line| line.starts_with("Other differences"))
    );

    // The Wine prefix alone is not a difference, and listed differences need no such line.
    let (mine, _) = conflict_lines(Some(&here), Some(&without_prefix(&here)));
    assert!(
        !mine
            .iter()
            .any(|line| line.starts_with("Other differences"))
    );
    let (mine, _) = conflict_lines(Some(&here), None);
    assert!(
        !mine
            .iter()
            .any(|line| line.starts_with("Other differences"))
    );
}

#[test]
fn a_game_saved_while_the_rules_fail_is_sent_and_the_rules_stay() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    state.env.environment = Some(Environment::default());
    let storage = state.session.as_ref().unwrap().storage.clone();
    let stage = dir.path().join("stage");
    let (manifest, config, env) = no_mapping();
    let mapper = Mapper::new(&manifest, &config, &env);
    for content in ["old", "new"] {
        let games = [saved_game(dir.path(), "Game", content)];
        back_up(
            &storage,
            &games,
            &stage,
            &mapper,
            dir.path(),
            "repo",
            |_| {},
        )
        .unwrap();
    }
    let ids = snapshot_ids(&storage, "Game");
    retention::save(&mut lock_storage(&storage).unwrap(), &keep_last(1), &stage).unwrap();
    // A directory where the rules' file is staged makes their save fail.
    let staged = (state.env.cache_dir.join("stage").join("settings"))
        .join("retention")
        .join("retention.json");
    std::fs::create_dir_all(&staged).unwrap();
    let config = state.config.as_mut().unwrap();
    config.custom_games = vec![custom("Mine", "Mine")];
    config.custom_games_unsent = vec![custom_games::Unsent::new("Mine".into(), None)];
    let unsent = retention::Unsent::new(keep_last(2), keep_last(1));
    config.retention_unsent = Some(unsent);
    let mut harness = theme::harness(
        |ui, (tab, state)| with_bar(ui, tab, state, false),
        (listing(&["Game"]), state),
    );
    settle(&mut harness);

    harness.get_by_label("Back up 1 game").click();
    step_until(&mut harness, "the backup", backed_up);
    settle(&mut harness);

    assert!(stored_game(dir.path(), "Mine").is_some());
    let config = harness.state().1.config.as_ref().unwrap();
    assert!(config.custom_games_unsent.is_empty());
    assert_eq!(config.retention_unsent, Some(unsent));
    assert_eq!(snapshot_ids(&storage, "Game"), ids, "nothing was forgotten");
    let said = "The Old backups rules were not saved, so no old backup was removed";
    assert!(harness.query_by_label(said).is_some());
}

#[test]
fn rules_that_could_not_be_saved_stay_unsent() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    // A directory where the rules' file is staged makes their save fail.
    let staged = (state.env.cache_dir.join("stage").join("settings"))
        .join("retention")
        .join("retention.json");
    std::fs::create_dir_all(&staged).unwrap();
    let unsent = retention::Unsent::new(keep_last(1), Retention::default());
    state.config.as_mut().unwrap().retention_unsent = Some(unsent);
    let mut harness = no_games_found(state);
    harness.get_by_label("Back up").click();
    step_until(&mut harness, "the backup", backed_up);
    settle(&mut harness);

    let config = harness.state().1.config.as_ref().unwrap();
    assert_eq!(config.retention_unsent, Some(unsent));
    let said = harness
        .query_all_by_label_contains("The Old backups rules were not saved to the repository")
        .next();
    assert!(said.is_some());
    assert!(retention::load(&mut other_pc(dir.path())).unwrap().is_off());
}

#[test]
fn a_cleanup_after_rules_that_were_not_saved_forgets_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let storage = shared_storage(dir.path());
    let stage = dir.path().join("stage");
    let (manifest, config, env) = no_mapping();
    let mapper = Mapper::new(&manifest, &config, &env);
    for content in ["old", "new"] {
        let games = [saved_game(dir.path(), "Game", content)];
        back_up(
            &storage,
            &games,
            &stage,
            &mapper,
            dir.path(),
            "repo",
            |_| {},
        )
        .unwrap();
    }
    let ids = snapshot_ids(&storage, "Game");
    retention::save(&mut lock_storage(&storage).unwrap(), &keep_last(1), &stage).unwrap();

    let cleaned = clean_up(&storage, &[], true);

    assert!(cleaned.rules_failed);
    assert!(cleaned.error.is_none());
    assert_eq!(cleaned.done.forgotten, 0);
    assert_eq!(snapshot_ids(&storage, "Game"), ids);
    assert_eq!(clean_up(&storage, &[], false).done.forgotten, 1);
}

#[test]
fn a_repository_switched_during_the_backup_keeps_the_changes_unsent() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(&dir.path().join("first"));
    let config = state.config.as_mut().unwrap();
    config.custom_games = vec![custom("Mine", "Mine")];
    let unsent = vec![custom_games::Unsent::new("Mine".into(), None)];
    config.custom_games_unsent = unsent.clone();
    let mut harness = no_games_found(state);
    let ctx = harness.ctx.clone();

    let first = harness.state().1.session.as_ref().unwrap().storage.clone();
    let held = first.lock().unwrap();
    let (tab, state) = harness.state_mut();
    tab.start_backup(&ctx, state);
    state.session = self::state(&dir.path().join("second")).session;
    drop(held);
    step_until(&mut harness, "the backup", backed_up);
    settle(&mut harness);

    let config = harness.state().1.config.as_ref().unwrap();
    assert_eq!(config.custom_games_unsent, unsent);
    assert!(stored_game(&dir.path().join("first"), "Mine").is_some());
}

#[test]
fn an_edit_made_during_the_backup_stays_unsent() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(dir.path());
    let mine = custom("Mine", "Mine");
    let config = state.config.as_mut().unwrap();
    config.custom_games = vec![mine.clone()];
    config.custom_games_unsent = vec![custom_games::Unsent::new("Mine".into(), None)];
    let mut harness = no_games_found(state);
    let ctx = harness.ctx.clone();

    let storage = harness.state().1.session.as_ref().unwrap().storage.clone();
    let held = storage.lock().unwrap();
    let (tab, state) = harness.state_mut();
    tab.start_backup(&ctx, state);
    let edited = custom("Mine", "Edited");
    let edit = crate::state::Edit {
        based_on: Some(mine.clone()),
        game: Some(edited.clone()),
    };
    state.edit_custom_game(&ctx, edit);
    drop(held);
    step_until(&mut harness, "the backup", backed_up);
    settle(&mut harness);

    // The backup sent the game as it was when it started.
    let sent = custom_games::Stored::Game(Box::new(mine.clone()));
    assert_eq!(stored_game(dir.path(), "Mine"), Some(sent));
    let config = harness.state().1.config.as_ref().unwrap();
    assert_eq!(config.custom_games, [edited]);
    let rebased = custom_games::Unsent::new("Mine".into(), Some(mine));
    assert_eq!(config.custom_games_unsent, [rebased]);
}
