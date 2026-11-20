use std::fs;

use super::*;
use crate::config::{AppConfig, ListSort, PasswordMode, ThemeChoice};
use crate::layout::{Root, RootKind};
use crate::manifest::{CustomFile, RegistryRule, When};

fn root(template: &str, family: Family) -> SnapshotRoot {
    classify(template, family).unwrap()
}

fn linux_root(template: &str) -> SnapshotRoot {
    root(template, Family::Linux)
}

fn win_root(template: &str) -> SnapshotRoot {
    root(template, Family::Windows)
}

fn meta(roots: Vec<SnapshotRoot>) -> SnapshotMeta {
    SnapshotMeta {
        format: layout::SNAPSHOT_FORMAT,
        title: "Game".into(),
        roots,
        prefix: None,
    }
}

fn save_rule(os: Option<Os>) -> FileRule {
    FileRule {
        tags: vec![Tag::Save],
        when: os
            .into_iter()
            .map(|os| When {
                os: Some(os),
                store: None,
            })
            .collect(),
    }
}

fn entry(files: &[(&str, Option<Os>)]) -> GameEntry {
    GameEntry {
        files: files
            .iter()
            .map(|&(template, os)| (template.to_owned(), save_rule(os)))
            .collect(),
        ..GameEntry::default()
    }
}

fn pair(linux: &str, windows: &str) -> Pair {
    Pair {
        linux: linux.into(),
        windows: windows.into(),
    }
}

fn config() -> AppConfig {
    AppConfig {
        repository: "/repo".into(),
        repository_options: Vec::new(),
        password: PasswordMode::None,
        roots: Vec::new(),
        custom_games: Vec::new(),
        check_updates: true,
        ignored_prefixes: Vec::new(),
        theme: ThemeChoice::System,
        backup_sort: ListSort::default(),
        restore_sort: ListSort::default(),
        custom_games_repository: None,
    }
}

fn one(entries: Vec<MappingEntry>) -> MappingEntry {
    assert_eq!(entries.len(), 1);
    entries.into_iter().next().unwrap()
}

#[test]
fn auto_pairs_single_linux_and_windows_save_dir() {
    let entry = entry(&[
        ("<home>/.config/Co/Game", Some(Os::Linux)),
        ("<winAppData>/Co/Game", Some(Os::Windows)),
    ]);
    let to_windows = one(propose(
        &meta(vec![linux_root("<home>/.config/Co/Game/slot1")]),
        Some(&entry),
        &[],
        &[],
        Family::Windows,
    ));
    assert_eq!(to_windows.confidence, Confidence::AutoPair);
    assert_eq!(
        to_windows.proposed.as_deref(),
        Some("<winAppData>/Co/Game/slot1")
    );
    let to_linux = one(propose(
        &meta(vec![win_root("<winAppData>/Co/Game")]),
        Some(&entry),
        &[],
        &[],
        Family::Linux,
    ));
    assert_eq!(to_linux.confidence, Confidence::AutoPair);
    assert_eq!(to_linux.proposed.as_deref(), Some("<home>/.config/Co/Game"));
}

#[test]
fn no_auto_pair_when_ambiguous() {
    let entry = entry(&[
        ("<home>/.config/Co/Game", Some(Os::Linux)),
        ("<winAppData>/Co/Game", Some(Os::Windows)),
        ("<winDocuments>/Game", Some(Os::Windows)),
    ]);
    let mapped = one(propose(
        &meta(vec![linux_root("<home>/.config/Co/Game")]),
        Some(&entry),
        &[],
        &[],
        Family::Windows,
    ));
    assert_eq!(mapped.confidence, Confidence::Unknown);
    assert_eq!(mapped.proposed, None);
}

#[test]
fn custom_pair_maps_tail() {
    let pairs = [pair("<xdgData>/Foo", "<winDocuments>/My Games/Foo")];
    let mapped = one(propose(
        &meta(vec![linux_root("<xdgData>/Foo/saves")]),
        None,
        &pairs,
        &[],
        Family::Windows,
    ));
    assert_eq!(mapped.confidence, Confidence::Saved);
    assert_eq!(
        mapped.proposed.as_deref(),
        Some("<winDocuments>/My Games/Foo/saves")
    );
    // The Windows side matches case-insensitively and keeps the root's spelling.
    let back = one(propose(
        &meta(vec![win_root("<winDocuments>/my games/foo/Saves")]),
        None,
        &pairs,
        &[],
        Family::Linux,
    ));
    assert_eq!(back.proposed.as_deref(), Some("<xdgData>/Foo/Saves"));
    // A sibling that only shares a name prefix does not match.
    let other = one(propose(
        &meta(vec![linux_root("<xdgData>/Foobar")]),
        None,
        &pairs,
        &[],
        Family::Windows,
    ));
    assert_eq!(other.confidence, Confidence::Unknown);
}

