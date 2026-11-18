use std::fs;
use std::path::Path;

use super::*;
use crate::crossbuild::{Confidence, PLAYER_PREFS_NOTE, root_template};
use crate::layout::{Root, RootKind, SNAPSHOT_FORMAT, WinFolders};
use crate::manifest::{CustomFile, CustomGame, FileRule, Integration, Manifest, RegistryRule};
use crate::registry::HKCU;

fn staged(roots: &[(Base, &str)], files: &[(Base, &str)]) -> StagedGame {
    let owned = |list: &[(Base, &str)]| {
        list.iter()
            .map(|&(base, path)| (base, path.to_owned()))
            .collect()
    };
    StagedGame {
        root: "/stage/game".into(),
        meta: SnapshotMeta {
            format: SNAPSHOT_FORMAT,
            title: "Game".into(),
            roots: owned(roots),
        },
        files: owned(files),
        registry: None,
    }
}

fn linux_folders() -> LinuxFolders {
    LinuxFolders {
        user: "me".into(),
        home: "/home/me".into(),
        xdg_data: "/home/me/.local/share".into(),
        xdg_config: "/home/me/.config".into(),
    }
}

/// A native Windows user; plain data, so it works on any host.
fn native_win() -> Context {
    Context::WindowsNative(WinFolders::for_prefix(Path::new("/win"), "Alice"))
}

fn linux_only() -> Targets {
    Targets {
        linux: Some(linux_folders()),
        win: None,
    }
}

fn win_only(win: Context) -> Targets {
    Targets {
        linux: None,
        win: Some(win),
    }
}

fn vars() -> GameVars {
    GameVars::new("Game", vec![], &[], None)
}

fn file(base: Base, tail: &str) -> Option<(Base, String)> {
    Some((base, tail.to_owned()))
}

fn targets_of(plan: &RestorePlan) -> Vec<&Path> {
    plan.writes.iter().map(|w| w.target.as_path()).collect()
}

#[test]
fn proton_save_plans_into_windows_folders() {
    let staged = staged(
        &[
            (Base::WinAppData, "Foo"),
            (Base::WinDocuments, "My Games/Foo"),
        ],
        &[
            (Base::WinAppData, "Foo/save.dat"),
            (Base::WinDocuments, "My Games/Foo/slot1.sav"),
        ],
    );
    let plan = plan(
        &staged,
        &win_only(native_win()),
        &vars(),
        Some(&entry_of(&staged)),
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [
            Path::new("/win/drive_c/users/Alice/AppData/Roaming/Foo/save.dat"),
            Path::new("/win/drive_c/users/Alice/Documents/My Games/Foo/slot1.sav"),
        ]
    );
    let first = &plan.writes[0];
    assert_eq!(
        first.source,
        Path::new("/stage/game/files/WinAppData/Foo/save.dat")
    );
    assert_eq!(first.target_file, file(Base::WinAppData, "Foo/save.dat"));
    assert_eq!(first.target_root, file(Base::WinAppData, "Foo"));
    assert!(plan.skipped.is_empty());
    assert!(!plan.cross_build);
}

#[test]
fn windows_save_plans_into_prefix_user() {
    let prefix = tempfile::tempdir().unwrap();
    let folders = WinFolders::for_prefix(prefix.path(), "steamuser");
    fs::create_dir_all(folders.local_app_data_low.join("co")).unwrap();
    let staged = staged(
        &[
            (Base::WinLocalAppDataLow, "Co/Game"),
            (Base::WinAbsolute, "C/Games/Foo"),
        ],
        &[
            // Scanned case-insensitively, so the disk case can differ from the root's.
            (Base::WinLocalAppDataLow, "CO/Game/save.dat"),
            (Base::WinAbsolute, "C/Games/Foo/save.dat"),
        ],
    );
    let ctx = Context::WinePrefix {
        prefix: prefix.path().into(),
        folders: folders.clone(),
    };
    let plan = plan(
        &staged,
        &win_only(ctx),
        &vars(),
        Some(&entry_of(&staged)),
        &[],
        &[],
    )
    .unwrap();
    // The existing `co` is reused instead of creating `Co` next to it (ADR-0005). NTFS ignores
    // case, so there the existing `co` is found under the tail's spelling.
    #[cfg(unix)]
    assert_eq!(
        targets_of(&plan),
        [folders.local_app_data_low.join("co/Game/save.dat")]
    );
    assert_eq!(plan.writes.len(), 1);
    assert_eq!(
        plan.writes[0].target_root,
        file(Base::WinLocalAppDataLow, "Co/Game")
    );
    // A drive path restores to native Windows only. Off Windows its root is not even declared.
    let reason = if cfg!(windows) {
        SkipReason::NoTarget
    } else {
        SkipReason::UndeclaredRoot
    };
    assert_eq!(
        plan.skipped,
        [Skipped {
            item: SkippedItem::File(Base::WinAbsolute, "C/Games/Foo/save.dat".into()),
            reason,
        }]
    );
    assert!(!plan.cross_build);
}

#[test]
fn mixed_snapshot_restores_both_families() {
    let staged = staged(
        &[(Base::XdgData, "Game"), (Base::WinAppData, "Game")],
        &[
            (Base::XdgData, "Game/linux.sav"),
            (Base::WinAppData, "Game/windows.sav"),
        ],
    );
    let targets = Targets {
        linux: Some(linux_folders()),
        win: Some(native_win()),
    };
    let plan = plan(
        &staged,
        &targets,
        &vars(),
        Some(&entry_of(&staged)),
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [
            Path::new("/home/me/.local/share/Game/linux.sav"),
            Path::new("/win/drive_c/users/Alice/AppData/Roaming/Game/windows.sav"),
        ]
    );
    assert!(plan.skipped.is_empty());
    assert!(!plan.cross_build);
}

