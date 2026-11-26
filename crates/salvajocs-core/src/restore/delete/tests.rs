use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::crossbuild::Choice;
use crate::layout::{Context, HASHES_FORMAT, Hashes, LinuxFolders, Root, RootKind, WinFolders};
use crate::manifest::Pair;
use crate::restore::tests::{entry_of, info, listed_at, staged, templates};
use crate::restore::{StagedGame, Targets, prepare};

/// The user `me` with its home folder at `home`.
fn linux(home: &Path) -> Targets {
    Targets {
        linux: Some(LinuxFolders {
            user: "me".into(),
            home: home.into(),
            xdg_data: home.join(".local/share"),
            xdg_config: home.join(".config"),
        }),
        win: None,
    }
}

fn vars() -> GameVars {
    GameVars::new("Game", vec![], &[], None)
}

/// Creates a file at `path`, with its parent directories.
fn put(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "here").unwrap();
}

fn everything() -> GameSettings {
    GameSettings::default()
}

/// What [`deletions`] finds for `listed`, planned by [`prepare`] into `targets`.
fn found(
    listed: &Listed,
    targets: &Targets,
    info: &GameInfo,
    choices: &[(SnapshotRoot, Choice)],
    today: Option<&GameSettings>,
) -> Deletions {
    let plan = prepare(listed, targets, &vars(), info, choices).unwrap();
    deletions(listed, &plan, &vars(), info, today)
}

fn paths(found: &Deletions) -> Vec<PathBuf> {
    found.files.iter().map(|file| file.target.clone()).collect()
}

fn game_staged(files: &[&str]) -> StagedGame {
    let files: Vec<(Base, &str)> = files.iter().map(|&tail| (Base::XdgData, tail)).collect();
    staged(&[(Base::XdgData, "Game")], &files)
}

#[test]
fn deletions_list_a_declared_file_the_backup_lacks() {
    let home = tempfile::tempdir().unwrap();
    let game = home.path().join(".local/share/Game");
    for name in ["a.sav", "b.sav", "sub/c.sav"] {
        put(&game.join(name));
    }
    let staged = game_staged(&["Game/a.sav"]);
    let info = info(Some(&entry_of(&staged)), &[], &[]);
    let listed = listed_at(staged, &[]);

    let found = found_any(&listed, &linux(home.path()), &info);
    let delete = |tail: &str| Delete {
        target: game.join(tail),
        target_file: (Base::XdgData, format!("Game/{tail}")),
        target_root: (Base::XdgData, "Game".into()),
    };
    assert_eq!(found.files, [delete("b.sav"), delete("sub/c.sav")]);
    assert!(found.unlisted.is_empty());
}

#[test]
fn nothing_is_deleted_without_todays_selection_or_for_a_pre_restore_snapshot() {
    let home = tempfile::tempdir().unwrap();
    put(&home.path().join(".local/share/Game/b.sav"));
    let staged = game_staged(&["Game/a.sav"]);
    let info = info(Some(&entry_of(&staged)), &[], &[]);
    let mut listed = listed_at(staged, &[]);
    let targets = linux(home.path());

    assert!(paths(&found(&listed, &targets, &info, &[], None)).is_empty());
    listed.snapshot.tags.push(PRE_RESTORE_TAG.into());
    assert!(paths(&found_any(&listed, &targets, &info)).is_empty());
}

#[test]
fn deletions_keep_a_file_the_template_glob_does_not_match() {
    let home = tempfile::tempdir().unwrap();
    let game = home.path().join(".local/share/Game");
    for name in ["a.sav", "b.sav", "notes.txt"] {
        put(&game.join(name));
    }
    let staged = game_staged(&["Game/a.sav"]);
    let info = info(Some(&templates(&["<xdgData>/Game/*.sav"])), &[], &[]);
    let listed = listed_at(staged, &[]);

    let found = found_any(&listed, &linux(home.path()), &info);
    assert_eq!(paths(&found), [game.join("b.sav")]);
}

