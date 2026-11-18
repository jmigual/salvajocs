use std::collections::BTreeSet;

use eframe::egui::accesskit::{Role, Toggled};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use super::*;
use crate::theme;

fn tree(keys: &[&str]) -> FileTree {
    FileTree::new(keys.iter().map(|key| ((*key).to_owned(), 10)))
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
    let totals = totals(&mut tree.included, &tree.files, included);
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
    enabled: bool,
    toggled: Option<(String, bool)>,
}

fn harness(keys: &[&str], included: &[&str], enabled: bool) -> Harness<'static, Fixture> {
    let fixture = Fixture {
        tree: tree(keys),
        included: included.iter().map(|key| (*key).to_owned()).collect(),
        enabled,
        toggled: None,
    };
    let mut harness = theme::harness(
        |ui, state: &mut Fixture| {
            let Fixture {
                tree,
                included,
                enabled,
                toggled,
            } = state;
            if let Some(clicked) = tree.ui(ui, &|key| included.contains(key), *enabled) {
                *toggled = Some(clicked);
            }
        },
        fixture,
    );
    harness.run();
    harness
}

fn toggled(harness: &Harness<'_, Fixture>, label: &str) -> Option<Toggled> {
    harness.get_by_label(label).accesskit_node().toggled()
}

const KEYS: [&str; 2] = ["WinAppData/Foo/a.sav", "WinAppData/Foo/b.sav"];

#[test]
fn clicking_a_checked_folder_unchecks_it() {
    let mut harness = harness(&KEYS, &KEYS, true);
    assert_eq!(toggled(&harness, "WinAppData"), Some(Toggled::True));

    harness.get_by_label("WinAppData").click();
    harness.run();

    assert_eq!(
        harness.state().toggled,
        Some(("WinAppData".to_owned(), false))
    );
}

#[test]
fn clicking_an_unchecked_folder_checks_it() {
    let mut harness = harness(&KEYS, &[], true);
    assert_eq!(toggled(&harness, "WinAppData"), Some(Toggled::False));

    harness.get_by_label("WinAppData").click();
    harness.run();

    assert_eq!(
        harness.state().toggled,
        Some(("WinAppData".to_owned(), true))
    );
}

#[test]
fn clicking_a_mixed_folder_checks_everything() {
    let mut harness = harness(&KEYS, &KEYS[..1], true);
    assert_eq!(toggled(&harness, "WinAppData"), Some(Toggled::Mixed));

    harness.get_by_label("WinAppData").click();
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
            .get_by_label("WinAppData")
            .accesskit_node()
            .is_disabled()
    );

    harness.get_by_label("WinAppData").click();
    harness.run();

    assert_eq!(harness.state().toggled, None);
}

#[test]
fn only_open_folders_show_their_children() {
    let mut harness = harness(&KEYS, &KEYS, true);
    assert_eq!(harness.query_all_by_label("Foo").count(), 0);

    // Clicking the label toggles the checkbox, so open the folder by its arrow.
    assert_eq!(harness.query_all_by_label("Open WinAppData").count(), 1);
    harness.get_by_role(Role::DisclosureTriangle).click();
    harness.run();

    assert_eq!(harness.query_all_by_label("Foo").count(), 1);
    assert_eq!(harness.query_all_by_label("a.sav").count(), 0);
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