#[test]
fn unity_rule_pairs_persistent_data_path() {
    let to_windows = one(propose(
        &meta(vec![linux_root("<xdgConfig>/unity3d/Co/Game/profile")]),
        None,
        &[],
        &[],
        Family::Windows,
    ));
    assert_eq!(to_windows.confidence, Confidence::EngineRule);
    assert_eq!(
        to_windows.proposed.as_deref(),
        Some("<winLocalAppDataLow>/Co/Game/profile")
    );
    let to_linux = one(propose(
        &meta(vec![win_root("<home>/AppData/LocalLow/Co/Game")]),
        None,
        &[],
        &[],
        Family::Linux,
    ));
    assert_eq!(to_linux.confidence, Confidence::EngineRule);
    assert_eq!(
        to_linux.proposed.as_deref(),
        Some("<xdgConfig>/unity3d/Co/Game")
    );
    // Company alone is not a persistent data path.
    let short = one(propose(
        &meta(vec![linux_root("<xdgConfig>/unity3d/Co")]),
        None,
        &[],
        &[],
        Family::Windows,
    ));
    assert_eq!(short.confidence, Confidence::Unknown);
}

#[test]
fn unity_playerprefs_registry_adds_note() {
    let roots = meta(vec![linux_root("<xdgConfig>/unity3d/Co/Game")]);
    let mut entry = entry(&[("<xdgConfig>/unity3d/Co/Game", Some(Os::Linux))]);
    let without = one(propose(&roots, Some(&entry), &[], &[], Family::Windows));
    assert!(without.notes.is_empty());
    entry.registry.insert(
        "HKEY_CURRENT_USER\\Software\\co\\GAME".into(),
        RegistryRule::default(),
    );
    let with = one(propose(&roots, Some(&entry), &[], &[], Family::Windows));
    assert_eq!(with.confidence, Confidence::EngineRule);
    assert_eq!(with.notes, [PLAYER_PREFS_NOTE]);
    // A key of another game does not trigger the note.
    entry.registry.clear();
    entry.registry.insert(
        "HKEY_CURRENT_USER/Software/Co/Other".into(),
        RegistryRule::default(),
    );
    assert!(
        one(propose(&roots, Some(&entry), &[], &[], Family::Windows))
            .notes
            .is_empty()
    );
}

#[test]
fn godot_rule_pairs_app_userdata() {
    let to_windows = one(propose(
        &meta(vec![linux_root(
            "<home>/.local/share/godot/app_userdata/Proj",
        )]),
        None,
        &[],
        &[],
        Family::Windows,
    ));
    assert_eq!(to_windows.confidence, Confidence::EngineRule);
    assert_eq!(
        to_windows.proposed.as_deref(),
        Some("<winAppData>/Godot/app_userdata/Proj")
    );
    let to_linux = one(propose(
        &meta(vec![win_root("<winAppData>/Godot/app_userdata/Proj")]),
        None,
        &[],
        &[],
        Family::Linux,
    ));
    assert_eq!(
        to_linux.proposed.as_deref(),
        Some("<xdgData>/godot/app_userdata/Proj")
    );
}

#[test]
fn unity_rule_matches_home_dot_config_spelling() {
    let spelled = linux_root("<home>/.config/unity3d/Co/Game");
    assert_eq!(spelled.0, Base::XdgConfig);
    let mapped = one(propose(
        &meta(vec![spelled]),
        None,
        &[],
        &[],
        Family::Windows,
    ));
    assert_eq!(mapped.confidence, Confidence::EngineRule);
    assert_eq!(
        mapped.proposed.as_deref(),
        Some("<winLocalAppDataLow>/Co/Game")
    );
}

#[test]
fn auto_pair_single_file_template_maps_with_empty_tail() {
    let entry = entry(&[
        ("<home>/.config/_12ibt6/savedata.ini", Some(Os::Linux)),
        ("<winAppData>/Foo/savedata.ini", Some(Os::Windows)),
    ]);
    let mapped = one(propose(
        &meta(vec![linux_root("<home>/.config/_12ibt6/savedata.ini")]),
        Some(&entry),
        &[],
        &[],
        Family::Windows,
    ));
    assert_eq!(mapped.confidence, Confidence::AutoPair);
    assert_eq!(
        mapped.proposed.as_deref(),
        Some("<winAppData>/Foo/savedata.ini")
    );
}

#[test]
fn auto_pair_file_glob_uses_parent_dir() {
    let entry = entry(&[
        ("<home>/.config/1001Spikes/*.sav", Some(Os::Linux)),
        ("<winAppData>/1001 Spikes/*.sav", Some(Os::Windows)),
    ]);
    let mapped = one(propose(
        &meta(vec![linux_root("<home>/.config/1001Spikes")]),
        Some(&entry),
        &[],
        &[],
        Family::Windows,
    ));
    assert_eq!(mapped.confidence, Confidence::AutoPair);
    assert_eq!(mapped.proposed.as_deref(), Some("<winAppData>/1001 Spikes"));
}

#[test]
fn unknown_root_lists_target_os_candidates() {
    let entry = entry(&[
        ("<xdgData>/Foo", Some(Os::Linux)),
        ("<winDocuments>/Foo/*.sav", Some(Os::Windows)),
        ("<winAppData>/Foo", Some(Os::Windows)),
        ("<winAppData>/Foo/*.cfg", None),
    ]);
    let mapped = one(propose(
        &meta(vec![linux_root("<xdgData>/Foo")]),
        Some(&entry),
        &[],
        &[],
        Family::Windows,
    ));
    assert_eq!(mapped.confidence, Confidence::Unknown);
    assert_eq!(
        mapped.candidates,
        ["<winAppData>/Foo", "<winDocuments>/Foo"]
    );
}

