use std::path::PathBuf;

use super::*;
use crate::crossbuild::{Family, base_family};
use crate::layout::Base;
use crate::scan::RegistrySource;

const KEY: &str = r"HKEY_CURRENT_USER\Software\Foo";

fn settings(paths: &[(&str, bool)]) -> GameSettings {
    GameSettings {
        paths: paths
            .iter()
            .map(|(node, included)| (node.to_string(), *included))
            .collect(),
        ..GameSettings::default()
    }
}

fn file(base: Base, tail: &str) -> FoundFile {
    FoundFile {
        base,
        tail: tail.into(),
        real: PathBuf::from("/real").join(tail),
        size: 1,
        modified: None,
    }
}

fn scan(files: Vec<FoundFile>) -> GameScan {
    GameScan {
        title: "Game".into(),
        files,
        roots: vec![],
        registry_keys: vec![KEY.into()],
        registry_source: Some(RegistrySource::WinePrefix("/pfx".into())),
        prefixes: Vec::new(),
        prefix_origin: None,
        unreadable: vec![],
    }
}

#[test]
fn deepest_saved_ancestor_decides() {
    let s = settings(&[("WinAppData/Foo", false), ("WinAppData/Foo/keep", true)]);
    assert!(!s.path_included("WinAppData/Foo"));
    assert!(!s.path_included("WinAppData/Foo/a.sav"));
    assert!(s.path_included("WinAppData/Foo/keep/b.sav"));
    assert!(s.path_included("WinAppData/Bar/c.sav"));
    assert!(s.path_included("WinAppData"));
}

#[test]
fn nothing_saved_is_included() {
    assert!(GameSettings::default().path_included("Home/a.sav"));
}

#[test]
fn a_name_prefix_is_not_an_ancestor() {
    let s = settings(&[("Home/a", false)]);
    assert!(s.path_included("Home/ab"));
    assert!(!s.path_included("Home/a/x"));
    assert!(s.path_included("XdgData/a/x"));
}

#[test]
fn windows_bases_ignore_case_linux_bases_do_not() {
    let s = settings(&[
        ("WinAppData/foo", false),
        ("Home/foo", false),
        ("GameDir/foo", false),
    ]);
    assert!(!s.path_included("WinAppData/FOO/x"));
    assert!(s.path_included("Home/FOO/x"));
    assert!(s.path_included("GameDir/FOO/x"));
    // The base name itself is always exact.
    assert!(s.path_included("winappdata/foo/x"));
}

#[test]
fn set_removes_entries_below_the_node() {
    let mut s = settings(&[
        ("Home/a/b", false),
        ("Home/a/c/d", true),
        ("Home/ab", false),
    ]);
    s.set_path("Home/a", false);
    assert_eq!(s, settings(&[("Home/a", false), ("Home/ab", false)]));
}

#[test]
fn set_saves_only_a_value_that_differs_from_the_inherited_one() {
    let mut s = settings(&[("Home/a", false)]);
    s.set_path("Home/a/b", false);
    assert_eq!(s, settings(&[("Home/a", false)]));
    s.set_path("Home/a/b", true);
    assert_eq!(s, settings(&[("Home/a", false), ("Home/a/b", true)]));
    s.set_path("Home/a", true);
    assert_eq!(s, GameSettings::default());
}

#[test]
fn set_replaces_a_case_variant_under_a_windows_base() {
    let mut s = settings(&[("WinAppData/Foo", false)]);
    s.set_path("WinAppData/foo", true);
    assert_eq!(s, GameSettings::default());
}

#[test]
fn registry_keys_ignore_case_and_absent_is_included() {
    let mut s = GameSettings::default();
    assert!(s.registry_included(KEY));
    s.set_registry(&KEY.to_lowercase(), false);
    assert!(!s.registry_included(KEY));
    s.set_registry(KEY, true);
    assert!(s.registry_included(KEY));
    assert!(s.registry.is_empty());
}