#[test]
fn deletions_keep_a_file_the_recorded_selection_excludes() {
    let home = tempfile::tempdir().unwrap();
    let game = home.path().join(".local/share/Game");
    for name in ["b.sav", "d.sav", "sub/c.sav", "sub/keep.sav"] {
        put(&game.join(name));
    }
    let staged = game_staged(&["Game/a.sav"]);
    let info = info(Some(&entry_of(&staged)), &[], &[]);
    let mut listed = listed_at(staged, &[]);
    // `sub/keep.sav` was ticked again below the unticked folder.
    let selection = [
        ("XdgData/Game/b.sav", false),
        ("XdgData/Game/sub", false),
        ("XdgData/Game/sub/keep.sav", true),
    ];
    listed.hashes = Some(Hashes {
        format: HASHES_FORMAT,
        meta: String::new(),
        registry: None,
        files: BTreeMap::new(),
        selection: (selection.iter())
            .map(|&(key, included)| (key.to_owned(), included))
            .collect(),
    });

    let found = found_any(&listed, &linux(home.path()), &info);
    assert_eq!(
        paths(&found),
        [game.join("d.sav"), game.join("sub/keep.sav")]
    );
}

/// A Windows snapshot of `WinAppData/Game` with `Game/a.sav`, restored onto `<xdgData>/GameLinux`
/// through a saved pair.
fn mapped() -> (Listed, GameInfo) {
    let staged = staged(
        &[(Base::WinAppData, "Game")],
        &[(Base::WinAppData, "Game/a.sav")],
    );
    let pairs = [Pair {
        linux: "<xdgData>/GameLinux".into(),
        windows: "<winAppData>/Game".into(),
    }];
    let info = info(Some(&entry_of(&staged)), &pairs, &[]);
    (listed_at(staged, &[]), info)
}

#[test]
fn deletions_follow_a_mapped_root() {
    let home = tempfile::tempdir().unwrap();
    let share = home.path().join(".local/share");
    put(&share.join("GameLinux/a.sav"));
    put(&share.join("GameLinux/x.sav"));
    // The snapshot's own folder name is not where this build keeps the saves.
    put(&share.join("Game/y.sav"));
    let (listed, info) = mapped();

    let found = found_any(&listed, &linux(home.path()), &info);
    assert_eq!(
        found.files,
        [Delete {
            target: share.join("GameLinux/x.sav"),
            target_file: (Base::XdgData, "GameLinux/x.sav".into()),
            target_root: (Base::XdgData, "GameLinux".into()),
        }]
    );
}

#[test]
fn deletions_keep_a_file_todays_selection_excludes_by_either_key() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".local/share/GameLinux");
    for name in ["b.sav", "c.sav", "d.sav"] {
        put(&dir.join(name));
    }
    let (listed, info) = mapped();
    let mut today = everything();
    // The backing-up build's key, then this build's.
    today.set_path("WinAppData/Game/b.sav", false);
    today.set_path("XdgData/GameLinux/c.sav", false);

    let found = found(&listed, &linux(home.path()), &info, &[], Some(&today));
    assert_eq!(paths(&found), [dir.join("d.sav")]);
}

#[test]
fn deletions_skip_a_whole_base_root() {
    let home = tempfile::tempdir().unwrap();
    let share = home.path().join(".local/share");
    put(&share.join("a.sav"));
    put(&share.join("b.sav"));
    let staged = staged(&[(Base::XdgData, "")], &[(Base::XdgData, "a.sav")]);
    let info = info(Some(&templates(&["<xdgData>/*.sav"])), &[], &[]);
    let listed = listed_at(staged, &[]);

    let plan = prepare(&listed, &linux(home.path()), &vars(), &info, &[]).unwrap();
    assert_eq!(plan.writes.len(), 1, "the root restores: {plan:?}");
    let found = deletions(&listed, &plan, &vars(), &info, Some(&everything()));
    assert!(paths(&found).is_empty());
}

