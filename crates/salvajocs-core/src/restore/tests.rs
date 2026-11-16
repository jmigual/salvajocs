use std::fs;
use std::path::Path;

use super::*;
use crate::crossbuild::{Confidence, PLAYER_PREFS_NOTE};
use crate::layout::{Root, RootKind, SNAPSHOT_FORMAT, WinFolders};
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
    let plan = plan(&staged, &win_only(native_win()), &vars(), &[]).unwrap();
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
    let plan = plan(&staged, &win_only(ctx), &vars(), &[]).unwrap();
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
    // A drive path restores to native Windows only.
    assert_eq!(
        plan.skipped,
        [Skipped {
            file: file(Base::WinAbsolute, "C/Games/Foo/save.dat"),
            reason: SkipReason::NoTarget,
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
    let plan = plan(&staged, &targets, &vars(), &[]).unwrap();
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
    let plan = plan(&staged, &linux_only(), &vars, &[]).unwrap();
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
    let plan = plan(&staged, &linux_only(), &vars, &[]).unwrap();
    assert!(plan.writes.is_empty());
    assert_eq!(
        plan.skipped,
        [Skipped {
            file: file(Base::GameDir, "saves/a.sav"),
            reason: SkipReason::NotInstalled,
        }]
    );
}

#[test]
fn registry_skipped_without_windows_target() {
    let mut staged = staged(&[(Base::XdgData, "Game")], &[(Base::XdgData, "Game/a")]);
    staged.registry = Some(RegistryExport::default());
    let plan_linux = plan(&staged, &linux_only(), &vars(), &[]).unwrap();
    assert_eq!(plan_linux.registry, None);
    let skipped = Skipped {
        file: None,
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
    let plan_win = plan(&staged, &targets, &vars(), &[]).unwrap();
    assert_eq!(
        plan_win.registry,
        Some((RegistryExport::default(), native_win()))
    );
    assert!(plan_win.skipped.is_empty());
}

#[test]
fn override_target_rejects_relative_path() {
    let staged = staged(&[(Base::XdgData, "Game")], &[(Base::XdgData, "Game/a")]);
    let mut plan = plan(&staged, &linux_only(), &vars(), &[]).unwrap();
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
    let direct = plan(&staged, &linux_only(), &vars(), &[]).unwrap();
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
    let mapped = plan(&staged, &win_only(native_win()), &vars(), &[mapping]).unwrap();
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
    let plan = plan(&staged, &linux_only(), &vars(), &[]).unwrap();
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
    let plan = plan(&staged, &win_only(native_win()), &vars(), &[]).unwrap();
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
    let plan = plan(&staged, &targets, &vars(), &[]).unwrap();
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
    let plan = plan(&staged, &win_only(native_win()), &vars(), &[mapping]).unwrap();
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
        plan(&staged, &win_only(native_win()), &vars(), &[]),
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
    let plan = prepare(&staged, &linux_only(), &vars(), None, &[], &[]).unwrap();
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
        None,
        &[],
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
    let result = prepare(&staged, &win_only(native_win()), &vars(), None, &[], &[]);
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
    let plan = prepare(&staged, &targets, &vars(), None, &[], &choices).unwrap();
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

    let skipped = prepare(&staged, &targets, &vars(), None, &[], &choose(Choice::Skip)).unwrap();
    assert_eq!(skipped.writes.len(), 1);
    assert_eq!(
        skipped.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [SkipReason::SkippedByUser; 2]
    );

    let real_path = choose(Choice::Template("/home/me/Game".into()));
    assert!(matches!(
        prepare(&staged, &targets, &vars(), None, &[], &real_path),
        Err(PrepareError::Unclassifiable { .. })
    ));
}

#[test]
fn prepare_engine_rule_returns_plan_without_choices() {
    let staged = staged(
        &[(Base::XdgConfig, "unity3d/Co/Game")],
        &[(Base::XdgConfig, "unity3d/Co/Game/save.dat")],
    );
    let plan = prepare(&staged, &win_only(native_win()), &vars(), None, &[], &[]).unwrap();
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
    let plan = prepare(&staged, &win_only(native_win()), &vars, None, &pairs, &[]).unwrap();
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
        let result = prepare(staged, &linux_only(), &vars(), None, &[], &[]);
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
    let plan = prepare(&staged, &win_only(native_win()), &vars(), None, &[], &[]).unwrap();
    assert!(plan.writes.is_empty());
    assert_eq!(
        plan.skipped,
        [Skipped {
            file: file(Base::Absolute, "opt/game/save.dat"),
            reason: SkipReason::NoTarget,
        }]
    );
}

#[test]
fn mapped_root_with_os_user_name_covers_file() {
    let staged = staged(
        &[(Base::WinDocuments, "<osUserName>/Game")],
        &[(Base::WinDocuments, "steamuser/Game/s.sav")],
    );
    let result = prepare(&staged, &linux_only(), &vars(), None, &[], &[]);
    let Err(PrepareError::NeedsMapping(entries)) = result else {
        panic!("expected NeedsMapping, got {result:?}");
    };
    assert_eq!(entries.iter().map(|e| e.files).collect::<Vec<_>>(), [1]);

    let choices = [(
        root(Base::WinDocuments, "<osUserName>/Game"),
        Choice::Template("<xdgData>/<osUserName>/Game".into()),
    )];
    let plan = prepare(&staged, &linux_only(), &vars(), None, &[], &choices).unwrap();
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
    let plan = prepare(&staged, &linux_only(), &vars(), None, &[], &[]).unwrap();
    assert_eq!(
        targets_of(&plan),
        [Path::new("/home/me/.config/unity3d/Co/Game/save.dat")]
    );
    assert_eq!(plan.notes, [PLAYER_PREFS_NOTE]);
}
