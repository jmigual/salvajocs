use std::collections::BTreeSet;
use std::path::Path;

use eframe::egui::accesskit::{Role, Toggled};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use super::*;
use crate::theme;

fn row(key: &str) -> Row {
    Row {
        key: key.to_owned(),
        size: 10,
        real: Some(Path::new("/disk").join(key)),
        removed: false,
    }
}

/// A row only in the backup.
fn removed(key: &str) -> Row {
    Row {
        real: None,
        removed: true,
        ..row(key)
    }
}

fn tree(keys: &[&str]) -> FileTree {
    FileTree::new(keys.iter().map(|key| row(key)))
}

/// The folder at `path` below the root, one component at a time.
fn folder<'a>(tree: &'a FileTree, path: &[&str]) -> &'a Folder {
    path.iter().fold(&tree.root, |parent, name| {
        parent
            .folders
            .iter()
            .find(|folder| folder.name == *name)
            .unwrap_or_else(|| panic!("no folder {name}"))
    })
}

/// The keys in the folder's range.
fn keys<'a>(tree: &'a FileTree, folder: &Folder) -> Vec<&'a str> {
    tree.files[folder.range.clone()]
        .iter()
        .map(|file| file.key.as_str())
        .collect()
}

fn names(folders: &[Folder]) -> Vec<&str> {
    folders.iter().map(|folder| folder.name.as_str()).collect()
}

/// How many files below `folder` `included` accepts, by the cached totals.
fn included_in(tree: &mut FileTree, folder: &[&str], included: &dyn Fn(&str) -> bool) -> usize {
    let (start, end) = {
        let range = &self::folder(tree, folder).range;
        (range.start, range.end)
    };
    let totals = totals(&mut tree.included, &tree.files, included, &|_, _| None);
    totals[end].files - totals[start].files
}

#[test]
fn files_group_into_bases_and_nested_folders() {
    let tree = tree(&[
        "WinAppData/Foo/sub/c.sav",
        "WinAppData/Foo/b.sav",
        "Home/top.sav",
        "WinAppData/Foo/a.sav",
    ]);

    assert_eq!(names(&tree.root.folders), ["Home", "WinAppData"]);
    let foo = folder(&tree, &["WinAppData", "Foo"]);
    assert_eq!(foo.key, "WinAppData/Foo");
    assert_eq!(
        keys(&tree, foo),
        [
            "WinAppData/Foo/a.sav",
            "WinAppData/Foo/b.sav",
            "WinAppData/Foo/sub/c.sav"
        ]
    );
    assert_eq!(foo.files.len(), 2);
    let sub = folder(&tree, &["WinAppData", "Foo", "sub"]);
    assert_eq!(sub.key, "WinAppData/Foo/sub");
    assert_eq!(keys(&tree, sub), ["WinAppData/Foo/sub/c.sav"]);
    let home = folder(&tree, &["Home"]);
    assert_eq!(keys(&tree, home), ["Home/top.sav"]);
    assert_eq!(home.files.len(), 1);
    assert_eq!(tree.files[home.files[0]].name, "top.sav");
}

#[test]
fn sibling_folders_list_by_component_order() {
    let tree = tree(&[
        "Base/ab",
        "Base/a/x",
        "Base/a-b/y",
        "Base/a.txt",
        "Base/a/z",
    ]);

    let base = folder(&tree, &["Base"]);
    assert_eq!(names(&base.folders), ["a", "a-b"]);
    assert_eq!(base.files.len(), 2);
    assert_eq!(
        keys(&tree, folder(&tree, &["Base", "a"])),
        ["Base/a/x", "Base/a/z"]
    );
    assert_eq!(keys(&tree, folder(&tree, &["Base", "a-b"])), ["Base/a-b/y"]);
}