#[test]
fn only_roots_of_the_other_family_are_proposed() {
    let roots = meta(vec![
        linux_root("<xdgData>/Foo"),
        win_root("<winAppData>/Foo"),
        (Base::GameDir, "saves".into()),
        // Custom absolute paths restore to the same OS only (ADR-0005).
        (Base::Absolute, "opt/foo".into()),
        (Base::WinAbsolute, "C/Games/Foo".into()),
    ]);
    let entries = propose(&roots, None, &[], &[], Family::Windows);
    assert_eq!(
        entries.iter().map(|e| e.root.clone()).collect::<Vec<_>>(),
        [linux_root("<xdgData>/Foo")]
    );
    let entries = propose(&roots, None, &[], &[], Family::Linux);
    assert_eq!(
        entries.iter().map(|e| e.root.clone()).collect::<Vec<_>>(),
        [win_root("<winAppData>/Foo")]
    );
}

#[test]
fn user_choice_saved_as_pairing() {
    let linux = linux_root("<xdgData>/Foo");
    let windows = win_root("<winDocuments>/Foo");
    let mut config = config();
    apply_choices(
        &mut config.custom_games,
        "Game",
        &[
            (linux.clone(), Choice::Template("<winAppData>/Old".into())),
            (windows.clone(), Choice::Skip),
        ],
    );
    // Choosing again for the same root replaces the earlier pair.
    apply_choices(
        &mut config.custom_games,
        "Game",
        &[(linux.clone(), Choice::Template("<winAppData>/Foo".into()))],
    );
    let [game] = &config.custom_games[..] else {
        panic!("one custom game expected");
    };
    assert_eq!(game.integration, Integration::Extend);
    assert_eq!(game.pairs, [pair("<xdgData>/Foo", "<winAppData>/Foo")]);
    let mapped = one(propose(
        &meta(vec![linux_root("<xdgData>/Foo/slot")]),
        None,
        &game.pairs,
        &[],
        Family::Windows,
    ));
    assert_eq!(mapped.confidence, Confidence::Saved);
    assert_eq!(mapped.proposed.as_deref(), Some("<winAppData>/Foo/slot"));

    // A Windows root stores its own template on the Windows side of the pair.
    apply_choices(
        &mut config.custom_games,
        "Game",
        &[(windows, Choice::Template("<xdgConfig>/Foo".into()))],
    );
    assert_eq!(
        config.custom_games[0].pairs[1],
        pair("<xdgConfig>/Foo", "<winDocuments>/Foo")
    );
}

#[test]
fn apply_choices_keeps_existing_custom_game() {
    let mut config = config();
    config.custom_games.push(CustomGame {
        name: "Game".into(),
        files: Vec::new(),
        entry: entry(&[("<xdgData>/Foo", Some(Os::Linux))]),
        integration: Integration::Override,
        wine_prefix: None,
        pairs: vec![pair("<xdgData>/A", "<winAppData>/A")],
        skips: Vec::new(),
    });
    apply_choices(
        &mut config.custom_games,
        "Game",
        &[(
            linux_root("<xdgData>/B"),
            Choice::Template("<winAppData>/B".into()),
        )],
    );
    assert_eq!(config.custom_games.len(), 1);
    assert_eq!(config.custom_games[0].integration, Integration::Override);
    assert_eq!(config.custom_games[0].pairs.len(), 2);
    apply_choices(
        &mut config.custom_games,
        "Other",
        &[(linux_root("<xdgData>/B"), Choice::Skip)],
    );
    assert_eq!(config.custom_games.len(), 2);
}

#[test]
fn skip_choice_is_remembered() {
    let linux = linux_root("<xdgData>/Foo");
    let mut config = config();
    let choice = [(linux.clone(), Choice::Skip)];
    apply_choices(&mut config.custom_games, "Game", &choice);
    apply_choices(&mut config.custom_games, "Game", &choice);
    let [game] = &config.custom_games[..] else {
        panic!("one custom game expected");
    };
    assert_eq!(game.skips, ["<xdgData>/Foo"]);
    assert!(game.pairs.is_empty());

    // A pair for the same root gives way to the skip.
    let mut with_pair = config.clone();
    apply_choices(
        &mut with_pair.custom_games,
        "Game",
        &[(linux, Choice::Template("<winAppData>/Foo".into()))],
    );
    apply_choices(&mut with_pair.custom_games, "Game", &choice);
    assert_eq!(with_pair.custom_games[0].skips, ["<xdgData>/Foo"]);
    assert!(with_pair.custom_games[0].pairs.is_empty());

    // A Windows root stores its own template.
    apply_choices(
        &mut config.custom_games,
        "Game",
        &[(win_root("<winDocuments>/Foo"), Choice::Skip)],
    );
    assert_eq!(
        config.custom_games[0].skips,
        ["<xdgData>/Foo", "<winDocuments>/Foo"]
    );
}