#[test]
fn deletions_skip_an_undeclared_or_too_wide_root() {
    let home = tempfile::tempdir().unwrap();
    let share = home.path().join(".local/share");
    for name in ["Game/a.sav", "Game/b.sav", "Other/x.sav"] {
        put(&share.join(name));
    }
    let staged = staged(
        &[(Base::XdgData, "Game"), (Base::XdgData, "Other")],
        &[
            (Base::XdgData, "Game/a.sav"),
            (Base::XdgData, "Other/a.sav"),
        ],
    );
    let listed = listed_at(staged, &[]);
    let targets = linux(home.path());

    // `Other` is not declared, so only `Game` deletes.
    let game = info(Some(&templates(&["<xdgData>/Game"])), &[], &[]);
    let found = found(&listed, &targets, &game, &[], Some(&everything()));
    assert_eq!(paths(&found), [share.join("Game/b.sav")]);
    // A too-wide template declares nothing.
    let wide = info(Some(&templates(&["<xdgData>/*"])), &[], &[]);
    assert!(paths(&found_any(&listed, &targets, &wide)).is_empty());
    assert!(paths(&found_any(&listed, &targets, &info(None, &[], &[]))).is_empty());
}

/// [`found`] with everything selected and no choices, for a plan that may write nothing.
fn found_any(listed: &Listed, targets: &Targets, info: &GameInfo) -> Deletions {
    found(listed, targets, info, &[], Some(&everything()))
}

#[test]
fn deletions_skip_a_root_skipped_by_choice() {
    let home = tempfile::tempdir().unwrap();
    let game = home.path().join(".local/share/Game");
    for name in ["a.sav", "b.sav", "saves/x.sav"] {
        put(&game.join(name));
    }
    let (outer, inner) = (
        (Base::WinAppData, "Game".to_owned()),
        (Base::WinAppData, "Game/saves".to_owned()),
    );
    let staged = staged(
        &[(Base::WinAppData, "Game"), (Base::WinAppData, "Game/saves")],
        &[
            (Base::WinAppData, "Game/a.sav"),
            (Base::WinAppData, "Game/saves/s.sav"),
        ],
    );
    let info = info(Some(&entry_of(&staged)), &[], &[]);
    let listed = listed_at(staged, &[]);
    let choices = [
        (outer, Choice::Template("<xdgData>/Game".into())),
        (inner, Choice::Skip),
    ];

    let found = found(
        &listed,
        &linux(home.path()),
        &info,
        &choices,
        Some(&everything()),
    );
    // `saves/x.sav` lies in the skipped root's folder.
    assert_eq!(paths(&found), [game.join("b.sav")]);
}

#[cfg(unix)]
#[test]
fn deletions_skip_symlinks_and_dirs() {
    use std::os::unix::fs::symlink;

    let home = tempfile::tempdir().unwrap();
    let game = home.path().join(".local/share/Game");
    put(&game.join("a.sav"));
    put(&home.path().join("elsewhere/x.sav"));
    fs::create_dir_all(game.join("empty")).unwrap();
    symlink(home.path().join("elsewhere/x.sav"), game.join("link.sav")).unwrap();
    symlink(home.path().join("elsewhere"), game.join("linked")).unwrap();

    let staged = game_staged(&["Game/a.sav"]);
    let info = info(Some(&entry_of(&staged)), &[], &[]);
    let listed = listed_at(staged, &[]);
    let found = found_any(&listed, &linux(home.path()), &info);
    assert!(found.files.is_empty(), "{found:?}");
}