#[test]
fn folder_counts_cover_their_range() {
    let mut tree = tree(&["Base/d/a", "Base/d/b", "Base/e/c"]);

    assert_eq!(included_in(&mut tree, &["Base", "d"], &|_| true), 2);
    tree.invalidate();
    assert_eq!(included_in(&mut tree, &["Base", "d"], &|_| false), 0);
    tree.invalidate();
    assert_eq!(
        included_in(&mut tree, &["Base", "d"], &|key| key == "Base/d/b"),
        1
    );
    tree.invalidate();
    assert_eq!(
        included_in(&mut tree, &["Base"], &|key| key != "Base/e/c"),
        2
    );
}

#[test]
fn counts_are_kept_until_invalidated() {
    let mut tree = tree(&["Base/a"]);

    assert_eq!(included_in(&mut tree, &["Base"], &|_| true), 1);
    assert_eq!(included_in(&mut tree, &["Base"], &|_| false), 1);
    tree.invalidate();
    assert_eq!(included_in(&mut tree, &["Base"], &|_| false), 0);
}

struct Fixture {
    tree: FileTree,
    included: BTreeSet<String>,
    /// Draw with checkboxes, else read-only.
    select: bool,
    enabled: bool,
    badges: Vec<(String, Badge)>,
    toggled: Option<(String, bool)>,
    opened: Option<PathBuf>,
}

fn harness(keys: &[&str], included: &[&str], enabled: bool) -> Harness<'static, Fixture> {
    harness_of(tree(keys), included, enabled, &[], true)
}

/// A harness drawing `tree` with `badges` by key, with checkboxes when `select`.
fn harness_of(
    tree: FileTree,
    included: &[&str],
    enabled: bool,
    badges: &[(&str, Badge)],
    select: bool,
) -> Harness<'static, Fixture> {
    let fixture = Fixture {
        tree,
        included: included.iter().map(|key| (*key).to_owned()).collect(),
        select,
        enabled,
        badges: (badges.iter())
            .map(|(key, badge)| ((*key).to_owned(), *badge))
            .collect(),
        toggled: None,
        opened: None,
    };
    let mut harness = theme::harness(
        |ui, state: &mut Fixture| {
            let Fixture {
                tree,
                included,
                select,
                enabled,
                badges,
                toggled,
                opened,
            } = state;
            let included = |key: &str| included.contains(key);
            let mode = if *select {
                Mode::Select {
                    included: &included,
                    enabled: *enabled,
                }
            } else {
                Mode::ReadOnly
            };
            let badge = |key: &str| {
                (badges.iter())
                    .find(|(badged, _)| badged == key)
                    .map(|(_, badge)| *badge)
            };
            // The key and the badge, so tests see both reach the tooltip.
            let hint = |key: &str, badge: Badge| format!("{key}: {badge:?}");
            match tree.ui(ui, mode, &badge, &hint) {
                Some(Clicked::Toggle(key, on)) => *toggled = Some((key, on)),
                Some(Clicked::Open(path)) => *opened = Some(path),
                None => {}
            }
        },
        fixture,
    );
    harness.run();
    harness
}

/// Opens every folder of `harness`, one at a time.
fn open_all(harness: &mut Harness<'static, Fixture>) {
    loop {
        let closed = harness
            .query_all_by_role(Role::DisclosureTriangle)
            .filter_map(|arrow| arrow.accesskit_node().label())
            .find(|label| label.starts_with("Open "));
        let Some(label) = closed else { return };
        harness.get_by_label(&label).click();
        harness.run();
    }
}

fn toggled(harness: &Harness<'_, Fixture>, label: &str) -> Option<Toggled> {
    harness.get_by_label(label).accesskit_node().toggled()
}

fn role(harness: &Harness<'_, Fixture>, label: &str) -> Role {
    harness.get_by_label(label).accesskit_node().role()
}

const KEYS: [&str; 2] = ["WinAppData/Foo/a.sav", "WinAppData/Foo/b.sav"];

#[test]
fn clicking_a_checked_folder_unchecks_it() {
    let mut harness = harness(&KEYS, &KEYS, true);
    assert_eq!(toggled(&harness, "WinAppData/Foo"), Some(Toggled::True));

    harness.get_by_label("WinAppData/Foo").click();
    harness.run();

    assert_eq!(
        harness.state().toggled,
        Some(("WinAppData".to_owned(), false))
    );
}