#[test]
fn template_choice_clears_skip() {
    let linux = linux_root("<xdgData>/Foo");
    let mut config = config();
    apply_choices(
        &mut config.custom_games,
        "Game",
        &[(linux.clone(), Choice::Skip)],
    );
    apply_choices(
        &mut config.custom_games,
        "Game",
        &[(linux, Choice::Template("<winAppData>/Foo".into()))],
    );
    let game = &config.custom_games[0];
    assert!(game.skips.is_empty());
    assert_eq!(game.pairs, [pair("<xdgData>/Foo", "<winAppData>/Foo")]);
}

#[test]
fn saved_skip_needs_no_dialog() {
    let skips = ["<xdgData>/Foo".to_owned()];
    let mapped = one(propose(
        &meta(vec![linux_root("<xdgData>/Foo/slot")]),
        None,
        &[],
        &skips,
        Family::Windows,
    ));
    assert_eq!(mapped.confidence, Confidence::Saved);
    assert_eq!(mapped.proposed, None);
    let resolved = resolve(&[mapped], &[]).unwrap();
    assert_eq!(resolved[0].target, None);

    // Component boundary, and the root's own family.
    for root in [
        linux_root("<xdgData>/Foobar"),
        linux_root("<xdgConfig>/Foo"),
    ] {
        let other = one(propose(
            &meta(vec![root]),
            None,
            &[],
            &skips,
            Family::Windows,
        ));
        assert_eq!(other.confidence, Confidence::Unknown);
    }
    let back = one(propose(
        &meta(vec![win_root("<winDocuments>/Mine/x")]),
        None,
        &[],
        &["<winDocuments>/mine".to_owned()],
        Family::Linux,
    ));
    assert_eq!((back.confidence, back.proposed), (Confidence::Saved, None));

    // The deepest match wins; a pair beats a skip on a tie.
    let deep = pair("<xdgData>/Foo/sub", "<winAppData>/Deep");
    let root = linux_root("<xdgData>/Foo/sub/x");
    let mapped = one(propose(
        &meta(vec![root.clone()]),
        None,
        &[deep],
        &skips,
        Family::Windows,
    ));
    assert_eq!(mapped.proposed.as_deref(), Some("<winAppData>/Deep/x"));
    let tie = pair("<xdgData>/Foo", "<winAppData>/Tie");
    let mapped = one(propose(
        &meta(vec![linux_root("<xdgData>/Foo/x")]),
        None,
        &[tie],
        &skips,
        Family::Windows,
    ));
    assert_eq!(mapped.proposed.as_deref(), Some("<winAppData>/Tie/x"));
    let deep_skip = ["<xdgData>/Foo/sub".to_owned()];
    let shallow = pair("<xdgData>/Foo", "<winAppData>/Shallow");
    let mapped = one(propose(
        &meta(vec![root]),
        None,
        &[shallow],
        &deep_skip,
        Family::Windows,
    ));
    assert_eq!(mapped.proposed, None);
}

#[test]
fn saved_skip_beats_engine_rule() {
    let unity = linux_root("<xdgConfig>/unity3d/C/P");
    let skips = ["<xdgConfig>/unity3d/C/P".to_owned()];
    let mapped = one(propose(
        &meta(vec![unity]),
        None,
        &[],
        &skips,
        Family::Windows,
    ));
    assert_eq!(
        (mapped.confidence, mapped.proposed),
        (Confidence::Saved, None)
    );
}

#[test]
fn home_skip_applies_in_both_directions() {
    let mut config = config();
    apply_choices(
        &mut config.custom_games,
        "Game",
        &[(linux_root("<home>/Foo"), Choice::Skip)],
    );
    let skips = &config.custom_games[0].skips;
    assert_eq!(*skips, ["<home>/Foo"]);
    let back = one(propose(
        &meta(vec![win_root("<home>/Foo/x")]),
        None,
        &[],
        skips,
        Family::Linux,
    ));
    assert_eq!((back.confidence, back.proposed), (Confidence::Saved, None));

    // Choosing a target for the Windows root removes the Linux-side skip.
    apply_choices(
        &mut config.custom_games,
        "Game",
        &[(
            win_root("<home>/Foo"),
            Choice::Template("<xdgData>/Foo".into()),
        )],
    );
    assert!(config.custom_games[0].skips.is_empty());
}

#[test]
fn skip_template_is_validated() {
    assert_eq!(validate_skip("<xdgData>/Foo"), Ok(()));
    assert_eq!(validate_skip("<winAppData>/Foo"), Ok(()));
    for bad in [
        "/opt/foo",
        "C:/Games/Foo",
        "<xdgData>/Foo/*.sav",
        "<xdgData>/../x",
        "Foo",
    ] {
        assert_eq!(
            validate_skip(bad),
            Err(SkipError {
                template: bad.into()
            }),
            "{bad}"
        );
    }
}

