use std::collections::BTreeSet;
use std::path::Path;

use eframe::egui::accesskit::{Role, Toggled};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use super::*;
use crate::theme;

fn tree(keys: &[&str]) -> FileTree {
    FileTree::new(
        keys.iter()
            .map(|key| ((*key).to_owned(), 10, Path::new("/disk").join(key))),
    )
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
    opened: Option<PathBuf>,
}

fn harness(keys: &[&str], included: &[&str], enabled: bool) -> Harness<'static, Fixture> {
    let fixture = Fixture {
        tree: tree(keys),
        included: included.iter().map(|key| (*key).to_owned()).collect(),
        enabled,
        toggled: None,
        opened: None,
    };
    let mut harness = theme::harness(
        |ui, state: &mut Fixture| {
            let Fixture {
                tree,
                included,
                enabled,
                toggled,
                opened,
            } = state;
            match tree.ui(ui, &|key| included.contains(key), *enabled) {
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

fn toggled(harness: &Harness<'_, Fixture>, label: &str) -> Option<Toggled> {
    harness.get_by_label(label).accesskit_node().toggled()
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
        .map(|(key, real)| (key.to_owned(), 1, PathBuf::from(real))),
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
