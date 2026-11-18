use super::*;

const SAMPLE: &str = include_str!("../../tests/fixtures/manifest-sample.yaml");

fn sample() -> Manifest {
    parse_yaml(SAMPLE).unwrap()
}

fn custom(name: &str, integration: Integration, template: &str) -> CustomGame {
    let rule = FileRule {
        tags: vec![Tag::Save],
        when: vec![When::default()],
    };
    CustomGame {
        name: name.into(),
        files: Vec::new(),
        entry: GameEntry {
            files: BTreeMap::from([(template.into(), rule)]),
            ..GameEntry::default()
        },
        integration,
        wine_prefix: None,
        pairs: vec![],
        skips: Vec::new(),
    }
}

#[test]
fn alias_entry_is_skipped() {
    let manifest = sample();
    assert!(!manifest.games.contains_key("-KLAUS-"));
    assert!(manifest.games.contains_key("Klaus"));
    assert_eq!(manifest.games.len(), 4);
}

#[test]
fn unknown_fields_and_values_are_ignored() {
    let yaml = "
Game:
  cloud: {steam: true}
  launch: {x: []}
  files:
    <base>/a:
      tags: [save, future]
      extra: 1
      when: [{os: haiku, store: itch, bit: 64}]
";
    let rule = &parse_yaml(yaml).unwrap().games["Game"].files["<base>/a"];
    assert_eq!(rule.tags, [Tag::Save, Tag::Other]);
    assert_eq!(
        rule.when,
        [When {
            os: Some(Os::Other),
            store: Some(Store::Other)
        }]
    );
}

#[test]
fn fixture_entries_keep_files_registry_install_dir_and_steam() {
    let manifest = sample();

    let klaus = &manifest.games["Klaus"];
    assert_eq!(klaus.steam, Some(Steam { id: Some(729370) }));
    assert!(klaus.install_dir.contains_key("KLAUS"));
    let sav =
        &klaus.files["<home>/AppData/LocalLow/La Cosa Entertainment/KLAUS/<storeUserId>/*.sav"];
    assert!(sav.tags.is_empty());
    assert_eq!(
        sav.when,
        [When {
            os: Some(Os::Windows),
            store: Some(Store::Steam)
        }]
    );

    let somnium = &manifest.games["AI: The Somnium Files"];
    let key = &somnium.registry["HKEY_CURRENT_USER/Software/SpikeChunsoft/AI_TheSomniumFiles"];
    assert_eq!(key.tags, [Tag::Config]);
    assert_eq!(
        key.when,
        [When {
            os: None,
            store: Some(Store::Microsoft)
        }]
    );
}

#[test]
fn unity_game_has_linux_and_windows_rules() {
    let files = &sample().games["Hollow Knight"].files;
    let applies_to =
        |template: &str, os: Os| files[template].when.iter().any(|when| when.os == Some(os));
    assert!(applies_to(
        "<xdgConfig>/unity3d/Team Cherry/Hollow Knight/*.dat",
        Os::Linux
    ));
    assert!(applies_to(
        "<home>/AppData/LocalLow/Team Cherry/Hollow Knight/*.dat",
        Os::Windows
    ));
    assert_eq!(
        files["<base>/hollow_knight_Data/Config.ini"].when.len(),
        2,
        "one rule can list several constraints"
    );
}

#[test]
fn yaml_to_json_to_struct_is_equal() {
    let manifest = sample();
    let json = to_json(&manifest).unwrap();
    assert_eq!(from_json(&json).unwrap(), manifest);
}

#[test]
fn malformed_input_is_a_typed_error() {
    assert!(matches!(
        parse_yaml("Game: [unclosed"),
        Err(ManifestError::Yaml(_))
    ));
    assert!(matches!(from_json("{"), Err(ManifestError::Json(_))));
}

#[test]
fn override_replaces_the_entry() {
    let mut manifest = sample();
    let game = custom("Hollow Knight", Integration::Override, "<base>/mine");
    merge_custom(&mut manifest, std::slice::from_ref(&game));
    assert_eq!(manifest.games["Hollow Knight"], game.entry);
}

#[test]
fn extend_appends_to_the_entry() {
    let mut manifest = sample();
    let before = manifest.games["Hollow Knight"].clone();
    let mut game = custom("Hollow Knight", Integration::Extend, "<base>/mine");
    game.entry
        .install_dir
        .insert("Hollow Knight Alt".into(), InstallDir {});
    game.entry.steam = Some(Steam { id: Some(1) });
    merge_custom(&mut manifest, &[game]);
    let after = &manifest.games["Hollow Knight"];
    assert_eq!(after.files.len(), before.files.len() + 1);
    assert!(after.files.contains_key("<base>/mine"));
    assert_eq!(after.install_dir.len(), before.install_dir.len() + 1);
    assert!(after.install_dir.contains_key("Hollow Knight Alt"));
    assert_eq!(after.registry, before.registry);
    assert_eq!(after.steam, before.steam, "manifest steam wins");
}