fn other_root(path: &Path) -> [Root; 1] {
    [Root {
        path: path.into(),
        kind: RootKind::Other,
    }]
}

#[test]
fn game_dir_resolves_through_install_dir() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("Install Dir")).unwrap();
    let roots = other_root(root.path());
    let vars = GameVars::new("Title", vec!["Install Dir".into()], &roots, None);
    let staged = staged(
        &[(Base::GameDir, "saves")],
        &[(Base::GameDir, "saves/a.sav")],
    );
    let plan = plan(
        &staged,
        &linux_only(),
        &vars,
        Some(&entry_of(&staged)),
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [root.path().join("Install Dir/saves/a.sav")]
    );
    assert!(plan.skipped.is_empty());
}

#[test]
fn game_dir_skipped_when_not_installed() {
    let root = tempfile::tempdir().unwrap();
    let roots = other_root(root.path());
    let vars = GameVars::new("Title", vec!["Install Dir".into()], &roots, None);
    let staged = staged(
        &[(Base::GameDir, "saves")],
        &[(Base::GameDir, "saves/a.sav")],
    );
    let plan = plan(
        &staged,
        &linux_only(),
        &vars,
        Some(&entry_of(&staged)),
        &[],
        &[],
    )
    .unwrap();
    assert!(plan.writes.is_empty());
    assert_eq!(
        plan.skipped,
        [Skipped {
            item: SkippedItem::File(Base::GameDir, "saves/a.sav".into()),
            reason: SkipReason::NotInstalled,
        }]
    );
}

#[test]
fn registry_skipped_without_windows_target() {
    let mut staged = staged(&[(Base::XdgData, "Game")], &[(Base::XdgData, "Game/a")]);
    let export = registry(&["Software\\Game"]);
    staged.registry = Some(export.clone());
    let entry = declaring(&staged, &["HKEY_CURRENT_USER/Software/Game"]);
    let plan_linux = plan(&staged, &linux_only(), &vars(), Some(&entry), &[], &[]).unwrap();
    assert_eq!(plan_linux.registry, None);
    let skipped = Skipped {
        item: SkippedItem::Registry,
        reason: SkipReason::NoWindowsTarget,
    };
    assert_eq!(plan_linux.skipped, [skipped]);
    // ADR-0005 fixes this text.
    assert_eq!(
        SkipReason::NoWindowsTarget.to_string(),
        "registry: no Windows target"
    );

    let targets = Targets {
        linux: Some(linux_folders()),
        win: Some(native_win()),
    };
    let plan_win = plan(&staged, &targets, &vars(), Some(&entry), &[], &[]).unwrap();
    assert_eq!(plan_win.registry, Some((export, native_win())));
    assert!(plan_win.skipped.is_empty());
}

/// An export holding `HKEY_CURRENT_USER\<key>` for each key, without values.
fn registry(keys: &[&str]) -> RegistryExport {
    RegistryExport {
        keys: keys
            .iter()
            .map(|key| (format!("{HKCU}\\{key}"), Vec::new()))
            .collect(),
    }
}

/// An entry declaring `keys` and a file template for every root of `staged`.
fn declaring(staged: &StagedGame, keys: &[&str]) -> GameEntry {
    GameEntry {
        registry: keys
            .iter()
            .map(|&key| (key.to_owned(), RegistryRule::default()))
            .collect(),
        ..entry_of(staged)
    }
}

/// An entry with a file template for every root of `staged`, so all of them are declared.
fn entry_of(staged: &StagedGame) -> GameEntry {
    templates(
        &staged
            .meta
            .roots
            .iter()
            .map(root_template)
            .collect::<Vec<_>>(),
    )
}

fn info(entry: Option<&GameEntry>, pairs: &[Pair], skips: &[String]) -> GameInfo {
    GameInfo {
        entry: entry.cloned(),
        pairs: pairs.to_vec(),
        row_pairs: Vec::new(),
        skips: skips.to_vec(),
    }
}

fn templates(templates: &[impl AsRef<str>]) -> GameEntry {
    GameEntry {
        files: templates
            .iter()
            .map(|template| (template.as_ref().to_owned(), FileRule::default()))
            .collect(),
        ..GameEntry::default()
    }
}

fn undeclared(keys: &[&str]) -> Vec<Skipped> {
    keys.iter()
        .map(|key| Skipped {
            item: SkippedItem::RegistryKey(format!("{HKCU}\\{key}")),
            reason: SkipReason::UndeclaredRegistryKey,
        })
        .collect()
}

#[test]
fn undeclared_registry_key_skipped() {
    let mut staged = staged(&[], &[]);
    staged.registry = Some(registry(&[
        "Software\\Game",
        "Software\\Game\\Sub",
        "Software\\GameX",
        "Software\\Other",
    ]));
    // Declared keys use `/` and may differ in case.
    let entry = declaring(&staged, &["HKEY_CURRENT_USER/software/game"]);
    let plan = plan(
        &staged,
        &win_only(native_win()),
        &vars(),
        Some(&entry),
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        plan.registry,
        Some((
            registry(&["Software\\Game", "Software\\Game\\Sub"]),
            native_win()
        ))
    );
    assert_eq!(
        plan.skipped,
        undeclared(&["Software\\GameX", "Software\\Other"])
    );
}

