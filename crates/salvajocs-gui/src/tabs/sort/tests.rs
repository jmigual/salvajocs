use std::time::{Duration, UNIX_EPOCH};

use eframe::egui::Role;
use egui_kittest::kittest::Queryable;
use salvajocs_core::storage::SnapshotInfo;

use super::*;

/// The stats of a game whose last backup, of `size`, is from 2026-11-19 14:02 UTC, or of a game
/// with only a safety backup.
fn stats(last: bool, size: Option<u64>) -> BackupStats {
    let time = UNIX_EPOCH + Duration::from_secs(1_795_096_920);
    let mut tags = vec!["salvajocs".to_owned(), "game:game".to_owned()];
    if !last {
        tags.push("pre-restore".to_owned());
    }
    let snapshot = SnapshotInfo::new("a".into(), time, "pc".into(), "Game".into(), tags);
    BackupStats::of(&[snapshot.with_size(size)])
}

fn emphasized(line: &Line) -> Vec<&str> {
    let parts = line.parts.iter().filter(|(_, emphasized)| *emphasized);
    parts.map(|(text, _)| text.as_str()).collect()
}

#[test]
fn a_backed_up_game_shows_its_last_backup_and_space() {
    let stats = stats(true, Some(1_003_520));
    let line = backup_line(Some(&stats), Some(Ok(3_250_586)), SortKey::Name);
    assert_eq!(
        line.text(),
        "Last backup 2026-11-19 14:02 UTC, 980.0 KiB. All backups use 3.1 MiB."
    );
    assert!(emphasized(&line).is_empty());
}

#[test]
fn the_sorted_figure_is_emphasized() {
    let stats = stats(true, Some(1024));
    let line = |by| backup_line(Some(&stats), Some(Ok(2048)), by);
    assert_eq!(
        emphasized(&line(SortKey::LastBackup)),
        ["2026-11-19 14:02 UTC"]
    );
    assert_eq!(emphasized(&line(SortKey::LastBackupSize)), ["1.0 KiB"]);
    assert_eq!(emphasized(&line(SortKey::SpaceUsed)), ["2.0 KiB"]);
}

#[test]
fn missing_figures_say_so() {
    let no_size = stats(true, None);
    let computing = backup_line(Some(&no_size), None, SortKey::Name);
    assert_eq!(
        computing.text(),
        "Last backup 2026-11-19 14:02 UTC, size unknown. All backups use …"
    );

    let failed = backup_line(Some(&no_size), Some(Err("broken".into())), SortKey::Name);
    assert!(
        failed
            .text()
            .ends_with("All backups use an unknown amount.")
    );
    assert!(failed.hover.contains("broken"));
}

#[test]
fn a_game_with_only_safety_backups_or_none_has_no_backup() {
    let safety = stats(false, Some(1024));
    let line = backup_line(Some(&safety), Some(Ok(1024)), SortKey::Name);
    assert_eq!(line.text(), "No backup. Safety backups use 1.0 KiB.");
    assert_eq!(backup_line(None, None, SortKey::Name).text(), "No backup.");
}

#[test]
fn the_restore_line_puts_the_sorted_figure_first() {
    let stats = stats(true, Some(1_003_520));
    let line = |by| restore_line(&stats, Some(Ok(3_250_586)), by);
    let name = line(SortKey::Name);
    assert_eq!(name.text(), "2026-11-19, 980.0 KiB, 3.1 MiB used");
    assert!(emphasized(&name).is_empty());
    let size = line(SortKey::LastBackupSize);
    assert_eq!(size.text(), "980.0 KiB, 2026-11-19, 3.1 MiB used");
    assert_eq!(emphasized(&size), ["980.0 KiB"]);
    let space = line(SortKey::SpaceUsed);
    assert_eq!(space.text(), "3.1 MiB used, 2026-11-19, 980.0 KiB");
    assert_eq!(emphasized(&space), ["3.1 MiB used"]);
    assert!(
        name.hover
            .starts_with("Last backup 2026-11-19 14:02 UTC, 980.0 KiB.")
    );
}

#[test]
fn the_restore_line_marks_missing_figures() {
    let no_size = stats(true, None);
    let computing = restore_line(&no_size, None, SortKey::Name);
    assert_eq!(computing.text(), "2026-11-19, … used");
    assert!(computing.hover.contains("size unknown"));

    let failed = restore_line(&no_size, Some(Err("broken".into())), SortKey::Name);
    assert_eq!(failed.text(), "2026-11-19, ? used");
    assert!(failed.hover.contains("broken"));

    let safety = restore_line(&stats(false, None), Some(Ok(1024)), SortKey::Name);
    assert_eq!(safety.text(), "Only safety backups, 1.0 KiB used");
}

#[test]
fn the_direction_is_named_in_words() {
    let words = |by, descending| order_words(ListSort::new(by, descending));
    assert_eq!(words(SortKey::Name, false), "A to Z");
    assert_eq!(words(SortKey::LastBackup, true), "Newest first");
    assert_eq!(words(SortKey::SpaceUsed, true), "Largest first");
    assert_eq!(words(SortKey::LastBackupSize, false), "Smallest first");
    assert_eq!(words(SortKey::Changes, true), "Most changes first");
    assert_eq!(words(SortKey::Changes, false), "Fewest changes first");
}

/// Whether the dropdown of `keys`, once opened, lists "Changes".
fn lists_changes(keys: &[SortKey]) -> bool {
    let mut harness = crate::theme::harness(
        |ui, _: &mut ()| {
            sort_ui(ui, "sort", keys, ListSort::default());
        },
        (),
    );
    harness.get_by_role(Role::ComboBox).click();
    harness.run();
    harness.query_by_label("Changes").is_some()
}

#[test]
fn changes_is_listed_on_the_backup_tab_and_not_on_the_restore_sidebar() {
    assert!(lists_changes(SortKey::ALL));
    assert!(!lists_changes(RESTORE_KEYS));
}