#[cfg(unix)]
#[test]
fn deletions_in_a_prefix_keep_a_case_variant_of_a_write() {
    let prefix = tempfile::tempdir().unwrap();
    let folders = WinFolders::for_prefix(prefix.path(), "steamuser");
    let foo = folders.app_data.join("Foo");
    for name in ["save.dat", "SAVE.DAT", "other.dat"] {
        put(&foo.join(name));
    }
    let ctx = Context::WinePrefix {
        prefix: prefix.path().into(),
        folders,
    };
    let targets = Targets {
        linux: None,
        win: Some(ctx),
    };
    let staged = staged(
        &[(Base::WinAppData, "Foo")],
        &[(Base::WinAppData, "Foo/Save.dat")],
    );
    let info = info(Some(&entry_of(&staged)), &[], &[]);
    let listed = listed_at(staged, &[]);

    let found = found_any(&listed, &targets, &info);
    assert_eq!(paths(&found), [foo.join("other.dat")]);
}

#[test]
fn a_root_that_is_one_file_lists_nothing() {
    let home = tempfile::tempdir().unwrap();
    let share = home.path().join(".local/share");
    put(&share.join("Game/one.sav"));
    let staged = staged(
        &[(Base::XdgData, "Game/one.sav")],
        &[(Base::XdgData, "Game/one.sav")],
    );
    let info = info(Some(&entry_of(&staged)), &[], &[]);
    let listed = listed_at(staged, &[]);

    let found = found_any(&listed, &linux(home.path()), &info);
    assert!(found.files.is_empty(), "{found:?}");
    assert!(found.unlisted.is_empty(), "{found:?}");
}

#[cfg(unix)]
#[test]
fn an_unreadable_root_dir_deletes_nothing_there() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().unwrap();
    let share = home.path().join(".local/share");
    for name in ["Game/b.sav", "Game/locked/c.sav", "Other/x.sav"] {
        put(&share.join(name));
    }
    let locked = share.join("Game/locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&locked).is_ok() {
        // Root reads it anyway.
        return;
    }
    let staged = staged(
        &[(Base::XdgData, "Game"), (Base::XdgData, "Other")],
        &[
            (Base::XdgData, "Game/a.sav"),
            (Base::XdgData, "Other/a.sav"),
        ],
    );
    let info = info(Some(&entry_of(&staged)), &[], &[]);
    let listed = listed_at(staged, &[]);

    let found = found_any(&listed, &linux(home.path()), &info);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(paths(&found), [share.join("Other/x.sav")]);
    let unlisted: Vec<&Path> = found
        .unlisted
        .iter()
        .map(|dir| dir.path.as_path())
        .collect();
    assert_eq!(unlisted, [share.join("Game")]);
}

#[test]
fn every_deletion_plans_back_onto_its_own_path() {
    let home = tempfile::tempdir().unwrap();
    let share = home.path().join(".local/share");
    let dot_game = home.path().join(".game");
    for name in ["a.sav", "b.sav", "deep/er/c.sav", "Mixed Case.sav"] {
        put(&share.join("GameLinux").join(name));
        put(&dot_game.join(name));
    }
    // Two Windows roots restored onto Linux, one through a pair and one through a choice.
    let roots = [
        (Base::WinAppData, "Game"),
        (Base::WinDocuments, "My Games/Game"),
    ];
    let mut files = vec![
        (Base::WinAppData, "Game/a.sav".to_owned()),
        (Base::WinDocuments, "My Games/Game/a.sav".to_owned()),
    ];
    let pairs = [Pair {
        linux: "<xdgData>/GameLinux".into(),
        windows: "<winAppData>/Game".into(),
    }];
    let choices = [(
        (Base::WinDocuments, "My Games/Game".to_owned()),
        Choice::Template("<home>/.game".into()),
    )];
    let game = staged(&roots, &[]);
    let info = info(Some(&entry_of(&game)), &pairs, &[]);
    let with = |files: &[(Base, String)]| {
        let files: Vec<(Base, &str)> = (files.iter())
            .map(|(base, tail)| (*base, tail.as_str()))
            .collect();
        listed_at(staged(&roots, &files), &[])
    };
    let targets = linux(home.path());

    let found = found(
        &with(&files),
        &targets,
        &info,
        &choices,
        Some(&everything()),
    );
    assert_eq!(found.files.len(), 6, "{found:?}");
    // Each deletion as the staged file of its root, by where its root restores to.
    let sides = [
        (&share.join("GameLinux"), Base::WinAppData, "Game"),
        (&dot_game, Base::WinDocuments, "My Games/Game"),
    ];
    for delete in &found.files {
        let (below, base, rest) = (sides.iter())
            .find_map(|(dir, base, rest)| Some((delete.target.strip_prefix(dir).ok()?, base, rest)))
            .unwrap();
        let below: Vec<&str> = below.iter().map(|part| part.to_str().unwrap()).collect();
        files.push((*base, format!("{rest}/{}", below.join("/"))));
    }
    let plan = prepare(&with(&files), &targets, &vars(), &info, &choices).unwrap();
    for delete in &found.files {
        assert!(
            plan.writes
                .iter()
                .any(|write| write.target == delete.target),
            "{delete:?} plans elsewhere: {plan:?}"
        );
    }
}