#[test]
fn root_template_round_trips_through_classify() {
    for (family, template) in [
        (Family::Linux, "<home>/Saves/Foo"),
        (Family::Linux, "<xdgData>/Foo"),
        (Family::Linux, "<xdgConfig>/Foo"),
        (Family::Linux, "<base>/saves"),
        (Family::Windows, "<home>/Saved Games/Foo"),
        (Family::Windows, "<winDocuments>/Foo"),
        (Family::Windows, "<winAppData>/Foo"),
        (Family::Windows, "<winLocalAppData>/Foo"),
        (Family::Windows, "<winLocalAppDataLow>/Co/Game"),
        (Family::Windows, "<winPublic>/Foo"),
        (Family::Windows, "<winProgramData>/Foo"),
        (Family::Windows, "<winDir>/Foo"),
        (Family::Windows, "<root>/Foo"),
    ] {
        let classified = root(template, family);
        assert_eq!(
            root(&root_template(&classified), family),
            classified,
            "{template}"
        );
    }
}

fn unknown(root: &SnapshotRoot) -> MappingEntry {
    MappingEntry {
        root: root.clone(),
        files: 0,
        proposed: None,
        confidence: Confidence::Unknown,
        candidates: Vec::new(),
        notes: Vec::new(),
    }
}

#[test]
fn resolve_rejects_unclassifiable_template() {
    let linux = linux_root("<xdgData>/Foo");
    let windows = win_root("<winAppData>/Foo");
    for (root, template) in [
        // Relative, a template of the wrong family, a glob, and a real path.
        (&linux, "Foo/saves"),
        (&linux, "<xdgData>/Foo"),
        (&linux, "<winAppData>/Foo/*.sav"),
        (&linux, "<unknown>/Foo"),
        (&windows, "<winAppData>/Foo"),
        (&windows, "/home/me/saves"),
        (&linux, "<winAppData>/../../Startup"),
        (&windows, "<xdgData>/../../.ssh"),
    ] {
        let result = resolve(
            &[unknown(root)],
            &[(root.clone(), Choice::Template(template.into()))],
        );
        assert!(
            matches!(result, Err(ResolveError::Unclassifiable { .. })),
            "{template}"
        );
    }
}

#[test]
fn resolve_choice_overrides_engine_rule_proposal() {
    let unity = linux_root("<xdgConfig>/unity3d/Co/Game");
    let entries = propose(&meta(vec![unity.clone()]), None, &[], &[], Family::Windows);
    assert_eq!(entries[0].confidence, Confidence::EngineRule);
    let without_choice = resolve(&entries, &[]).unwrap();
    assert_eq!(
        without_choice[0].target,
        Some((Base::WinLocalAppDataLow, "Co/Game".into()))
    );
    let chosen = resolve(
        &entries,
        &[(
            unity.clone(),
            Choice::Template("<winDocuments>/Mine".into()),
        )],
    )
    .unwrap();
    assert_eq!(chosen[0].target, Some((Base::WinDocuments, "Mine".into())));
    let skipped = resolve(&entries, &[(unity, Choice::Skip)]).unwrap();
    assert_eq!(skipped[0].target, None);
}