#[test]
fn registry_skipped_without_game_entry() {
    let mut staged = staged(&[], &[]);
    staged.registry = Some(registry(&["Software\\Game"]));
    let plan = plan(
        &staged,
        &win_only(native_win()),
        &vars(),
        Some(&entry_of(&staged)),
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(plan.registry, None);
    assert_eq!(plan.skipped, undeclared(&["Software\\Game"]));
}

#[test]
fn override_target_rejects_relative_path() {
    let staged = staged(&[(Base::XdgData, "Game")], &[(Base::XdgData, "Game/a")]);
    let mut plan = plan(
        &staged,
        &linux_only(),
        &vars(),
        Some(&entry_of(&staged)),
        &[],
        &[],
    )
    .unwrap();
    let before = plan.clone();
    // Absolute on every host, unlike `/elsewhere` on Windows.
    let elsewhere = std::env::temp_dir().join("elsewhere/a");
    assert_eq!(
        plan.override_target(0, "elsewhere/a".into()),
        Err(OverrideError::RelativePath)
    );
    assert_eq!(
        plan.override_target(1, elsewhere.clone()),
        Err(OverrideError::NoSuchWrite(1))
    );
    assert_eq!(plan, before);

    plan.override_target(0, elsewhere.clone()).unwrap();
    let write = &plan.writes[0];
    assert_eq!(write.target, elsewhere);
    assert_eq!(write.target_file, None);
    assert_eq!(write.target_root, None);
}

#[test]
fn tail_leaving_its_base_is_skipped() {
    let staged = staged(
        &[(Base::XdgData, "Game")],
        &[(Base::XdgData, "Game/../../x")],
    );
    let direct = plan(
        &staged,
        &linux_only(),
        &vars(),
        Some(&entry_of(&staged)),
        &[],
        &[],
    )
    .unwrap();
    assert!(direct.writes.is_empty());
    assert_eq!(
        direct.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [SkipReason::UnsafePath]
    );

    // Through a mapping, the tail below the root is checked against the target base.
    let mapping = ResolvedMapping {
        root: (Base::XdgData, "Game".into()),
        target: Some((Base::WinAppData, "Game".into())),
    };
    let mapped = plan(
        &staged,
        &win_only(native_win()),
        &vars(),
        Some(&entry_of(&staged)),
        &[],
        &[mapping],
    )
    .unwrap();
    assert!(mapped.writes.is_empty());
    assert_eq!(
        mapped.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [SkipReason::UnsafePath]
    );
}

#[test]
fn file_outside_recorded_roots_skipped() {
    let staged = staged(
        &[(Base::XdgData, "Game")],
        &[
            (Base::XdgData, "Game/a.sav"),
            (Base::XdgData, "Other/b.sav"),
            (Base::XdgData, "../escape"),
            (Base::XdgConfig, "Game/c.sav"),
        ],
    );
    let plan = plan(
        &staged,
        &linux_only(),
        &vars(),
        Some(&entry_of(&staged)),
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [Path::new("/home/me/.local/share/Game/a.sav")]
    );
    assert_eq!(
        plan.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [SkipReason::OutsideRoots; 3]
    );
}

#[test]
fn os_user_name_in_tail_is_swapped_proton_to_windows() {
    let staged = staged(
        &[(Base::WinDocuments, "<osUserName>/Game")],
        &[(Base::WinDocuments, "steamuser/Game/s.sav")],
    );
    let plan = plan(
        &staged,
        &win_only(native_win()),
        &vars(),
        Some(&entry_of(&staged)),
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [Path::new(
            "/win/drive_c/users/Alice/Documents/Alice/Game/s.sav"
        )]
    );
    let write = &plan.writes[0];
    assert_eq!(
        write.target_file,
        file(Base::WinDocuments, "Alice/Game/s.sav")
    );
    assert_eq!(
        write.target_root,
        file(Base::WinDocuments, "<osUserName>/Game")
    );
}

#[test]
fn mixed_restore_uses_each_family_user() {
    let staged = staged(
        &[
            (Base::XdgData, "saves_<osUserName>"),
            (Base::WinDocuments, "<osUserName>/Game"),
        ],
        &[
            (Base::XdgData, "saves_olduser/a.sav"),
            (Base::WinDocuments, "steamuser/Game/b.sav"),
        ],
    );
    let targets = Targets {
        linux: Some(linux_folders()),
        win: Some(native_win()),
    };
    let plan = plan(
        &staged,
        &targets,
        &vars(),
        Some(&entry_of(&staged)),
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [
            Path::new("/home/me/.local/share/saves_me/a.sav"),
            Path::new("/win/drive_c/users/Alice/Documents/Alice/Game/b.sav"),
        ]
    );
}

#[test]
fn absolute_mapping_target_is_skipped() {
    let staged = staged(&[(Base::XdgData, "Game")], &[(Base::XdgData, "Game/a")]);
    let mapping = ResolvedMapping {
        root: (Base::XdgData, "Game".into()),
        target: Some((Base::WinAbsolute, "C/Games/Game".into())),
    };
    let plan = plan(
        &staged,
        &win_only(native_win()),
        &vars(),
        Some(&entry_of(&staged)),
        &[],
        &[mapping],
    )
    .unwrap();
    assert!(plan.writes.is_empty());
    assert_eq!(
        plan.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [SkipReason::NoTarget]
    );
}

#[test]
fn unmapped_root_without_target_is_an_error() {
    let staged = staged(&[(Base::XdgData, "Game")], &[(Base::XdgData, "Game/a")]);
    assert_eq!(
        plan(
            &staged,
            &win_only(native_win()),
            &vars(),
            Some(&entry_of(&staged)),
            &[],
            &[]
        ),
        Err(PlanError::Unmapped {
            root: (Base::XdgData, "Game".into()),
        })
    );
}

fn root(base: Base, rest: &str) -> SnapshotRoot {
    (base, rest.to_owned())
}

#[test]
fn native_linux_save_restores_without_prefix() {
    let staged = staged(
        &[(Base::XdgConfig, "Game")],
        &[(Base::XdgConfig, "Game/save.dat")],
    );
    let plan = prepare(
        &staged,
        &linux_only(),
        &vars(),
        &info(Some(&entry_of(&staged)), &[], &[]),
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [Path::new("/home/me/.config/Game/save.dat")]
    );
    assert!(plan.skipped.is_empty());
    assert!(!plan.cross_build);
}

/// A Linux snapshot with a Unity root (engine rule) and an unknown root.
fn linux_unity_and_unknown() -> StagedGame {
    staged(
        &[
            (Base::XdgConfig, "unity3d/Co/Game"),
            (Base::XdgData, "Game"),
        ],
        &[
            (Base::XdgConfig, "unity3d/Co/Game/prefs.dat"),
            (Base::XdgData, "Game/a.sav"),
            (Base::XdgData, "Game/sub/b.sav"),
        ],
    )
}

#[test]
fn cross_build_unknown_entry_blocks_restore() {
    let result = prepare(
        &linux_unity_and_unknown(),
        &win_only(native_win()),
        &vars(),
        &info(Some(&entry_of(&linux_unity_and_unknown())), &[], &[]),
        &[],
    );
    let Err(PrepareError::NeedsMapping(entries)) = result else {
        panic!("expected NeedsMapping, got {result:?}");
    };
    let summary: Vec<_> = entries
        .iter()
        .map(|e| (e.root.clone(), e.confidence, e.files, e.proposed.as_deref()))
        .collect();
    assert_eq!(
        summary,
        [
            (
                root(Base::XdgConfig, "unity3d/Co/Game"),
                Confidence::EngineRule,
                1,
                Some("<winLocalAppDataLow>/Co/Game"),
            ),
            (root(Base::XdgData, "Game"), Confidence::Unknown, 2, None),
        ]
    );
}

#[test]
fn nested_roots_count_files_under_the_deepest() {
    let staged = staged(
        &[(Base::XdgData, "Game"), (Base::XdgData, "Game/sub")],
        &[
            (Base::XdgData, "Game/a.sav"),
            (Base::XdgData, "Game/sub/b.sav"),
            (Base::XdgData, "Game/sub/c.sav"),
        ],
    );
    let result = prepare(
        &staged,
        &win_only(native_win()),
        &vars(),
        &info(Some(&entry_of(&staged)), &[], &[]),
        &[],
    );
    let Err(PrepareError::NeedsMapping(entries)) = result else {
        panic!("expected NeedsMapping, got {result:?}");
    };
    assert_eq!(entries.iter().map(|e| e.files).collect::<Vec<_>>(), [1, 2]);
}

#[test]
fn prepare_with_choices_returns_plan() {
    let staged = linux_unity_and_unknown();
    let targets = win_only(native_win());
    let choose = |choice: Choice| [(root(Base::XdgData, "Game"), choice)];
    let choices = choose(Choice::Template("<winDocuments>/My Games/Game".into()));
    let plan = prepare(
        &staged,
        &targets,
        &vars(),
        &info(Some(&entry_of(&staged)), &[], &[]),
        &choices,
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [
            Path::new("/win/drive_c/users/Alice/AppData/LocalLow/Co/Game/prefs.dat"),
            Path::new("/win/drive_c/users/Alice/Documents/My Games/Game/a.sav"),
            Path::new("/win/drive_c/users/Alice/Documents/My Games/Game/sub/b.sav"),
        ]
    );
    assert_eq!(
        plan.writes[2].target_file,
        file(Base::WinDocuments, "My Games/Game/sub/b.sav")
    );
    assert_eq!(
        plan.writes[2].target_root,
        file(Base::WinDocuments, "My Games/Game")
    );
    assert!(plan.cross_build);

    let skipped = prepare(
        &staged,
        &targets,
        &vars(),
        &info(Some(&entry_of(&staged)), &[], &[]),
        &choose(Choice::Skip),
    )
    .unwrap();
    assert_eq!(skipped.writes.len(), 1);
    assert_eq!(
        skipped.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [SkipReason::SkippedByUser; 2]
    );

    // A saved skip needs no choice.
    let remembered = prepare(
        &staged,
        &targets,
        &vars(),
        &info(
            Some(&entry_of(&staged)),
            &[],
            &["<xdgData>/Game".to_owned()],
        ),
        &[],
    )
    .unwrap();
    assert_eq!(remembered.writes.len(), 1);
    assert_eq!(
        remembered
            .skipped
            .iter()
            .map(|s| s.reason)
            .collect::<Vec<_>>(),
        [SkipReason::SkippedByUser; 2]
    );

    let real_path = choose(Choice::Template("/home/me/Game".into()));
    assert!(matches!(
        prepare(
            &staged,
            &targets,
            &vars(),
            &info(Some(&entry_of(&staged)), &[], &[]),
            &real_path
        ),
        Err(PrepareError::Unclassifiable { .. })
    ));
}

#[test]
fn prepare_engine_rule_returns_plan_without_choices() {
    let staged = staged(
        &[(Base::XdgConfig, "unity3d/Co/Game")],
        &[(Base::XdgConfig, "unity3d/Co/Game/save.dat")],
    );
    let plan = prepare(
        &staged,
        &win_only(native_win()),
        &vars(),
        &info(Some(&entry_of(&staged)), &[], &[]),
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [Path::new(
            "/win/drive_c/users/Alice/AppData/LocalLow/Co/Game/save.dat"
        )]
    );
    assert!(plan.cross_build);
}

#[test]
fn linux_save_maps_onto_windows_via_pair() {
    // Raw roots and pair sides keep `<game>`; the target uses the candidate the root matched.
    let dirs = vec!["Missing".into(), "Real Name".into()];
    let vars = GameVars::new("Title", dirs, &[], None);
    let staged = staged(
        &[(Base::XdgData, "<game>")],
        &[(Base::XdgData, "Real Name/save.dat")],
    );
    let pairs = [Pair {
        linux: "<xdgData>/<game>".into(),
        windows: "<winAppData>/<game>/Saves".into(),
    }];
    let plan = prepare(
        &staged,
        &win_only(native_win()),
        &vars,
        &info(Some(&entry_of(&staged)), &pairs, &[]),
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [Path::new(
            "/win/drive_c/users/Alice/AppData/Roaming/Real Name/Saves/save.dat"
        )]
    );
    let write = &plan.writes[0];
    assert_eq!(
        write.target_file,
        file(Base::WinAppData, "Real Name/Saves/save.dat")
    );
    assert_eq!(write.target_root, file(Base::WinAppData, "<game>/Saves"));
    assert!(plan.cross_build);
}

#[test]
fn prepare_adds_player_prefs_note_from_staged_registry() {
    let mut staged = staged(
        &[
            (Base::WinLocalAppDataLow, "Co/Game"),
            (Base::WinDocuments, "Game"),
        ],
        &[
            (Base::WinLocalAppDataLow, "Co/Game/save.dat"),
            (Base::WinDocuments, "Game/b.sav"),
        ],
    );
    let note = |staged: &StagedGame| {
        let result = prepare(
            staged,
            &linux_only(),
            &vars(),
            &info(Some(&entry_of(staged)), &[], &[]),
            &[],
        );
        let Err(PrepareError::NeedsMapping(entries)) = result else {
            panic!("expected NeedsMapping, got {result:?}");
        };
        entries[0].notes.clone()
    };
    assert!(note(&staged).is_empty());
    let mut export = RegistryExport::default();
    export
        .keys
        .insert(format!("{HKCU}\\Software\\co\\game"), Vec::new());
    staged.registry = Some(export);
    assert_eq!(note(&staged), [PLAYER_PREFS_NOTE]);
}

#[test]
fn absolute_root_restores_to_its_own_os_only() {
    let staged = staged(
        &[(Base::Absolute, "opt/game")],
        &[(Base::Absolute, "opt/game/save.dat")],
    );
    // No Linux target: the root is skipped, not offered for mapping.
    let plan = prepare(
        &staged,
        &win_only(native_win()),
        &vars(),
        &info(Some(&entry_of(&staged)), &[], &[]),
        &[],
    )
    .unwrap();
    assert!(plan.writes.is_empty());
    assert_eq!(
        plan.skipped,
        [Skipped {
            item: SkippedItem::File(Base::Absolute, "opt/game/save.dat".into()),
            // A Unix path is not a declared root on Windows.
            reason: if cfg!(unix) {
                SkipReason::NoTarget
            } else {
                SkipReason::UndeclaredRoot
            },
        }]
    );
}

#[test]
fn mapped_root_with_os_user_name_covers_file() {
    let staged = staged(
        &[(Base::WinDocuments, "<osUserName>/Game")],
        &[(Base::WinDocuments, "steamuser/Game/s.sav")],
    );
    let result = prepare(
        &staged,
        &linux_only(),
        &vars(),
        &info(Some(&entry_of(&staged)), &[], &[]),
        &[],
    );
    let Err(PrepareError::NeedsMapping(entries)) = result else {
        panic!("expected NeedsMapping, got {result:?}");
    };
    assert_eq!(entries.iter().map(|e| e.files).collect::<Vec<_>>(), [1]);

    let choices = [(
        root(Base::WinDocuments, "<osUserName>/Game"),
        Choice::Template("<xdgData>/<osUserName>/Game".into()),
    )];
    let plan = prepare(
        &staged,
        &linux_only(),
        &vars(),
        &info(Some(&entry_of(&staged)), &[], &[]),
        &choices,
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [Path::new("/home/me/.local/share/me/Game/s.sav")]
    );
}

#[test]
fn unity_note_kept_when_mapped_automatically() {
    let mut staged = staged(
        &[(Base::WinLocalAppDataLow, "Co/Game")],
        &[(Base::WinLocalAppDataLow, "Co/Game/save.dat")],
    );
    let mut export = RegistryExport::default();
    export
        .keys
        .insert(format!("{HKCU}\\Software\\Co\\Game"), Vec::new());
    staged.registry = Some(export);
    let plan = prepare(
        &staged,
        &linux_only(),
        &vars(),
        &info(Some(&entry_of(&staged)), &[], &[]),
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [Path::new("/home/me/.config/unity3d/Co/Game/save.dat")]
    );
    assert_eq!(plan.notes, [PLAYER_PREFS_NOTE]);
}

#[test]
fn crafted_root_cannot_read_outside_the_staged_files() {
    // A root holding `..` would match a tail that leaves `files/<Base>`; even a declared one is
    // refused, so the staged source never leaves `files/<Base>` either.
    let staged = staged(&[(Base::XdgData, "x/..")], &[(Base::XdgData, "x/../save")]);
    let mapping = ResolvedMapping {
        root: (Base::XdgData, "x/..".into()),
        target: Some((Base::WinAppData, "Game".into())),
    };
    let plan = plan(
        &staged,
        &win_only(native_win()),
        &vars(),
        Some(&entry_of(&staged)),
        &[],
        &[mapping],
    )
    .unwrap();
    assert!(plan.writes.is_empty());
    assert_eq!(
        plan.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [SkipReason::UndeclaredRoot]
    );
}

#[test]
fn store_game_id_substituted_into_root() {
    let staged = staged(
        &[(Base::WinAppData, "<storeGameId>/Saves")],
        &[(Base::WinAppData, "123/Saves/s.sav")],
    );
    let with_id = GameVars::new("Game", vec![], &[], Some("123".into()));
    let plan_id = plan(
        &staged,
        &win_only(native_win()),
        &with_id,
        Some(&entry_of(&staged)),
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan_id),
        [Path::new(
            "/win/drive_c/users/Alice/AppData/Roaming/123/Saves/s.sav"
        )]
    );
    let plan_none = plan(
        &staged,
        &win_only(native_win()),
        &vars(),
        Some(&entry_of(&staged)),
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        plan_none
            .skipped
            .iter()
            .map(|s| s.reason)
            .collect::<Vec<_>>(),
        [SkipReason::OutsideRoots]
    );
}