#[test]
fn clicking_an_unchecked_folder_checks_it() {
    let mut harness = harness(&KEYS, &[], true);
    assert_eq!(toggled(&harness, "WinAppData/Foo"), Some(Toggled::False));

    harness.get_by_label("WinAppData/Foo").click();
    harness.run();

    assert_eq!(
        harness.state().toggled,
        Some(("WinAppData".to_owned(), true))
    );
}

#[test]
fn clicking_a_mixed_folder_checks_everything() {
    let mut harness = harness(&KEYS, &KEYS[..1], true);
    assert_eq!(toggled(&harness, "WinAppData/Foo"), Some(Toggled::Mixed));

    harness.get_by_label("WinAppData/Foo").click();
    harness.run();

    assert_eq!(
        harness.state().toggled,
        Some(("WinAppData".to_owned(), true))
    );
}

#[test]
fn a_file_toggles_by_its_key() {
    let mut harness = harness(&["top.sav"], &[], true);

    harness.get_by_label("top.sav").click();
    harness.run();

    assert_eq!(harness.state().toggled, Some(("top.sav".to_owned(), true)));
}

#[test]
fn a_disabled_tree_toggles_nothing() {
    let mut harness = harness(&KEYS, &KEYS, false);
    assert!(
        harness
            .get_by_label("WinAppData/Foo")
            .accesskit_node()
            .is_disabled()
    );

    harness.get_by_label("WinAppData/Foo").click();
    harness.run();

    assert_eq!(harness.state().toggled, None);
}

#[test]
fn only_open_folders_show_their_children() {
    let mut harness = harness(&KEYS, &KEYS, true);
    assert_eq!(harness.query_all_by_label("a.sav").count(), 0);

    // Clicking the label toggles the checkbox, so open the folder by its arrow.
    assert_eq!(harness.query_all_by_label("Open WinAppData/Foo").count(), 1);
    harness.get_by_role(Role::DisclosureTriangle).click();
    harness.run();

    assert_eq!(harness.query_all_by_label("a.sav").count(), 1);
}

#[test]
fn a_folder_shows_how_much_of_it_is_included() {
    let harness = harness(&KEYS, &KEYS[..1], true);

    assert_eq!(harness.query_all_by_label("1 of 2 files, 10 B").count(), 1);
}

#[test]
fn empty_and_non_ascii_components_are_folders_of_their_own() {
    let tree = tree(&["Base//é/y", "Base/é/z"]);

    let base = folder(&tree, &["Base"]);
    assert_eq!(names(&base.folders), ["", "é"]);
    let nested = folder(&tree, &["Base", "", "é"]);
    assert_eq!(nested.key, "Base//é");
    assert_eq!(keys(&tree, nested), ["Base//é/y"]);
    let direct = folder(&tree, &["Base", "é"]);
    assert_eq!(direct.key, "Base/é");
    assert_eq!(keys(&tree, direct), ["Base/é/z"]);
}

#[test]
fn case_variant_folders_merge_below_windows_bases_only() {
    assert!(selection::folds_case("WinAppData"));
    assert!(!selection::folds_case("Home"));
    let tree = tree(&[
        "WinAppData/Foo/a.sav",
        "WinAppData/Goo/c.sav",
        "WinAppData/foo/b.sav",
        "Home/Foo/a.sav",
        "Home/foo/b.sav",
    ]);

    assert_eq!(
        names(&folder(&tree, &["WinAppData"]).folders),
        ["Foo", "Goo"]
    );
    let foo = folder(&tree, &["WinAppData", "Foo"]);
    assert_eq!(foo.key, "WinAppData/Foo");
    assert_eq!(
        keys(&tree, foo),
        ["WinAppData/Foo/a.sav", "WinAppData/foo/b.sav"]
    );
    assert_eq!(names(&folder(&tree, &["Home"]).folders), ["Foo", "foo"]);
}

