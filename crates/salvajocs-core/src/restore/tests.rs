use std::fs;
use std::path::Path;

use super::*;
use crate::layout::{RootKind, SNAPSHOT_FORMAT, WinFolders};

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
    GameVars::new("Game", vec![], &[], None, "me".into())
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
    let plan = plan(&staged, &win_only(native_win()), &[], &vars(), &[]).unwrap();
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
        &[(Base::WinLocalAppDataLow, "Co/Game")],
        &[
            (Base::WinLocalAppDataLow, "Co/Game/save.dat"),
            (Base::WinAbsolute, "C/Games/Foo/save.dat"),
        ],
    );
    let ctx = Context::WinePrefix {
        prefix: prefix.path().into(),
        folders: folders.clone(),
    };
    let plan = plan(&staged, &win_only(ctx), &[], &vars(), &[]).unwrap();
    // The existing `co` is reused instead of creating `Co` next to it (ADR-0005).
    assert_eq!(
        targets_of(&plan),
        [folders.local_app_data_low.join("co/Game/save.dat")]
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
    let plan = plan(&staged, &targets, &[], &vars(), &[]).unwrap();
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
    let vars = GameVars::new(
        "Title",
        vec!["Install Dir".into()],
        &roots,
        None,
        "me".into(),
    );
    let staged = staged(
        &[(Base::GameDir, "saves")],
        &[(Base::GameDir, "saves/a.sav")],
    );
    let plan = plan(&staged, &linux_only(), &roots, &vars, &[]).unwrap();
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
    let vars = GameVars::new(
        "Title",
        vec!["Install Dir".into()],
        &roots,
        None,
        "me".into(),
    );
    let staged = staged(
        &[(Base::GameDir, "saves")],
        &[(Base::GameDir, "saves/a.sav")],
    );
    let plan = plan(&staged, &linux_only(), &roots, &vars, &[]).unwrap();
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
    let plan_linux = plan(&staged, &linux_only(), &[], &vars(), &[]).unwrap();
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
    let plan_win = plan(&staged, &targets, &[], &vars(), &[]).unwrap();
    assert_eq!(
        plan_win.registry,
        Some((RegistryExport::default(), native_win()))
    );
    assert!(plan_win.skipped.is_empty());
}

#[test]
fn override_target_rejects_relative_path() {
    let staged = staged(&[(Base::XdgData, "Game")], &[(Base::XdgData, "Game/a")]);
    let mut plan = plan(&staged, &linux_only(), &[], &vars(), &[]).unwrap();
    let before = plan.clone();
    assert_eq!(
        plan.override_target(0, "elsewhere/a".into()),
        Err(OverrideError::RelativePath)
    );
    assert_eq!(
        plan.override_target(1, "/elsewhere/a".into()),
        Err(OverrideError::NoSuchWrite(1))
    );
    assert_eq!(plan, before);

    plan.override_target(0, "/elsewhere/a".into()).unwrap();
    let write = &plan.writes[0];
    assert_eq!(write.target, Path::new("/elsewhere/a"));
    assert_eq!(write.target_file, None);
    assert_eq!(write.target_root, None);
}

#[test]
fn tail_leaving_its_base_is_skipped() {
    let staged = staged(
        &[(Base::XdgData, "Game")],
        &[
            (Base::XdgData, "../escape"),
            (Base::XdgData, "Game/../../x"),
        ],
    );
    let direct = plan(&staged, &linux_only(), &[], &vars(), &[]).unwrap();
    assert!(direct.writes.is_empty());
    assert!(
        direct
            .skipped
            .iter()
            .all(|s| s.reason == SkipReason::UnsafePath)
    );
    assert_eq!(direct.skipped.len(), 2);

    // Through a mapping, the tail below the root is checked against the target base.
    let mapping = ResolvedMapping {
        root: (Base::XdgData, "Game".into()),
        target: Some((Base::WinAppData, "Game".into())),
    };
    let mapped = plan(&staged, &win_only(native_win()), &[], &vars(), &[mapping]).unwrap();
    assert!(mapped.writes.is_empty());
    assert_eq!(
        mapped.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [SkipReason::NoTarget, SkipReason::UnsafePath]
    );
}

#[test]
fn unmapped_root_without_target_is_an_error() {
    let staged = staged(&[(Base::XdgData, "Game")], &[(Base::XdgData, "Game/a")]);
    assert_eq!(
        plan(&staged, &win_only(native_win()), &[], &vars(), &[]),
        Err(PlanError::Unmapped {
            root: (Base::XdgData, "Game".into()),
        })
    );
}