#[test]
fn extend_takes_custom_steam_when_manifest_has_none() {
    let mut manifest = Manifest::default();
    manifest
        .games
        .insert("Homebrew".into(), GameEntry::default());
    let mut game = custom("Homebrew", Integration::Extend, "<base>/mine");
    game.entry.steam = Some(Steam { id: Some(7) });
    merge_custom(&mut manifest, &[game]);
    assert_eq!(
        manifest.games["Homebrew"].steam,
        Some(Steam { id: Some(7) })
    );
}

#[test]
fn unknown_title_is_inserted() {
    for integration in [Integration::Override, Integration::Extend] {
        let mut manifest = sample();
        let game = custom("Homebrew", integration, "<base>/mine");
        merge_custom(&mut manifest, std::slice::from_ref(&game));
        assert_eq!(manifest.games["Homebrew"], game.entry);
    }
}

#[test]
fn custom_game_round_trips_through_yaml() {
    let mut game = custom("Homebrew", Integration::Extend, "<base>/mine");
    game.wine_prefix = Some("/home/me/prefix".into());
    game.pairs = vec![Pair {
        linux: "<xdgConfig>/Homebrew".into(),
        windows: "<winAppData>/Homebrew".into(),
    }];
    let yaml = serde_saphyr::to_string(&game).unwrap();
    assert_eq!(serde_saphyr::from_str::<CustomGame>(&yaml).unwrap(), game);
}

fn row(linux: Option<&str>, windows: Option<&str>) -> CustomFile {
    CustomFile {
        linux: linux.map(Into::into),
        windows: windows.map(Into::into),
    }
}

fn rows_game(files: Vec<CustomFile>) -> CustomGame {
    CustomGame {
        files,
        ..custom("Homebrew", Integration::Override, "<base>/mine")
    }
}

fn oses(entry: &GameEntry, template: &str) -> Vec<Option<Os>> {
    entry.files[template]
        .when
        .iter()
        .map(|when| when.os)
        .collect()
}

#[test]
fn each_row_side_is_a_save_rule_of_its_os() {
    let game = rows_game(vec![row(Some("<xdgData>/g"), Some("<winDocuments>/G"))]);
    let entry = game.game_entry();
    assert_eq!(oses(&entry, "<xdgData>/g"), [Some(Os::Linux)]);
    assert_eq!(oses(&entry, "<winDocuments>/G"), [Some(Os::Windows)]);
    assert_eq!(entry.files["<xdgData>/g"].tags, [Tag::Save]);
    assert!(entry.files.contains_key("<base>/mine"), "entry files kept");
}

#[test]
fn a_template_on_both_sides_applies_to_both_oses() {
    for files in [
        vec![row(Some("<base>/save"), Some("<base>/save"))],
        vec![
            row(Some("<base>/save"), None),
            row(None, Some("<base>/save")),
            row(Some("<base>/save"), None),
        ],
    ] {
        let entry = rows_game(files).game_entry();
        assert_eq!(
            oses(&entry, "<base>/save"),
            [Some(Os::Linux), Some(Os::Windows)]
        );
    }
}

#[test]
fn merge_custom_takes_the_rows() {
    let mut manifest = sample();
    let game = rows_game(vec![row(None, Some("<winAppData>/Homebrew"))]);
    merge_custom(&mut manifest, std::slice::from_ref(&game));
    assert_eq!(
        oses(&manifest.games["Homebrew"], "<winAppData>/Homebrew"),
        [Some(Os::Windows)]
    );
}

#[test]
fn file_pairs_map_rows_with_two_different_sides_through_their_glob_free_prefixes() {
    let game = rows_game(vec![
        row(
            Some("<xdgData>/g/*.sav"),
            Some("<winDocuments>/G/Saves/*.sav"),
        ),
        row(Some("<base>/save"), Some("<base>/save")),
        row(Some("<xdgData>/only"), None),
        row(Some("/opt/g"), Some("<winAppData>/G")),
        row(Some("<home>/*.sav"), Some("<winDocuments>/G/*.sav")),
    ]);
    assert_eq!(
        game.file_pairs(),
        [Pair {
            linux: "<xdgData>/g".into(),
            windows: "<winDocuments>/G/Saves".into(),
        }]
    );
}