#[test]
fn folders_know_their_folder_on_disk() {
    let tree = FileTree::new(
        [
            ("WinAppData/foo/sub/c.sav", "/pfx/Roaming/foo/sub/c.sav"),
            ("WinAppData/Foo/b.sav", "/pfx/Roaming/Foo/b.sav"),
            ("WinAbsolute/C/Games/d.sav", "C:/Games/d.sav"),
        ]
        .map(|(key, real)| Row {
            key: key.to_owned(),
            size: 1,
            real: Some(PathBuf::from(real)),
            removed: false,
        }),
    );

    let real = |path: &[&str]| folder(&tree, path).real.clone();
    assert_eq!(real(&["WinAppData"]), Some(PathBuf::from("/pfx/Roaming")));
    // The case variants merge into the first one in order, `Foo`, which is where it opens.
    assert_eq!(
        real(&["WinAppData", "Foo"]),
        Some(PathBuf::from("/pfx/Roaming/Foo"))
    );
    assert_eq!(
        real(&["WinAppData", "Foo", "sub"]),
        Some(PathBuf::from("/pfx/Roaming/foo/sub"))
    );
    assert_eq!(tree.root.real, None);
    assert_eq!(
        real(&["WinAbsolute", "C", "Games"]),
        Some(PathBuf::from("C:/Games"))
    );
    #[cfg(windows)]
    assert_eq!(real(&["WinAbsolute"]), None);
}

#[test]
fn a_folder_opens_even_in_a_disabled_tree() {
    let mut harness = harness(&KEYS, &KEYS, false);

    harness
        .get_by_label("Show WinAppData/Foo in the file manager")
        .click();
    harness.run();

    assert_eq!(
        harness.state().opened,
        Some(PathBuf::from("/disk/WinAppData/Foo"))
    );
    assert_eq!(harness.state().toggled, None);
}

#[test]
fn a_chain_of_single_folders_is_one_row_toggling_its_top_folder() {
    let keys = ["a/b/c/x", "a/b/c/y"];
    let mut harness = harness(&keys, &keys[..1], true);

    assert_eq!(harness.query_all_by_label("a").count(), 0);
    assert_eq!(harness.query_all_by_label("a/b").count(), 0);
    assert_eq!(toggled(&harness, "a/b/c"), Some(Toggled::Mixed));
    assert_eq!(harness.query_all_by_label("1 of 2 files, 10 B").count(), 1);
    harness.get_by_label("a/b/c").click();
    harness.run();
    assert_eq!(harness.state().toggled, Some(("a".to_owned(), true)));

    harness
        .get_by_label("Show a/b/c in the file manager")
        .click();
    harness.run();
    assert_eq!(harness.state().opened, Some(PathBuf::from("/disk/a/b/c")));
}

#[test]
fn a_chain_stops_at_a_folder_with_a_file_or_several_folders() {
    let mut harness = harness(&["a/b/c/x", "a/b/f", "a/g/y"], &[], true);
    assert_eq!(harness.query_all_by_label("a").count(), 1);

    harness.get_by_role(Role::DisclosureTriangle).click();
    harness.run();

    assert_eq!(harness.query_all_by_label("b").count(), 1);
    assert_eq!(harness.query_all_by_label("g").count(), 1);
}

#[test]
fn a_chain_ending_in_a_file_is_merged_up_to_the_last_folder() {
    let mut harness = harness(&["a/b/x"], &[], true);
    assert_eq!(harness.query_all_by_label("a/b").count(), 1);
    assert_eq!(harness.query_all_by_label("x").count(), 0);

    harness.get_by_role(Role::DisclosureTriangle).click();
    harness.run();

    assert_eq!(harness.query_all_by_label("x").count(), 1);
}