#[test]
fn resolve_requires_every_unknown_root() {
    let known = linux_root("<xdgConfig>/unity3d/Co/Game");
    let first = linux_root("<xdgData>/A");
    let second = linux_root("<xdgData>/B");
    let entries = propose(
        &meta(vec![known, first.clone(), second.clone()]),
        None,
        &[],
        &[],
        Family::Windows,
    );
    let missing = resolve(
        &entries,
        &[(first.clone(), Choice::Template("<winAppData>/A".into()))],
    );
    assert!(matches!(
        missing,
        Err(ResolveError::MissingChoice { root }) if root == second
    ));
    let all = resolve(
        &entries,
        &[
            (first, Choice::Template("<winAppData>/A".into())),
            (second, Choice::Skip),
        ],
    )
    .unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn parent_components_never_map() {
    // A pair target, a root tail copied through a pair, and an engine capture.
    let bad_target = [pair("<xdgData>/Foo", "<winAppData>/Foo/..")];
    let good = [pair("<xdgData>/Foo", "<winAppData>/Foo")];
    for (root, pairs) in [
        (linux_root("<xdgData>/Foo/saves"), &bad_target),
        ((Base::XdgData, "Foo/../x".into()), &good),
        ((Base::XdgConfig, "unity3d/../Game".into()), &good),
    ] {
        let mapped = one(propose(
            &meta(vec![root]),
            None,
            pairs,
            &[],
            Family::Windows,
        ));
        assert_eq!(mapped.confidence, Confidence::Unknown);
    }
    let unity = (Base::XdgConfig, "unity3d/Co/Game/../../x".into());
    assert_eq!(
        one(propose(&meta(vec![unity]), None, &[], &[], Family::Windows)).confidence,
        Confidence::Unknown
    );
}

#[test]
fn playerprefs_note_survives_saved_pair() {
    let unity = linux_root("<xdgConfig>/unity3d/Co/Game");
    let mut entry = entry(&[]);
    entry.registry.insert(
        "HKEY_CURRENT_USER/Software/Co/Game".into(),
        RegistryRule::default(),
    );
    let pairs = [pair("<xdgConfig>/unity3d/Co/Game", "<winAppData>/Mine")];
    let mapped = one(propose(
        &meta(vec![unity]),
        Some(&entry),
        &pairs,
        &[],
        Family::Windows,
    ));
    assert_eq!(mapped.confidence, Confidence::Saved);
    assert_eq!(mapped.notes, [PLAYER_PREFS_NOTE]);
}

#[test]
fn saved_pair_beats_engine_rule() {
    let pairs = [pair("<xdgConfig>/unity3d/Co/Game", "<winAppData>/Mine")];
    let mapped = one(propose(
        &meta(vec![linux_root("<xdgConfig>/unity3d/Co/Game")]),
        None,
        &pairs,
        &[],
        Family::Windows,
    ));
    assert_eq!(mapped.confidence, Confidence::Saved);
    assert_eq!(mapped.proposed.as_deref(), Some("<winAppData>/Mine"));
}

#[test]
fn engine_rule_beats_auto_pair() {
    let entry = entry(&[
        ("<xdgConfig>/unity3d/Co/Game", Some(Os::Linux)),
        ("<winAppData>/Mine", Some(Os::Windows)),
    ]);
    let mapped = one(propose(
        &meta(vec![linux_root("<xdgConfig>/unity3d/Co/Game")]),
        Some(&entry),
        &[],
        &[],
        Family::Windows,
    ));
    assert_eq!(mapped.confidence, Confidence::EngineRule);
    assert_eq!(
        mapped.proposed.as_deref(),
        Some("<winLocalAppDataLow>/Co/Game")
    );
}

#[test]
fn deepest_saved_pair_wins() {
    let root = linux_root("<xdgData>/Foo/sub/x");
    let shallow = pair("<xdgData>/Foo", "<winAppData>/Shallow");
    let deep = pair("<xdgData>/Foo/sub", "<winAppData>/Deep");
    for pairs in [[shallow.clone(), deep.clone()], [deep, shallow]] {
        let mapped = one(propose(
            &meta(vec![root.clone()]),
            None,
            &pairs,
            &[],
            Family::Windows,
        ));
        assert_eq!(mapped.proposed.as_deref(), Some("<winAppData>/Deep/x"));
    }
}

#[test]
fn braces_in_pairs_are_literal() {
    let pairs = [pair("<xdgData>/{GUID}", "<winAppData>/Foo")];
    let other = one(propose(
        &meta(vec![linux_root("<xdgData>/abc")]),
        None,
        &pairs,
        &[],
        Family::Windows,
    ));
    assert_eq!(other.confidence, Confidence::Unknown);
    let literal = one(propose(
        &meta(vec![linux_root("<xdgData>/{GUID}")]),
        None,
        &pairs,
        &[],
        Family::Windows,
    ));
    assert_eq!(literal.confidence, Confidence::Saved);
}

#[test]
fn roots_keep_game_placeholder_unsubstituted() {
    // Roots are raw glob-free template prefixes; the caller substitutes `<game>` later.
    let pairs = [pair("<xdgData>/<game>", "<winAppData>/<game>")];
    let mapped = one(propose(
        &meta(vec![linux_root("<xdgData>/<game>/saves")]),
        None,
        &pairs,
        &[],
        Family::Windows,
    ));
    assert_eq!(
        mapped.proposed.as_deref(),
        Some("<winAppData>/<game>/saves")
    );
    let substituted = one(propose(
        &meta(vec![linux_root("<xdgData>/Foo/saves")]),
        None,
        &pairs,
        &[],
        Family::Windows,
    ));
    assert_eq!(substituted.confidence, Confidence::Unknown);
}

#[test]
fn apply_choices_replaces_pair_spelled_differently() {
    let mut config = config();
    config.custom_games.push(CustomGame {
        name: "Game".into(),
        files: Vec::new(),
        entry: GameEntry::default(),
        integration: Integration::Extend,
        wine_prefix: None,
        pairs: vec![pair("<home>/.config/Foo", "<winAppData>/Old")],
        skips: Vec::new(),
    });
    apply_choices(
        &mut config.custom_games,
        "Game",
        &[(
            linux_root("<xdgConfig>/Foo"),
            Choice::Template("<winAppData>/New".into()),
        )],
    );
    assert_eq!(
        config.custom_games[0].pairs,
        [pair("<xdgConfig>/Foo", "<winAppData>/New")]
    );
}

#[test]
fn validate_pair_accepts_placeholder_templates_and_names_the_bad_side() {
    assert_eq!(
        validate_pair(&pair("<xdgData>/Foo", "<winAppData>/Foo")),
        Ok(())
    );
    for (linux, windows, side) in [
        ("<winAppData>/Foo", "<winAppData>/Foo", Family::Linux),
        ("<xdgData>/Foo", "<xdgData>/Foo", Family::Windows),
        ("/home/me/Foo", "<winAppData>/Foo", Family::Linux),
        ("<xdgData>/Foo/*.sav", "<winAppData>/Foo", Family::Linux),
        ("<xdgData>/../Foo", "<winAppData>/Foo", Family::Linux),
        ("<xdgData>/Foo", "C:/Users/me/Foo", Family::Windows),
        ("", "<winAppData>/Foo", Family::Linux),
    ] {
        let error = validate_pair(&pair(linux, windows)).unwrap_err();
        assert_eq!(error.side, side, "{linux} / {windows}");
    }
}

fn linux_ctx() -> Context {
    Context::Linux(LinuxFolders {
        user: "me".into(),
        home: "/home/me".into(),
        xdg_data: "/home/me/.local/share".into(),
        xdg_config: "/home/me/.config".into(),
    })
}

fn prefix_ctx(prefix: &Path) -> Context {
    Context::WinePrefix {
        prefix: prefix.to_path_buf(),
        folders: WinFolders::for_prefix(prefix, "steamuser"),
    }
}

fn no_game() -> GameVars {
    GameVars::new("Game", vec![], &[], None)
}

fn other_root(path: &str) -> Root {
    Root {
        path: path.into(),
        kind: RootKind::Other,
    }
}

fn template(path: &str, ctx: &Context, vars: &GameVars) -> Option<String> {
    template_for(Path::new(path), ctx, vars)
}

#[test]
fn template_for_prefers_the_deepest_base() {
    let ctx = linux_ctx();
    let vars = no_game();
    assert_eq!(
        template("/home/me/.config/Foo", &ctx, &vars).as_deref(),
        Some("<xdgConfig>/Foo")
    );
    assert_eq!(
        template("/home/me/Documents/Foo", &ctx, &vars).as_deref(),
        Some("<home>/Documents/Foo")
    );
    // A base dir itself has no trailing slash.
    assert_eq!(template("/home/me", &ctx, &vars).as_deref(), Some("<home>"));
}

#[test]
fn template_for_prefers_game_dir_over_a_shallower_base() {
    let vars = GameVars::new("Foo", vec![], &[other_root("/home/me/.local/share")], None);
    let ctx = linux_ctx();
    assert_eq!(
        template("/home/me/.local/share/Foo/saves", &ctx, &vars).as_deref(),
        Some("<base>/saves")
    );
    assert_eq!(
        template("/home/me/.local/share/Foo", &ctx, &vars).as_deref(),
        Some("<base>")
    );
    assert_eq!(
        template("/home/me/.local/share/Other", &ctx, &vars).as_deref(),
        Some("<xdgData>/Other")
    );
}

#[test]
fn template_for_store_root_itself_has_no_trailing_slash() {
    let steam = Root {
        path: "/games/steam".into(),
        kind: RootKind::Steam,
    };
    let vars = GameVars::new("Foo", vec![], &[steam], None);
    assert_eq!(
        template("/games/steam", &linux_ctx(), &vars).as_deref(),
        Some("<root>")
    );
}

#[test]
fn template_for_game_dir_wins_a_tie_with_a_family_base() {
    // The game dir is `~/.config`, the same directory as `<xdgConfig>`.
    let vars = GameVars::new(
        "Game",
        vec![".config".into()],
        &[other_root("/home/me")],
        None,
    );
    assert_eq!(
        template("/home/me/.config/saves", &linux_ctx(), &vars).as_deref(),
        Some("<base>/saves")
    );
}

#[cfg(unix)]
#[test]
fn template_for_escapes_glob_characters() {
    let ctx = linux_ctx();
    let vars = no_game();
    assert_eq!(
        template("/home/me/a[1]", &ctx, &vars).as_deref(),
        Some("<home>/a[[]1[]]")
    );
    assert_eq!(
        template("/mnt/a*b", &ctx, &vars).as_deref(),
        Some("/mnt/a[*]b")
    );
}

#[test]
fn template_for_escaped_name_in_a_prefix_still_expands_to_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = prefix_ctx(dir.path());
    let pick = dir
        .path()
        .join("drive_c/users/steamuser/AppData/Roaming/a[1]");
    fs::create_dir_all(&pick).unwrap();
    let vars = no_game();
    let template = template_for(&pick, &ctx, &vars).unwrap();
    assert_eq!(template, "<winAppData>/a[[]1[]]");
    let (_, pattern) = layout::expand(&template, &ctx, &vars).remove(0);
    let found: Vec<_> = glob::glob(&pattern).unwrap().flatten().collect();
    assert_eq!(found, [pick]);
}