#[test]
fn linux_snapshot_restored_into_proton_build() {
    // On a Linux host the user picked the Windows build in a prefix, so `linux` is unset.
    let prefix = tempfile::tempdir().unwrap();
    let folders = WinFolders::for_prefix(prefix.path(), "steamuser");
    let ctx = Context::WinePrefix {
        prefix: prefix.path().into(),
        folders: folders.clone(),
    };
    let staged = staged(
        &[(Base::XdgConfig, "unity3d/Co/Game")],
        &[(Base::XdgConfig, "unity3d/Co/Game/save.dat")],
    );
    let plan = prepare(
        &staged,
        &win_only(ctx),
        &vars(),
        &info(Some(&entry_of(&staged)), &[], &[]),
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [folders.local_app_data_low.join("Co/Game/save.dat")]
    );
    assert!(plan.cross_build);
}

#[test]
fn snapshot_registry_keys_do_not_declare_themselves() {
    // `prepare` reads the staged keys for the PlayerPrefs note; they must not pass the filter.
    let mut staged = staged(
        &[(Base::XdgConfig, "unity3d/Co/Game")],
        &[(Base::XdgConfig, "unity3d/Co/Game/save.dat")],
    );
    staged.registry = Some(registry(&["Software\\Game", "Software\\Evil"]));
    let entry = declaring(&staged, &["HKEY_CURRENT_USER/Software/Game"]);
    let plan = prepare(
        &staged,
        &win_only(native_win()),
        &vars(),
        &info(Some(&entry), &[], &[]),
        &[],
    )
    .unwrap();
    assert_eq!(
        plan.registry,
        Some((registry(&["Software\\Game"]), native_win()))
    );
    assert_eq!(plan.skipped, undeclared(&["Software\\Evil"]));
}