/// Native Windows folders of the user `Alice` below `dir`, as plain data on any host.
fn native(dir: &Path) -> (Targets, WinFolders) {
    let folders = WinFolders::for_prefix(dir, "Alice");
    let targets = Targets {
        linux: None,
        win: Some(Context::WindowsNative(folders.clone())),
    };
    (targets, folders)
}

#[test]
fn deletions_keep_a_file_the_backup_holds_whatever_the_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (targets, folders) = native(dir.path());
    let foo = folders.app_data.join("Foo");
    for name in ["save.dat", "other.dat"] {
        put(&foo.join(name));
    }
    let staged = staged(
        &[(Base::WinAppData, "Foo")],
        &[(Base::WinAppData, "Foo/Save.dat")],
    );
    let info = info(Some(&entry_of(&staged)), &[], &[]);
    let listed = listed_at(staged, &[]);
    // As if the write were unticked or overridden elsewhere.
    let mut plan = prepare(&listed, &targets, &vars(), &info, &[]).unwrap();
    plan.writes.clear();

    let found = deletions(&listed, &plan, &vars(), &info, Some(&everything()));
    assert_eq!(paths(&found), [foo.join("other.dat")]);
}

#[test]
fn whole_base_counts_a_bare_drive() {
    assert!(whole_base(&(Base::XdgData, String::new())));
    assert!(whole_base(&(Base::WinAbsolute, "C:".into())));
    assert!(whole_base(&(Base::WinAbsolute, "C".into())));
    assert!(!whole_base(&(Base::WinAbsolute, "C:/Game".into())));
    assert!(!whole_base(&(Base::XdgData, "Game".into())));
}

#[test]
fn deletions_follow_an_os_user_name_root_onto_this_user() {
    let dir = tempfile::tempdir().unwrap();
    let (targets, folders) = native(dir.path());
    let game = folders.documents.join("Alice/Game");
    for name in ["s.sav", "x.sav"] {
        put(&game.join(name));
    }
    let staged = staged(
        &[(Base::WinDocuments, "<osUserName>/Game")],
        &[(Base::WinDocuments, "steamuser/Game/s.sav")],
    );
    let info = info(Some(&entry_of(&staged)), &[], &[]);
    let listed = listed_at(staged, &[]);

    let found = found(&listed, &targets, &info, &[], Some(&everything()));
    assert_eq!(
        found.files,
        [Delete {
            target: game.join("x.sav"),
            target_file: (Base::WinDocuments, "Alice/Game/x.sav".into()),
            target_root: (Base::WinDocuments, "<osUserName>/Game".into()),
        }]
    );
}

#[test]
fn deletions_follow_the_game_name_the_backup_matched() {
    let home = tempfile::tempdir().unwrap();
    let share = home.path().join(".local/share");
    for name in [
        "Real Name/save.dat",
        "Real Name/old.dat",
        "Missing/m.dat",
        "Title/t.dat",
    ] {
        put(&share.join(name));
    }
    let vars = GameVars::new(
        "Title",
        vec!["Missing".into(), "Real Name".into()],
        &[],
        None,
    );
    let staged = staged(
        &[(Base::XdgData, "<game>")],
        &[(Base::XdgData, "Real Name/save.dat")],
    );
    let info = info(Some(&entry_of(&staged)), &[], &[]);
    let listed = listed_at(staged, &[]);

    let plan = prepare(&listed, &linux(home.path()), &vars, &info, &[]).unwrap();
    let found = deletions(&listed, &plan, &vars, &info, Some(&everything()));
    assert_eq!(paths(&found), [share.join("Real Name/old.dat")]);
}