#[cfg(unix)]
#[test]
fn template_for_falls_back_to_the_absolute_path_on_native_linux() {
    assert_eq!(
        template("/mnt/games/Foo", &linux_ctx(), &no_game()).as_deref(),
        Some("/mnt/games/Foo")
    );
}

#[test]
fn template_for_has_no_fallback_in_a_prefix() {
    let ctx = prefix_ctx(Path::new("/pfx"));
    assert_eq!(template("/mnt/games/Foo", &ctx, &no_game()), None);
}

#[cfg(unix)]
#[test]
fn template_for_never_gives_a_template_that_resolves_elsewhere() {
    // `<home>/.config/Foo` would classify to `<xdgConfig>/Foo`, which is `/cfg/Foo` here.
    let Context::Linux(mut folders) = linux_ctx() else {
        unreachable!()
    };
    folders.xdg_config = "/cfg".into();
    assert_eq!(
        template("/home/me/.config/Foo", &Context::Linux(folders), &no_game()).as_deref(),
        Some("/home/me/.config/Foo")
    );

    // Redirected `%APPDATA%`: `<home>/AppData/Roaming/Foo` would classify to `<winAppData>/Foo`.
    let prefix = Path::new("/pfx");
    let mut folders = WinFolders::for_prefix(prefix, "steamuser");
    folders.app_data = "/elsewhere".into();
    let ctx = Context::WinePrefix {
        prefix: prefix.to_path_buf(),
        folders,
    };
    assert_eq!(
        template(
            "/pfx/drive_c/users/steamuser/AppData/Roaming/Foo",
            &ctx,
            &no_game()
        ),
        None
    );
}