#[test]
fn os_user_name_does_not_match_dot_dot() {
    let staged = staged(
        &[(Base::XdgData, "<osUserName>/Game")],
        &[(Base::XdgData, "../Game/s.sav")],
    );
    let plan = plan(
        &staged,
        &linux_only(),
        &vars(),
        Some(&entry_of(&staged)),
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        plan.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [SkipReason::OutsideRoots]
    );
}

#[test]
fn crafted_root_covering_home_is_skipped() {
    let staged = staged(
        &[
            (Base::Home, ""),
            (Base::Absolute, ""),
            (Base::XdgData, "Game"),
        ],
        &[
            (Base::Home, ".ssh/authorized_keys"),
            (Base::Absolute, "etc/profile.d/x.sh"),
            (Base::XdgData, "Game/a.sav"),
        ],
    );
    let entry = templates(&["<xdgData>/Game"]);
    let plan = plan(&staged, &linux_only(), &vars(), Some(&entry), &[], &[]).unwrap();
    assert_eq!(
        targets_of(&plan),
        [Path::new("/home/me/.local/share/Game/a.sav")]
    );
    assert_eq!(
        plan.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [SkipReason::UndeclaredRoot; 2]
    );
    // Without an entry no root is declared.
    let unknown = plan_of(&staged, None, &[]);
    assert!(unknown.writes.is_empty());
    assert!((unknown.skipped.iter()).all(|s| s.reason == SkipReason::UndeclaredRoot));
}