#[test]
fn a_recorded_selection_excludes_a_file_in_another_case_below_a_windows_base() {
    let dir = tempfile::tempdir().unwrap();
    let (targets, folders) = native(dir.path());
    let foo = folders.app_data.join("Foo");
    for name in ["b.sav", "c.sav"] {
        put(&foo.join(name));
    }
    let staged = staged(
        &[(Base::WinAppData, "Foo")],
        &[(Base::WinAppData, "Foo/a.sav")],
    );
    let info = info(Some(&entry_of(&staged)), &[], &[]);
    let mut listed = listed_at(staged, &[]);
    listed.hashes = Some(Hashes {
        format: HASHES_FORMAT,
        meta: String::new(),
        registry: None,
        files: BTreeMap::new(),
        selection: BTreeMap::from([("WinAppData/FOO/B.SAV".to_owned(), false)]),
    });

    let found = found(&listed, &targets, &info, &[], Some(&everything()));
    assert_eq!(paths(&found), [foo.join("c.sav")]);
}

#[test]
fn a_game_not_backed_up_still_deletes() {
    // The user's decision of 2026-11-25: unticking the whole game only stops its backups.
    let home = tempfile::tempdir().unwrap();
    let game = home.path().join(".local/share/Game");
    put(&game.join("b.sav"));
    let staged = game_staged(&["Game/a.sav"]);
    let info = info(Some(&entry_of(&staged)), &[], &[]);
    let listed = listed_at(staged, &[]);
    let mut today = everything();
    today.skip = true;

    let found = found(&listed, &linux(home.path()), &info, &[], Some(&today));
    assert_eq!(paths(&found), [game.join("b.sav")]);
}

#[test]
fn deletions_keep_a_game_dir_file_the_backup_holds_in_another_case() {
    let install = tempfile::tempdir().unwrap();
    let saves = install.path().join("Game/saves");
    for name in ["slot.sav", "other.sav"] {
        put(&saves.join(name));
    }
    let roots = [Root {
        path: install.path().into(),
        kind: RootKind::Other,
    }];
    let vars = GameVars::new("Game", vec![], &roots, None);
    let staged = staged(
        &[(Base::GameDir, "saves")],
        &[(Base::GameDir, "saves/Slot.sav")],
    );
    let info = info(Some(&entry_of(&staged)), &[], &[]);
    let listed = listed_at(staged, &[]);
    // As if the write were unticked, so only the backup's copy speaks for `slot.sav`.
    let mut plan = prepare(&listed, &linux(install.path()), &vars, &info, &[]).unwrap();
    plan.writes.clear();

    let found = deletions(&listed, &plan, &vars, &info, Some(&everything()));
    assert_eq!(paths(&found), [saves.join("other.sav")]);
}

#[test]
fn lowered_equates_every_spelling_of_one_path() {
    let path = Path::new("/home/me/Game/old.sav");
    for spelling in [
        "/home/me/Game/OLD.sav",
        "/home/me/Game/./old.sav",
        "/home/me/Game//old.sav",
    ] {
        assert_eq!(lowered(Path::new(spelling)), lowered(path), "{spelling}");
    }
    assert_ne!(lowered(Path::new("/home/me/Game/new.sav")), lowered(path));
    #[cfg(windows)]
    assert_eq!(
        lowered(Path::new(r"C:\Users\me/AppData/Game\old.sav")),
        lowered(Path::new(r"c:\users\me\AppData\Game\old.sav"))
    );
}