#[cfg(windows)]
#[test]
fn template_for_falls_back_to_a_drive_path_on_native_windows() {
    let ctx = Context::WindowsNative(WinFolders::for_prefix(Path::new("C:/fake"), "me"));
    let vars = no_game();
    assert_eq!(
        template(r"C:\Games\Foo", &ctx, &vars).as_deref(),
        Some("C:/Games/Foo")
    );
    assert_eq!(template(r"C:\", &ctx, &vars).as_deref(), Some("C:/"));
    assert_eq!(template(r"\\server\share\Foo", &ctx, &vars), None);
}

fn file_row(linux: Option<&str>, windows: Option<&str>) -> CustomFile {
    CustomFile {
        linux: linux.map(Into::into),
        windows: windows.map(Into::into),
    }
}

#[test]
fn validate_file_needs_a_side() {
    assert_eq!(validate_file(&file_row(None, None)), Err(FileError::Empty));
}

#[test]
fn validate_file_accepts_placeholder_templates_and_absolute_paths_of_each_os() {
    for linux in [
        "<home>/g",
        "<xdgData>/g/*.sav",
        "<xdgConfig>/<game>/<storeUserId>/s[0-9].dat",
        "<home>/<osUserName>",
        "<base>/save",
        "<root>/g",
        "/opt/g",
    ] {
        assert_eq!(
            validate_file(&file_row(Some(linux), None)),
            Ok(()),
            "{linux}"
        );
    }
    for windows in [
        "<winDocuments>/G/*.sav",
        "<home>/G",
        "<base>/save",
        r"C:\Games\G",
        "C:/Games/G",
    ] {
        let file = file_row(None, Some(windows));
        assert_eq!(validate_file(&file), Ok(()), "{windows}");
    }
}

#[test]
fn validate_file_names_the_side_that_is_not_a_path_of_its_os() {
    for (linux, windows, family) in [
        (Some("<winAppData>/g"), None, Family::Linux),
        (Some("relative/g"), None, Family::Linux),
        (Some(r"C:\Games\G"), None, Family::Linux),
        (Some("<home>/../g"), None, Family::Linux),
        (Some("/opt/./g"), None, Family::Linux),
        (Some(""), None, Family::Linux),
        (None, Some("<xdgData>/g"), Family::Windows),
        (None, Some("/opt/g"), Family::Windows),
        (None, Some("C:G"), Family::Windows),
        (None, Some(r"\\server\share\G"), Family::Windows),
        (None, Some(r"C:\Games\..\G"), Family::Windows),
    ] {
        let error = validate_file(&file_row(linux, windows)).unwrap_err();
        assert!(
            matches!(error, FileError::Side { family: f, .. } if f == family),
            "{linux:?} / {windows:?}: {error:?}"
        );
    }
}

#[test]
fn validate_file_accepts_any_two_valid_sides() {
    for (linux, windows) in [
        ("<xdgData>/g/*.sav", "<winDocuments>/G/Saves/*.sav"),
        ("/opt/g", "<winAppData>/G"),
        ("<xdgData>/g", r"C:\Games\G\*.sav"),
        ("<home>/*.sav", "<winDocuments>/G/*.sav"),
        ("<base>/save", "<base>/save"),
    ] {
        let file = file_row(Some(linux), Some(windows));
        assert_eq!(validate_file(&file), Ok(()), "{linux} / {windows}");
    }
}

#[test]
fn validate_file_rejects_a_backslash_after_a_placeholder() {
    for (linux, windows, family) in [
        (Some(r"<xdgData>\g"), None, Family::Linux),
        (None, Some(r"<winDocuments>\My Games\G"), Family::Windows),
        (None, Some(r"<winDocuments>/G\..\H"), Family::Windows),
    ] {
        let error = validate_file(&file_row(linux, windows)).unwrap_err();
        assert!(
            matches!(error, FileError::Backslash { family: f, .. } if f == family),
            "{linux:?} / {windows:?}: {error:?}"
        );
    }
}

#[test]
fn apply_choices_does_not_store_a_pair_a_file_row_declares() {
    let mut config = config();
    config.custom_games.push(CustomGame {
        name: "Game".into(),
        files: vec![file_row(
            Some("<home>/.local/share/g/*.sav"),
            Some("<winDocuments>/G/*.sav"),
        )],
        entry: GameEntry::default(),
        integration: Integration::Extend,
        wine_prefix: None,
        pairs: Vec::new(),
        skips: vec!["<xdgData>/g".into()],
    });
    apply_choices(
        &mut config.custom_games,
        "Game",
        &[
            (
                linux_root("<xdgData>/g"),
                Choice::Template("<winDocuments>/G".into()),
            ),
            (
                linux_root("<xdgData>/h"),
                Choice::Template("<winDocuments>/H".into()),
            ),
        ],
    );
    let game = &config.custom_games[0];
    assert_eq!(game.pairs, [pair("<xdgData>/h", "<winDocuments>/H")]);
    assert!(game.skips.is_empty(), "the choice still clears the skip");
}