fn plan_of(staged: &StagedGame, entry: Option<&GameEntry>, pairs: &[Pair]) -> RestorePlan {
    plan(staged, &linux_only(), &vars(), entry, pairs, &[]).unwrap()
}

#[test]
fn declared_root_still_restores() {
    // Roots are the classified glob-free prefixes of the entry's templates, in either family,
    // plus the engine-rule targets of those (a pre-restore snapshot of a cross-build restore).
    let staged = staged(
        &[
            (Base::XdgData, "Game"),
            (Base::XdgConfig, "Foo"),
            (Base::WinLocalAppDataLow, "Co/Game"),
        ],
        &[
            (Base::XdgData, "Game/a.sav"),
            (Base::XdgConfig, "Foo/b.cfg"),
            (Base::WinLocalAppDataLow, "Co/Game/c.sav"),
        ],
    );
    let entry = templates(&[
        "<xdgData>/Game/*.sav",
        "<home>/.config/Foo",
        "<xdgConfig>/unity3d/Co/Game",
    ]);
    let targets = Targets {
        linux: Some(linux_folders()),
        win: Some(native_win()),
    };
    let plan = plan(&staged, &targets, &vars(), Some(&entry), &[], &[]).unwrap();
    assert_eq!(
        targets_of(&plan),
        [
            Path::new("/home/me/.local/share/Game/a.sav"),
            Path::new("/home/me/.config/Foo/b.cfg"),
            Path::new("/win/drive_c/users/Alice/AppData/LocalLow/Co/Game/c.sav"),
        ]
    );
    assert!(plan.skipped.is_empty());
}