#[test]
fn a_removed_row_has_no_checkbox_and_says_it_is_removed() {
    let tree = FileTree::new([row("Base/kept.sav"), removed("Base/gone.sav")]);
    let mut harness = harness_of(tree, &["Base/kept.sav"], true, &[], true);
    open_all(&mut harness);

    let gone = harness.get_by_label("gone.sav, removed");
    assert_ne!(gone.accesskit_node().role(), Role::CheckBox);
    assert_eq!(harness.query_all_by_label("gone.sav").count(), 0);
    assert_eq!(
        harness.get_by_label("kept.sav").accesskit_node().role(),
        Role::CheckBox
    );
    assert_eq!(role(&harness, "Base/gone.sav: Removed"), Role::Image);
    // `N of M` counts present files only; the removed one is in the badge counts.
    assert_eq!(harness.query_all_by_label("1 of 1 file, 10 B").count(), 1);
    assert_eq!(role(&harness, "1 removed"), Role::Image);
    assert_eq!(toggled(&harness, "Base"), Some(Toggled::True));
}

#[test]
fn a_folder_of_removed_files_has_no_checkbox_and_no_open_button() {
    let tree = FileTree::new([row("Base/kept.sav"), removed("Base/old/gone.sav")]);
    let mut harness = harness_of(tree, &[], true, &[], true);
    open_all(&mut harness);

    let old = harness.get_by_label("old");
    assert_ne!(old.accesskit_node().role(), Role::CheckBox);
    // On `old` and on `Base` above it.
    assert_eq!(harness.query_all_by_label("1 removed").count(), 2);
    assert_eq!(
        harness
            .query_all_by_label("Show old in the file manager")
            .count(),
        0
    );
    assert_eq!(
        harness
            .query_all_by_label("Show Base in the file manager")
            .count(),
        1
    );
}

#[test]
fn a_folder_opens_where_its_first_present_file_is() {
    // `a.sav` sorts first but is only in the backup.
    let tree = FileTree::new([removed("Base/dir/a.sav"), row("Base/dir/b.sav")]);
    assert_eq!(
        folder(&tree, &["Base", "dir"]).real,
        Some(PathBuf::from("/disk/Base/dir"))
    );
    let only_removed = FileTree::new([removed("Base/dir/a.sav")]);
    assert_eq!(folder(&only_removed, &["Base", "dir"]).real, None);
}

#[test]
fn a_chain_through_a_folder_of_removed_files_still_merges() {
    let tree = FileTree::new([removed("a/b/c/x")]);
    let harness = harness_of(tree, &[], true, &[], true);
    assert_eq!(harness.query_all_by_label("a/b/c").count(), 1);
}

#[test]
fn badges_join_the_row_name_and_count_in_folder_headers() {
    let keys = [
        "Base/new.sav",
        "Base/changed.sav",
        "Base/same.sav",
        "Base/locked.sav",
    ];
    let tree = FileTree::new(keys.map(row).into_iter().chain([removed("Base/gone.sav")]));
    let badges = [
        ("Base/new.sav", Badge::New),
        ("Base/changed.sav", Badge::Changed),
        ("Base/locked.sav", Badge::Unknown),
    ];
    let mut harness = harness_of(tree, &keys, true, &badges, true);
    assert_eq!(harness.query_all_by_label("4 of 4 files, 40 B").count(), 1);
    let chips = "1 new, 1 changed, 1 removed, 1 could not be compared";
    assert_eq!(role(&harness, chips), Role::Image);
    open_all(&mut harness);
    for label in [
        "new.sav, new",
        "changed.sav, changed",
        "same.sav",
        "gone.sav, removed",
        "locked.sav, could not be compared",
    ] {
        assert_eq!(harness.query_all_by_label(label).count(), 1, "{label}");
    }
    for icon in [
        "Base/new.sav: New",
        "Base/changed.sav: Changed",
        "Base/gone.sav: Removed",
        "Base/locked.sav: Unknown",
    ] {
        assert_eq!(role(&harness, icon), Role::Image, "{icon}");
    }
    assert_eq!(
        harness.query_all_by_label_contains("Base/same.sav").count(),
        0
    );
}