#[test]
fn apply_drops_excluded_files_and_keys() {
    let game = scan(vec![
        file(Base::Home, "keep.sav"),
        file(Base::Home, "drop/x.sav"),
    ]);
    let mut s = settings(&[("Home/drop", false)]);
    s.set_registry(KEY, false);
    let kept = apply(&game, &s).unwrap();
    assert_eq!(kept.files, vec![file(Base::Home, "keep.sav")]);
    assert!(kept.registry_keys.is_empty());
    assert_eq!(kept.registry_source, None);
}

#[test]
fn apply_keeps_the_registry_source_while_a_key_remains() {
    let game = scan(vec![]);
    let kept = apply(&game, &GameSettings::default()).unwrap();
    assert_eq!(kept, game);
}

#[test]
fn skipped_game_is_not_backed_up() {
    let s = GameSettings {
        skip: true,
        ..GameSettings::default()
    };
    assert_eq!(apply(&scan(vec![file(Base::Home, "a.sav")]), &s), None);
}

#[test]
fn nothing_left_makes_no_backup() {
    let mut game = scan(vec![file(Base::Home, "a.sav")]);
    let mut s = settings(&[("Home", false)]);
    s.set_registry(KEY, false);
    assert_eq!(apply(&game, &s), None);
    // Keys the manifest declares but the scan did not find are not something to back up.
    game.registry_source = None;
    assert_eq!(apply(&game, &settings(&[("Home", false)])), None);
}

#[test]
fn empty_fields_are_not_written() {
    let json: Value = serde_json::from_slice(&GameSettings::default().to_json()).unwrap();
    assert_eq!(json, serde_json::json!({ "format": 1 }));
}

#[test]
fn unknown_fields_survive_a_rewrite() {
    let read = br#"{ "format": 1, "skip": true, "future": { "a": [1] } }"#;
    let mut s = GameSettings::from_json(read).unwrap();
    s.set_path("Home/a", false);
    let json: Value = serde_json::from_slice(&s.to_json()).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "format": 1,
            "skip": true,
            "paths": { "Home/a": false },
            "future": { "a": [1] },
        })
    );
}

#[test]
fn newer_format_is_refused_whatever_it_holds() {
    let read = br#"{ "format": 2, "paths": ["changed shape"] }"#;
    assert!(matches!(
        GameSettings::from_json(read),
        Err(SelectionError::NewerFormat(2))
    ));
    assert!(matches!(
        GameSettings::from_json(b"{}"),
        Err(SelectionError::Parse(_))
    ));
}

#[test]
fn changes_apply_in_order() {
    let mut s = GameSettings::default();
    s.apply_changes(&[
        Change::Skip(true),
        Change::Path {
            node: "Home/a".into(),
            included: false,
        },
        Change::Registry {
            key: KEY.into(),
            included: false,
        },
        Change::Skip(false),
        Change::Path {
            node: "Home".into(),
            included: true,
        },
    ]);
    let mut expected = GameSettings::default();
    expected.set_registry(KEY, false);
    assert_eq!(s, expected);
}

#[test]
fn path_key_is_the_staging_path() {
    assert_eq!(
        path_key(&file(Base::WinAppData, "Foo/a.sav")),
        "WinAppData/Foo/a.sav"
    );
}

/// The base after `base`, through every variant. The match has no wildcard arm, so a new
/// variant fails to compile here until it joins the chain.
fn next_base(base: Base) -> Option<Base> {
    use Base::*;
    Some(match base {
        WinHome => WinDocuments,
        WinDocuments => WinAppData,
        WinAppData => WinLocalAppData,
        WinLocalAppData => WinLocalAppDataLow,
        WinLocalAppDataLow => WinPublic,
        WinPublic => WinProgramData,
        WinProgramData => WinDir,
        WinDir => Home,
        Home => XdgData,
        XdgData => XdgConfig,
        XdgConfig => GameDir,
        GameDir => StoreRoot,
        StoreRoot => Absolute,
        Absolute => WinAbsolute,
        WinAbsolute => return None,
    })
}