#[test]
fn pair_side_root_is_trusted() {
    let staged = staged(
        &[(Base::WinDocuments, "My Games/Foo")],
        &[(Base::WinDocuments, "My Games/Foo/x.sav")],
    );
    let pairs = [Pair {
        linux: "<xdgData>/Foo".into(),
        windows: "<winDocuments>/My Games/Foo".into(),
    }];
    assert_eq!(
        plan_of(&staged, None, &[]).skipped[0].reason,
        SkipReason::UndeclaredRoot
    );
    let plan = prepare(
        &staged,
        &linux_only(),
        &vars(),
        &info(None, &pairs, &[]),
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [Path::new("/home/me/.local/share/Foo/x.sav")]
    );
}

#[test]
fn os_user_name_root_loses_a_tie_to_a_literal_root() {
    let staged = staged(
        &[
            (Base::WinDocuments, "Game/<osUserName>"),
            (Base::WinDocuments, "Game/Shared"),
        ],
        &[(Base::WinDocuments, "Game/Shared/x.sav")],
    );
    let plan = plan(
        &staged,
        &win_only(native_win()),
        &vars(),
        Some(&entry_of(&staged)),
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [Path::new(
            "/win/drive_c/users/Alice/Documents/Game/Shared/x.sav"
        )]
    );
}

#[test]
fn root_below_a_pair_side_is_trusted() {
    // A pre-restore snapshot of a cross-build restore records the pair side plus the subdirectory.
    let game = staged(
        &[(Base::WinDocuments, "my games/foo/sub"), (Base::Home, "")],
        &[
            (Base::WinDocuments, "my games/foo/sub/x.sav"),
            (Base::Home, ".ssh/authorized_keys"),
        ],
    );
    let pairs = [Pair {
        linux: "<xdgData>/Foo".into(),
        windows: "<winDocuments>/My Games/Foo".into(),
    }];
    let plan = plan(&game, &win_only(native_win()), &vars(), None, &pairs, &[]).unwrap();
    assert_eq!(
        targets_of(&plan),
        [Path::new(
            "/win/drive_c/users/Alice/Documents/my games/foo/sub/x.sav"
        )]
    );
    assert_eq!(
        plan.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [SkipReason::UndeclaredRoot]
    );
    // `..` below a declared prefix is still refused.
    let dot_dot = staged(
        &[(Base::XdgData, "Foo/..")],
        &[(Base::XdgData, "Foo/../x.sav")],
    );
    assert_eq!(
        plan_of(&dot_dot, None, &pairs).skipped[0].reason,
        SkipReason::UndeclaredRoot
    );
}

#[test]
fn empty_rest_prefix_only_trusts_files_matching_the_template() {
    let install = tempfile::tempdir().unwrap();
    fs::create_dir(install.path().join("Game")).unwrap();
    let roots = other_root(install.path());
    let vars = GameVars::new("Game", vec![], &roots, None);
    let game = staged(
        &[(Base::GameDir, "")],
        &[
            (Base::GameDir, "a.sav"),
            (Base::GameDir, "game.exe"),
            (Base::GameDir, "bin/steam_api64.dll"),
        ],
    );
    let entry = templates(&["<base>/*.sav"]);
    let plan = plan(&game, &linux_only(), &vars, Some(&entry), &[], &[]).unwrap();
    assert_eq!(targets_of(&plan), [install.path().join("Game/a.sav")]);
    assert_eq!(
        plan.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [SkipReason::UndeclaredRoot; 2]
    );
}

#[test]
fn empty_rest_user_folder_prefix_only_trusts_matching_files() {
    let home = staged(
        &[(Base::Home, "")],
        &[
            (Base::Home, "ACH_1.DAT"),
            (Base::Home, ".ssh/authorized_keys"),
        ],
    );
    let plan_home = plan_of(&home, Some(&templates(&["<home>/ACH_*.DAT"])), &[]);
    assert_eq!(targets_of(&plan_home), [Path::new("/home/me/ACH_1.DAT")]);
    assert_eq!(
        plan_home
            .skipped
            .iter()
            .map(|s| s.reason)
            .collect::<Vec<_>>(),
        [SkipReason::UndeclaredRoot]
    );

    // A matched directory's files are saved too; Windows bases ignore case.
    let app_data = staged(
        &[(Base::WinAppData, "")],
        &[
            (Base::WinAppData, "COM.X.game/save.dat"),
            (Base::WinAppData, "Microsoft/Windows/Start Menu/x.lnk"),
        ],
    );
    let entry = templates(&["<winAppData>/com.x.*"]);
    let plan_win = plan(
        &app_data,
        &win_only(native_win()),
        &vars(),
        Some(&entry),
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan_win),
        [Path::new(
            "/win/drive_c/users/Alice/AppData/Roaming/COM.X.game/save.dat"
        )]
    );
    assert_eq!(
        plan_win
            .skipped
            .iter()
            .map(|s| s.reason)
            .collect::<Vec<_>>(),
        [SkipReason::UndeclaredRoot]
    );
}

