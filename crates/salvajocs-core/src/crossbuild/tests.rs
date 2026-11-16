use super::*;
use crate::config::PasswordMode;
use crate::manifest::{RegistryRule, When};

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
        Family::Linux,
    ));
    assert_eq!(back.proposed.as_deref(), Some("<xdgData>/Foo/Saves"));
    // A sibling that only shares a name prefix does not match.
    let other = one(propose(
        &meta(vec![linux_root("<xdgData>/Foobar")]),
        None,
        &pairs,
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
        Family::Windows,
    ));
    assert_eq!(short.confidence, Confidence::Unknown);
}

#[test]
fn unity_playerprefs_registry_adds_note() {
    let roots = meta(vec![linux_root("<xdgConfig>/unity3d/Co/Game")]);
    let mut entry = entry(&[("<xdgConfig>/unity3d/Co/Game", Some(Os::Linux))]);
    let without = one(propose(&roots, Some(&entry), &[], Family::Windows));
    assert!(without.notes.is_empty());
    entry.registry.insert(
        "HKEY_CURRENT_USER\\Software\\co\\GAME".into(),
        RegistryRule::default(),
    );
    let with = one(propose(&roots, Some(&entry), &[], Family::Windows));
    assert_eq!(with.confidence, Confidence::EngineRule);
    assert_eq!(with.notes, [PLAYER_PREFS_NOTE]);
    // A key of another game does not trigger the note.
    entry.registry.clear();
    entry.registry.insert(
        "HKEY_CURRENT_USER/Software/Co/Other".into(),
        RegistryRule::default(),
    );
    assert!(
        one(propose(&roots, Some(&entry), &[], Family::Windows))
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
    let mapped = one(propose(&meta(vec![spelled]), None, &[], Family::Windows));
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
    ]);
    let entries = propose(&roots, None, &[], Family::Windows);
    assert_eq!(
        entries.iter().map(|e| e.root.clone()).collect::<Vec<_>>(),
        [linux_root("<xdgData>/Foo")]
    );
}

#[test]
fn user_choice_saved_as_pairing() {
    let linux = linux_root("<xdgData>/Foo");
    let windows = win_root("<winDocuments>/Foo");
    let mut config = config();
    apply_choices(
        &mut config,
        "Game",
        &[
            (linux.clone(), Choice::Template("<winAppData>/Old".into())),
            (windows.clone(), Choice::Skip),
        ],
    );
    // Choosing again for the same root replaces the earlier pair.
    apply_choices(
        &mut config,
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
        Family::Windows,
    ));
    assert_eq!(mapped.confidence, Confidence::Saved);
    assert_eq!(mapped.proposed.as_deref(), Some("<winAppData>/Foo/slot"));

    // A Windows root stores its own template on the Windows side of the pair.
    apply_choices(
        &mut config,
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
        entry: entry(&[("<xdgData>/Foo", Some(Os::Linux))]),
        integration: Integration::Override,
        wine_prefix: None,
        pairs: vec![pair("<xdgData>/A", "<winAppData>/A")],
    });
    apply_choices(
        &mut config,
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
        &mut config,
        "Other",
        &[(linux_root("<xdgData>/B"), Choice::Skip)],
    );
    assert_eq!(config.custom_games.len(), 1);
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
    let entries = propose(&meta(vec![unity.clone()]), None, &[], Family::Windows);
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
        let mapped = one(propose(&meta(vec![root]), None, pairs, Family::Windows));
        assert_eq!(mapped.confidence, Confidence::Unknown);
    }
    let unity = (Base::XdgConfig, "unity3d/Co/Game/../../x".into());
    assert_eq!(
        one(propose(&meta(vec![unity]), None, &[], Family::Windows)).confidence,
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
        Family::Windows,
    ));
    assert_eq!(other.confidence, Confidence::Unknown);
    let literal = one(propose(
        &meta(vec![linux_root("<xdgData>/{GUID}")]),
        None,
        &pairs,
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
        Family::Windows,
    ));
    assert_eq!(substituted.confidence, Confidence::Unknown);
}

#[test]
fn apply_choices_replaces_pair_spelled_differently() {
    let mut config = config();
    config.custom_games.push(CustomGame {
        name: "Game".into(),
        entry: GameEntry::default(),
        integration: Integration::Extend,
        wine_prefix: None,
        pairs: vec![pair("<home>/.config/Foo", "<winAppData>/Old")],
    });
    apply_choices(
        &mut config,
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