#[test]
fn every_windows_family_base_and_no_other_folds_case() {
    let bases: Vec<Base> =
        std::iter::successors(Some(Base::WinHome), |base| next_base(*base)).collect();
    assert_eq!(bases.len(), 15);
    for base in bases {
        let windows = base_family(base) == Some(Family::Windows);
        assert_eq!(folds_case(base.dir_name()), windows, "{base:?}");
    }
}

#[test]
fn only_windows_bases_fold_case() {
    assert!(folds_case("WinAppData"));
    assert!(folds_case("WinAbsolute"));
    assert!(!folds_case("Home"));
    assert!(!folds_case("GameDir"));
    assert!(!folds_case("winappdata"));
    assert!(!folds_case("NotABase"));
}

#[test]
fn a_key_stored_before_appdata_matched_without_case_still_applies() {
    // Before, `<home>/Appdata/LocalLow/..` was keyed as `WinHome/Appdata/LocalLow/..`.
    let mut s = settings(&[
        ("WinHome/Appdata/LocalLow/Co/Game/a.sav", false),
        ("WinHome/APPDATA/roaming", false),
    ]);
    assert!(!s.path_included("WinLocalAppDataLow/Co/Game/a.sav"));
    assert!(s.path_included("WinLocalAppDataLow/Co/Game/b.sav"));
    assert!(!s.path_included("WinAppData/Foo/x"));
    // Setting the new key replaces the old one.
    s.set_path("WinLocalAppDataLow/Co/Game/a.sav", true);
    assert!(s.path_included("WinLocalAppDataLow/Co/Game/a.sav"));
    assert!(s.paths.keys().all(|key| !key.contains("LocalLow")));
}

#[test]
fn a_deeper_new_key_beats_an_older_key_of_an_ancestor_stored_as_win_home() {
    let mut s = settings(&[("WinHome/Appdata/LocalLow/Co", false)]);
    s.set_path("WinLocalAppDataLow/Co/Game", true);
    assert!(s.path_included("WinLocalAppDataLow/Co/Game/a.sav"));
    assert!(!s.path_included("WinLocalAppDataLow/Co/Other/a.sav"));
}

#[test]
fn an_older_key_on_an_ancestor_of_appdata_still_applies() {
    // Before, these folders were keyed below `WinHome/AppData/..`; the tree stores a folded
    // chain by its top folder.
    for old in ["WinHome", "WinHome/Appdata", "WinHome/APPDATA/localLow"] {
        let s = settings(&[(old, false)]);
        assert!(
            !s.path_included("WinLocalAppDataLow/Co/Game/a.sav"),
            "{old}"
        );
        assert!(!s.path_included("WinLocalAppDataLow"), "{old}");
    }
    let s = settings(&[("WinHome/Appdata", false)]);
    assert!(!s.path_included("WinAppData/Foo/x"));
    assert!(!s.path_included("WinLocalAppData/Foo/x"));
    let s = settings(&[("WinHome/Appdata/Local", false)]);
    assert!(!s.path_included("WinLocalAppData/Foo/x"));
    assert!(s.path_included("WinLocalAppDataLow/Foo/x"));
    assert!(s.path_included("WinAppData/Foo/x"));
}

#[test]
fn a_new_key_below_an_older_ancestor_key_wins_by_depth() {
    let mut s = settings(&[("WinHome/Appdata", false)]);
    s.set_path("WinLocalAppDataLow", true);
    assert!(s.path_included("WinLocalAppDataLow/Co/Game/a.sav"));
    assert!(!s.path_included("WinAppData/Foo/x"));
    let s = settings(&[
        ("WinHome/Appdata", false),
        ("WinHome/Appdata/LocalLow", true),
    ]);
    assert!(s.path_included("WinLocalAppDataLow/Co/a.sav"));
    assert!(!s.path_included("WinAppData/Foo/x"));
}