#[test]
fn shallow_prefix_only_trusts_files_matching_the_template() {
    // `[number]` is a glob class, as in the scan: it matches one of those letters.
    let game = staged(
        &[(Base::WinDir, "System32")],
        &[
            (Base::WinDir, "System32/Skout-Interpreter-sn"),
            (Base::WinDir, "System32/evil.dll"),
        ],
    );
    let entry = templates(&["<winDir>/System32/Skout-Interpreter-s[number]"]);
    let plan = plan(
        &game,
        &win_only(native_win()),
        &vars(),
        Some(&entry),
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        targets_of(&plan),
        [Path::new(
            "/win/drive_c/windows/System32/Skout-Interpreter-sn"
        )]
    );
    assert_eq!(
        plan.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [SkipReason::UndeclaredRoot]
    );
}

fn file_row(linux: Option<&str>, windows: Option<&str>) -> CustomFile {
    CustomFile {
        linux: linux.map(Into::into),
        windows: windows.map(Into::into),
    }
}

/// What a restore takes from a custom game holding only `files`.
fn rows_info(files: Vec<CustomFile>) -> GameInfo {
    let game = CustomGame {
        name: "Game".into(),
        files,
        entry: GameEntry::default(),
        integration: Integration::Override,
        wine_prefix: None,
        pairs: Vec::new(),
        skips: Vec::new(),
    };
    game_info("Game", &Manifest::default(), &[game])
}

#[test]
fn two_path_rows_restore_cross_build_without_a_dialog() {
    // Two rows, so the manifest-style auto-pair cannot map them.
    let game = rows_info(vec![
        file_row(
            Some("<xdgData>/g/*.sav"),
            Some("<winDocuments>/G/Saves/*.sav"),
        ),
        file_row(Some("<xdgConfig>/g/*.ini"), Some("<winAppData>/G/*.ini")),
    ]);
    let staged = staged(
        &[(Base::XdgData, "g"), (Base::XdgConfig, "g")],
        &[(Base::XdgData, "g/a.sav"), (Base::XdgConfig, "g/o.ini")],
    );
    let plan = prepare(&staged, &win_only(native_win()), &vars(), &game, &[]).unwrap();
    let mut targets = targets_of(&plan);
    targets.sort();
    assert_eq!(
        targets,
        [
            Path::new("/win/drive_c/users/Alice/AppData/Roaming/G/o.ini"),
            Path::new("/win/drive_c/users/Alice/Documents/G/Saves/a.sav"),
        ]
    );
    assert!(plan.skipped.is_empty());
}

#[test]
fn a_one_sided_row_is_mapped_as_before() {
    let game = rows_info(vec![
        file_row(Some("<xdgConfig>/unity3d/Co/G"), None),
        file_row(Some("<xdgData>/g/*.sav"), None),
    ]);
    assert_eq!(game.row_pairs, []);
    let staged = staged(
        &[(Base::XdgConfig, "unity3d/Co/G"), (Base::XdgData, "g")],
        &[
            (Base::XdgConfig, "unity3d/Co/G/prefs.dat"),
            (Base::XdgData, "g/a.sav"),
        ],
    );
    let result = prepare(&staged, &win_only(native_win()), &vars(), &game, &[]);
    let Err(PrepareError::NeedsMapping(entries)) = result else {
        panic!("expected NeedsMapping, got {result:?}");
    };
    let confidences: Vec<_> = entries.iter().map(|e| e.confidence).collect();
    assert_eq!(confidences, [Confidence::EngineRule, Confidence::Unknown]);
}

fn skip_reasons(plan: &RestorePlan) -> Vec<SkipReason> {
    plan.skipped.iter().map(|s| s.reason).collect()
}

#[test]
fn a_row_pair_does_not_trust_its_whole_windows_folder() {
    let game = rows_info(vec![file_row(
        Some("<xdgData>/g/*.sav"),
        Some("<winDir>/System32/g*.sav"),
    )]);
    let staged = staged(
        &[(Base::WinDir, "System32")],
        &[
            (Base::WinDir, "System32/gx.sav"),
            (Base::WinDir, "System32/evil.dll"),
        ],
    );
    let plan = prepare(&staged, &win_only(native_win()), &vars(), &game, &[]).unwrap();
    assert_eq!(
        targets_of(&plan),
        [Path::new("/win/drive_c/windows/System32/gx.sav")]
    );
    assert_eq!(skip_reasons(&plan), [SkipReason::UndeclaredRoot]);
}

#[test]
fn a_row_pair_does_not_trust_the_whole_install_dir() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("Install Dir")).unwrap();
    let roots = other_root(root.path());
    let vars = GameVars::new("Game", vec!["Install Dir".into()], &roots, None);
    let game = rows_info(vec![file_row(
        Some("<base>/*.sav"),
        Some("<base>/Saves/*.sav"),
    )]);
    let staged = staged(
        &[(Base::GameDir, "")],
        &[(Base::GameDir, "a.sav"), (Base::GameDir, "game.exe")],
    );
    let plan = prepare(&staged, &linux_only(), &vars, &game, &[]).unwrap();
    assert_eq!(targets_of(&plan), [root.path().join("Install Dir/a.sav")]);
    assert_eq!(skip_reasons(&plan), [SkipReason::UndeclaredRoot]);
}

#[test]
fn a_single_row_whose_sides_end_differently_asks_where_its_files_go() {
    for windows in [
        "<winDocuments>/Foo/Bar [[]x[]]",
        "<winDocuments>/S/slot*/data.sav",
    ] {
        // One Linux and one Windows template, so the auto-pair sees this row too.
        let game = rows_info(vec![file_row(Some("<xdgData>/b"), Some(windows))]);
        let staged = staged(&[(Base::XdgData, "b")], &[(Base::XdgData, "b/s.sav")]);
        let result = prepare(&staged, &win_only(native_win()), &vars(), &game, &[]);
        assert!(
            matches!(result, Err(PrepareError::NeedsMapping(_))),
            "{windows}: {result:?}"
        );
    }
}