#[test]
fn a_read_only_tree_has_no_checkbox_and_toggles_nothing() {
    let keys = ["Base/dir/a.sav", "Base/dir/b.sav"];
    let badges = [
        ("Base/dir/a.sav", Badge::New),
        ("Base/dir/b.sav", Badge::WillFail),
    ];
    let mut harness = harness_of(tree(&keys), &[], true, &badges, false);
    assert_eq!(harness.query_all_by_label("2 files, 20 B").count(), 1);
    assert_eq!(role(&harness, "1 new, 1 will fail"), Role::Image);
    open_all(&mut harness);
    assert_eq!(harness.query_all_by_role(Role::CheckBox).count(), 0);
    assert_eq!(harness.query_all_by_label("b.sav, will fail").count(), 1);
    assert_eq!(role(&harness, "Base/dir/b.sav: WillFail"), Role::Image);

    harness.get_by_label("a.sav, new").click();
    harness.get_by_label("Base/dir").click();
    harness.run();
    assert_eq!(harness.state().toggled, None);
}

#[test]
fn only_existing_folders_keep_their_open_button() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("present/a.sav");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    let rows = [
        Row {
            real: Some(real),
            ..row("Base/present/a.sav")
        },
        Row {
            real: Some(tmp.path().join("missing/b.sav")),
            ..row("Base/missing/b.sav")
        },
    ];
    let tree = FileTree::new(rows).existing_folders_only();
    assert!(folder(&tree, &["Base", "present"]).real.is_some());
    assert_eq!(folder(&tree, &["Base", "missing"]).real, None);
}

#[test]
fn the_row_slot_draws_on_and_under_each_open_file_row_with_its_row_position() {
    // Given out of order, so a row's position differs from where the tree lists it.
    let mut harness = theme::harness(
        |ui, tree: &mut FileTree| {
            let mut row = |ui: &mut Ui, at: usize, key: &str, slot: Slot<'_>| {
                let place = match slot {
                    Slot::End(_) => "End",
                    Slot::Below => "Below",
                };
                ui.label(format!("{place} {at} {key}"));
            };
            let (badge, hint) = (
                |_: usize, _: &str| None,
                |_: usize, _: &str, _| String::new(),
            );
            tree.ui_with_rows(ui, Mode::ReadOnly, &badge, &hint, &mut row);
        },
        tree(&["Base/b.sav", "Base/a.sav"]),
    );
    harness.run();
    assert_eq!(harness.query_all_by_label_contains("End ").count(), 0);

    harness.get_by_label("Open Base").click();
    harness.run();
    let rect = |label: &str| harness.get_by_label(label).rect();
    let (a, end, below, b) = (
        rect("a.sav"),
        rect("End 1 Base/a.sav"),
        rect("Below 1 Base/a.sav"),
        rect("b.sav"),
    );
    assert!(end.min.x > a.max.x && end.min.y < a.max.y && end.max.y > a.min.y);
    assert!(a.max.y <= below.min.y && below.max.y <= b.min.y);
    assert!(harness.query_by_label("End 0 Base/b.sav").is_some());
}

#[test]
fn rows_that_share_a_key_get_their_badges_by_position() {
    // Two rows land on one key, as a restore of both builds' roots into one build does.
    let mut harness = theme::harness(
        |ui, tree: &mut FileTree| {
            let badge = |at: usize, _: &str| Some([Badge::New, Badge::Changed][at]);
            let hint = |at: usize, key: &str, _| format!("{key} at {at}");
            tree.ui_with_rows(ui, Mode::ReadOnly, &badge, &hint, &mut |_, _, _, _| {});
        },
        tree(&["Base/a.sav", "Base/a.sav"]),
    );
    harness.run();
    let chips = harness
        .get_by_label("1 new, 1 changed")
        .accesskit_node()
        .role();
    assert_eq!(chips, Role::Image);

    harness.get_by_label("Open Base").click();
    harness.run();
    for row in [
        "a.sav, new",
        "a.sav, changed",
        "Base/a.sav at 0",
        "Base/a.sav at 1",
    ] {
        assert_eq!(harness.query_all_by_label(row).count(), 1, "{row}");
    }
}
