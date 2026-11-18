use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use super::*;
use crate::layout::Base;
use crate::registry::{HKCU, RegData, RegValue};
use crate::scan::FoundFile;
use crate::storage::{Compression, RepoLocation};

/// Writes `contents` to `path`, creating its parent directories.
fn write(path: &Path, contents: &str) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
    path.to_path_buf()
}

fn found(base: Base, tail: &str, real: &Path) -> FoundFile {
    FoundFile {
        base,
        tail: tail.into(),
        real: real.to_path_buf(),
        size: fs::metadata(real).unwrap().len(),
    }
}

fn game(title: &str, files: Vec<FoundFile>) -> GameScan {
    GameScan {
        title: title.into(),
        files,
        roots: vec![],
        registry_keys: vec![],
        registry_source: None,
        prefixes: Vec::new(),
        unreadable: vec![],
    }
}

/// An export of `HKCU\Software\Foo` holding one string value `v`.
fn registry(value: &str) -> RegistryExport {
    let mut export = RegistryExport::default();
    export.keys.insert(
        format!("{HKCU}\\Software\\Foo"),
        vec![RegValue {
            name: "v".into(),
            data: RegData::Sz(value.into()),
        }],
    );
    export
}

fn mtime(path: &Path) -> SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

fn set_mtime(path: &Path, time: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

#[test]
fn slug_is_stable() {
    // Hashes checked against an independent FNV-1a 32 (`foobar` is a published test vector).
    for (title, expected) in [
        ("foobar", "foobar-bf9cf968"),
        ("Hollow Knight", "hollow-knight-827f414b"),
        (
            "The Witcher 3: Wild Hunt",
            "the-witcher-3-wild-hunt-0ec1fc28",
        ),
        ("Title, with comma: ünï", "title-with-comma-n-883363e5"),
        ("東方", "035ad832"),
        (
            "A Very Long Game Title That Goes On And On Beyond Forty",
            "a-very-long-game-title-that-goes-on-and-5ccc6819",
        ),
        (
            "Abcdefghijklmnopqrstuvwxyz abcdefghijkl nop",
            "abcdefghijklmnopqrstuvwxyz-abcdefghijkl-9e646b8f",
        ),
    ] {
        assert_eq!(slug(title), expected, "{title}");
    }
}

#[test]
fn stage_layout() {
    let tmp = tempfile::tempdir().unwrap();
    let save = write(&tmp.path().join("src/Roaming/Foo/save.dat"), "save");
    let ini = write(&tmp.path().join("src/config/Foo/settings.ini"), "ini");
    let mut scan = game(
        "Foo: The Game",
        vec![
            found(Base::WinAppData, "Foo/save.dat", &save),
            found(Base::XdgConfig, "Foo/settings.ini", &ini),
        ],
    );
    scan.roots = vec![
        (Base::WinAppData, "Foo".into()),
        (Base::XdgConfig, "Foo".into()),
    ];
    let stage_dir = tmp.path().join("stage/foo");
    stage(&scan, &registry("a"), &stage_dir).unwrap();

    let staged_save = stage_dir.join("files/WinAppData/Foo/save.dat");
    assert_eq!(fs::read_to_string(&staged_save).unwrap(), "save");
    assert_eq!(mtime(&staged_save), mtime(&save));
    let staged_ini = stage_dir.join("files/XdgConfig/Foo/settings.ini");
    assert_eq!(fs::read_to_string(&staged_ini).unwrap(), "ini");
    let meta: SnapshotMeta =
        serde_json::from_slice(&fs::read(stage_dir.join(META_FILE)).unwrap()).unwrap();
    assert_eq!(
        meta,
        SnapshotMeta {
            format: SNAPSHOT_FORMAT,
            title: scan.title.clone(),
            roots: scan.roots.clone(),
        }
    );
    let staged_registry: RegistryExport =
        serde_json::from_slice(&fs::read(stage_dir.join(REGISTRY_FILE)).unwrap()).unwrap();
    assert_eq!(staged_registry, registry("a"));

    let empty_dir = tmp.path().join("stage/bar");
    stage(&game("Bar", vec![]), &RegistryExport::default(), &empty_dir).unwrap();
    assert!(empty_dir.join(META_FILE).is_file());
    assert!(!empty_dir.join(REGISTRY_FILE).exists());
}

#[test]
fn restage_unchanged_keeps_inode_and_mtime() {
    let tmp = tempfile::tempdir().unwrap();
    let save = write(&tmp.path().join("src/save.dat"), "save");
    let scan = game("Foo", vec![found(Base::WinAppData, "Foo/save.dat", &save)]);
    let stage_dir = tmp.path().join("stage");
    stage(&scan, &registry("a"), &stage_dir).unwrap();
    let staged = stage_dir.join("files/WinAppData/Foo/save.dat");
    #[cfg(unix)]
    let inode = std::os::unix::fs::MetadataExt::ino(&fs::metadata(&staged).unwrap());

    // Same size and mtime with other bytes: only a copy would bring "save" back.
    write(&staged, "SAVE");
    set_mtime(&staged, mtime(&save));
    // An old mtime on the metadata files: any rewrite would move it to now.
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    set_mtime(&stage_dir.join(META_FILE), old);
    set_mtime(&stage_dir.join(REGISTRY_FILE), old);
    stage(&scan, &registry("a"), &stage_dir).unwrap();

    assert_eq!(fs::read_to_string(&staged).unwrap(), "SAVE");
    assert_eq!(mtime(&staged), mtime(&save));
    #[cfg(unix)]
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&fs::metadata(&staged).unwrap()),
        inode
    );
    assert_eq!(mtime(&stage_dir.join(META_FILE)), old);
    assert_eq!(mtime(&stage_dir.join(REGISTRY_FILE)), old);

    // The same mtime with another size is a change.
    write(&staged, "sav");
    set_mtime(&staged, mtime(&save));
    stage(&scan, &registry("a"), &stage_dir).unwrap();
    assert_eq!(fs::read_to_string(&staged).unwrap(), "save");

    // A new mtime alone, same size, is a change.
    let touched = mtime(&save) + Duration::from_secs(10);
    set_mtime(&save, touched);
    stage(&scan, &registry("a"), &stage_dir).unwrap();
    assert_eq!(fs::read_to_string(&staged).unwrap(), "save");
    assert_eq!(mtime(&staged), touched);
}

#[test]
fn restage_removes_deleted_source() {
    let tmp = tempfile::tempdir().unwrap();
    let kept = write(&tmp.path().join("src/a.sav"), "a");
    let gone = write(&tmp.path().join("src/sub/b.sav"), "b");
    let stage_dir = tmp.path().join("stage");
    let both = game(
        "Foo",
        vec![
            found(Base::WinAppData, "Foo/a.sav", &kept),
            found(Base::WinAppData, "Foo/sub/b.sav", &gone),
        ],
    );
    stage(&both, &registry("a"), &stage_dir).unwrap();
    assert!(stage_dir.join("files/WinAppData/Foo/sub/b.sav").is_file());

    fs::remove_file(&gone).unwrap();
    let one = game("Foo", vec![found(Base::WinAppData, "Foo/a.sav", &kept)]);
    stage(&one, &RegistryExport::default(), &stage_dir).unwrap();

    let foo = stage_dir.join("files/WinAppData/Foo");
    assert_eq!(fs::read_to_string(foo.join("a.sav")).unwrap(), "a");
    assert!(!foo.join("sub").exists());
    assert!(!stage_dir.join(REGISTRY_FILE).exists());
    assert_eq!(fs::read_to_string(&kept).unwrap(), "a");
}

#[test]
fn restage_keeps_copies_when_source_unreadable() {
    let tmp = tempfile::tempdir().unwrap();
    let changed = write(&tmp.path().join("src/a.sav"), "a");
    let hidden = write(&tmp.path().join("src/sub/b.sav"), "b");
    let stage_dir = tmp.path().join("stage");
    let both = game(
        "Foo",
        vec![
            found(Base::WinAppData, "Foo/a.sav", &changed),
            found(Base::WinAppData, "Foo/sub/b.sav", &hidden),
        ],
    );
    stage(&both, &registry("a"), &stage_dir).unwrap();

    write(&changed, "a, longer");
    let mut partial = game("Foo", vec![found(Base::WinAppData, "Foo/a.sav", &changed)]);
    partial.unreadable = vec![tmp.path().join("src/sub")];
    stage(&partial, &RegistryExport::default(), &stage_dir).unwrap();

    let foo = stage_dir.join("files/WinAppData/Foo");
    assert_eq!(fs::read_to_string(foo.join("a.sav")).unwrap(), "a, longer");
    assert_eq!(fs::read_to_string(foo.join("sub/b.sav")).unwrap(), "b");
    assert!(stage_dir.join(REGISTRY_FILE).is_file());

    // The same scan without the read error removes them.
    partial.unreadable.clear();
    stage(&partial, &RegistryExport::default(), &stage_dir).unwrap();
    assert!(!foo.join("sub").exists());
    assert!(!stage_dir.join(REGISTRY_FILE).exists());
}

#[test]
fn restage_skips_file_deleted_since_scan() {
    let tmp = tempfile::tempdir().unwrap();
    let kept = write(&tmp.path().join("src/a.sav"), "a");
    let gone = write(&tmp.path().join("src/sub/b.sav"), "b");
    let mut scan = game(
        "Foo",
        vec![
            found(Base::WinAppData, "Foo/a.sav", &kept),
            found(Base::WinAppData, "Foo/sub/b.sav", &gone),
        ],
    );
    let stage_dir = tmp.path().join("stage");
    let foo = stage_dir.join("files/WinAppData/Foo");
    fs::remove_file(&gone).unwrap();
    stage(&scan, &RegistryExport::default(), &stage_dir).unwrap();
    assert!(foo.join("a.sav").is_file());
    assert!(!foo.join("sub").exists());

    // With a read error the copy staged earlier is kept.
    write(&gone, "b");
    stage(&scan, &RegistryExport::default(), &stage_dir).unwrap();
    fs::remove_file(&gone).unwrap();
    scan.unreadable = vec![tmp.path().join("other")];
    stage(&scan, &RegistryExport::default(), &stage_dir).unwrap();
    assert_eq!(fs::read_to_string(foo.join("sub/b.sav")).unwrap(), "b");

    // Without one it is removed like any source that is gone.
    scan.unreadable.clear();
    stage(&scan, &RegistryExport::default(), &stage_dir).unwrap();
    assert!(!foo.join("sub").exists());
    assert_eq!(fs::read_to_string(foo.join("a.sav")).unwrap(), "a");
}

#[cfg(unix)]
#[test]
fn restage_replaces_a_link_at_the_destination() {
    let tmp = tempfile::tempdir().unwrap();
    let save = write(&tmp.path().join("src/save.dat"), "save");
    let scan = game("Foo", vec![found(Base::WinAppData, "Foo/save.dat", &save)]);
    let stage_dir = tmp.path().join("stage");
    stage(&scan, &RegistryExport::default(), &stage_dir).unwrap();
    let staged = stage_dir.join("files/WinAppData/Foo/save.dat");
    let outside = write(&tmp.path().join("outside.txt"), "outside");
    fs::remove_file(&staged).unwrap();
    std::os::unix::fs::symlink(&outside, &staged).unwrap();

    write(&save, "changed");
    stage(&scan, &RegistryExport::default(), &stage_dir).unwrap();
    assert_eq!(fs::read_to_string(&outside).unwrap(), "outside");
    assert!(fs::symlink_metadata(&staged).unwrap().is_file());
    assert_eq!(fs::read_to_string(&staged).unwrap(), "changed");
}

#[test]
fn stage_rejects_unsafe_tail() {
    let tmp = tempfile::tempdir().unwrap();
    let save = write(&tmp.path().join("src/save.dat"), "save");
    let stage_dir = tmp.path().join("stage");
    let scan = game("Foo", vec![found(Base::WinAppData, "Foo/save.dat", &save)]);
    stage(&scan, &RegistryExport::default(), &stage_dir).unwrap();

    for tail in ["../x", "Foo/../../x", "../../../x"] {
        // Without the good file: had the tail been checked after the removal pass, it would be gone.
        let bad = game("Foo", vec![found(Base::WinAppData, tail, &save)]);
        assert!(matches!(
            stage(&bad, &RegistryExport::default(), &stage_dir),
            Err(BackupError::UnsafeTail(_))
        ));
    }
    assert!(stage_dir.join("files/WinAppData/Foo/save.dat").is_file());
    assert!(!stage_dir.join("files/x").exists());
    assert!(!tmp.path().join("x").exists());
}

#[cfg(unix)]
#[test]
fn restage_removes_a_link_without_following_it() {
    let tmp = tempfile::tempdir().unwrap();
    let save = write(&tmp.path().join("src/save.dat"), "save");
    let scan = game("Foo", vec![found(Base::WinAppData, "Foo/save.dat", &save)]);
    let stage_dir = tmp.path().join("stage");
    stage(&scan, &RegistryExport::default(), &stage_dir).unwrap();
    let target = write(&tmp.path().join("elsewhere/keep.txt"), "keep");
    let link = stage_dir.join("files/WinAppData/link");
    std::os::unix::fs::symlink(target.parent().unwrap(), &link).unwrap();

    stage(&scan, &RegistryExport::default(), &stage_dir).unwrap();
    assert!(fs::symlink_metadata(&link).is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
}

fn staged_meta(stage_dir: &Path) -> SnapshotMeta {
    serde_json::from_slice(&fs::read(stage_dir.join(META_FILE)).unwrap()).unwrap()
}

#[test]
fn restage_keeps_recorded_roots_when_source_unreadable() {
    let tmp = tempfile::tempdir().unwrap();
    let stage_dir = tmp.path().join("stage");
    let a = (Base::WinAppData, "A".to_string());
    let b = (Base::WinDocuments, "B".to_string());
    let mut scan = game("Foo", vec![]);
    scan.roots = vec![a.clone(), b.clone()];
    stage(&scan, &RegistryExport::default(), &stage_dir).unwrap();

    // B's files were all unreadable, so the scan did not record B.
    scan.roots = vec![a.clone()];
    scan.unreadable = vec![tmp.path().join("B")];
    stage(&scan, &RegistryExport::default(), &stage_dir).unwrap();
    assert_eq!(staged_meta(&stage_dir).roots, [a.clone(), b.clone()]);
    let bytes = fs::read(stage_dir.join(META_FILE)).unwrap();
    stage(&scan, &RegistryExport::default(), &stage_dir).unwrap();
    assert_eq!(fs::read(stage_dir.join(META_FILE)).unwrap(), bytes);

    scan.unreadable.clear();
    stage(&scan, &RegistryExport::default(), &stage_dir).unwrap();
    assert_eq!(staged_meta(&stage_dir).roots, [a]);

    write(&stage_dir.join(META_FILE), "not json");
    scan.unreadable = vec![tmp.path().join("B")];
    assert!(matches!(
        stage(&scan, &RegistryExport::default(), &stage_dir),
        Err(BackupError::Meta { .. })
    ));
}

fn init_repo(root: &Path) -> Storage {
    let location = RepoLocation {
        repository: root.join("repo").to_str().unwrap().into(),
        options: BTreeMap::new(),
    };
    Storage::init(&location, "", Compression::Default).unwrap()
}

/// A game with one save file `<title>.sav` holding `contents` under `root/src`.
fn game_with_save(root: &Path, title: &str, contents: &str) -> GameScan {
    let real = write(&root.join("src").join(format!("{title}.sav")), contents);
    game(title, vec![found(Base::WinAppData, "Game/save.sav", &real)])
}

/// The new snapshot of each game, `None` when it was unchanged; panics on a failed game.
fn written(outcomes: &[GameBackup]) -> Vec<Option<SnapshotInfo>> {
    outcomes
        .iter()
        .map(|outcome| outcome.result.as_ref().unwrap().clone())
        .collect()
}

fn user_reg(prefix: &Path, value: &str) {
    write(
        &prefix.join("user.reg"),
        &format!("WINE REGISTRY Version 2\n\n[Software\\\\Foo] 1\n#time=0\n\"v\"=\"{value}\"\n"),
    );
}

#[test]
fn backup_creates_tagged_labelled_snapshot_per_game() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let games = [
        game_with_save(tmp.path(), "Foo, Part 1: ünï", "foo"),
        game_with_save(tmp.path(), "Bar", "bar"),
    ];
    let stage_root = tmp.path().join("stage");
    let mut seen = Vec::new();
    let outcomes = backup_games(&mut storage, &games, &stage_root, &["extra"], |outcome| {
        seen.push(outcome.title.clone())
    })
    .unwrap();

    assert_eq!(seen, ["Foo, Part 1: ünï", "Bar"]);
    for (game, outcome) in games.iter().zip(&outcomes) {
        let slug = slug(&game.title);
        assert_eq!((&outcome.title, &outcome.slug), (&game.title, &slug));
        let info = outcome.result.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(info.label, game.title);
        let mut tags = info.tags.clone();
        tags.sort();
        assert_eq!(tags, ["extra", &format!("game:{slug}"), "salvajocs"]);

        let restored = tmp.path().join("restored").join(&slug);
        storage
            .restore_path(&info.id, &format!("/salvajocs/{slug}"), &restored)
            .unwrap();
        let meta: SnapshotMeta =
            serde_json::from_slice(&fs::read(restored.join(META_FILE)).unwrap()).unwrap();
        assert_eq!(meta.title, game.title);
        assert!(restored.join("files/WinAppData/Game/save.sav").is_file());
    }
    assert_eq!(storage.snapshots("salvajocs").unwrap().len(), 2);
}

#[test]
fn unchanged_game_makes_no_new_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let games = [
        game_with_save(tmp.path(), "Foo", "foo"),
        game_with_save(tmp.path(), "Bar", "bar"),
    ];
    let stage_root = tmp.path().join("stage");
    let run = |storage: &mut Storage| {
        written(&backup_games(storage, &games, &stage_root, &[], |_| {}).unwrap())
    };

    assert!(run(&mut storage).iter().all(Option::is_some));
    assert_eq!(run(&mut storage), [None, None]);
    write(&games[0].files[0].real, "foo, changed");
    let third = run(&mut storage);
    assert!(third[0].is_some());
    assert_eq!(third[1], None);
    assert_eq!(storage.snapshots("salvajocs").unwrap().len(), 3);
}

#[test]
fn registry_exported_from_recorded_source() {
    let tmp = tempfile::tempdir().unwrap();
    let (first, second) = (tmp.path().join("first"), tmp.path().join("second"));
    user_reg(&first, "first");
    user_reg(&second, "second");
    let mut scan = game("Foo", vec![]);
    scan.registry_keys = vec![format!("{HKCU}\\Software\\Foo")];
    scan.registry_source = Some(RegistrySource::WinePrefix(second.clone()));
    // Changed after the scan: the backup reads the source again.
    user_reg(&second, "fresh");
    let mut storage = init_repo(tmp.path());
    let stage_root = tmp.path().join("stage");
    let games = [scan];

    let outcomes = backup_games(&mut storage, &games, &stage_root, &[], |_| {}).unwrap();
    assert!(written(&outcomes)[0].is_some());
    let registry_file = stage_root.join(slug("Foo")).join(REGISTRY_FILE);
    let staged: RegistryExport =
        serde_json::from_slice(&fs::read(&registry_file).unwrap()).unwrap();
    assert_eq!(staged, registry("fresh"));

    // The keys are gone from the source: the export is empty and `registry.json` is dropped.
    write(&second.join("user.reg"), "WINE REGISTRY Version 2\n");
    let outcomes = backup_games(&mut storage, &games, &stage_root, &[], |_| {}).unwrap();
    assert!(written(&outcomes)[0].is_some());
    assert!(!registry_file.exists());
}

#[cfg(windows)]
#[test]
fn registry_exported_from_recorded_windows_source() {
    /// Deletes the test key on drop, also when the test fails.
    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            // Best effort: a panic here would hide the test failure.
            let _ = windows_registry::CURRENT_USER.remove_tree(&self.0);
        }
    }
    // A sibling of the `registry::windows` test keys, never below one they replace.
    let relative = format!("Software\\salvajocs-test-backup-{}", std::process::id());
    let _cleanup = Cleanup(relative.clone());
    let key = format!("{HKCU}\\{relative}");
    let mut live = RegistryExport::default();
    live.keys.insert(
        key.clone(),
        vec![RegValue {
            name: "v".into(),
            data: RegData::Sz("live".into()),
        }],
    );
    crate::registry::windows::replace_subtrees(&live).unwrap();
    let mut scan = game("Foo", vec![]);
    scan.registry_keys = vec![key];
    scan.registry_source = Some(RegistrySource::Windows);
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let stage_root = tmp.path().join("stage");

    let outcomes = backup_games(&mut storage, &[scan], &stage_root, &[], |_| {}).unwrap();
    assert!(written(&outcomes)[0].is_some());
    let registry_file = stage_root.join(slug("Foo")).join(REGISTRY_FILE);
    let staged: RegistryExport = serde_json::from_slice(&fs::read(registry_file).unwrap()).unwrap();
    assert_eq!(staged, live);
}

#[test]
fn failed_game_does_not_stop_the_others() {
    let tmp = tempfile::tempdir().unwrap();
    let mut broken = game("Broken", vec![]);
    broken.registry_keys = vec![format!("{HKCU}\\Software\\Foo")];
    broken.registry_source = Some(RegistrySource::WinePrefix(tmp.path().join("no-prefix")));
    let games = [broken, game_with_save(tmp.path(), "Fine", "fine")];
    let mut storage = init_repo(tmp.path());

    let outcomes =
        backup_games(&mut storage, &games, &tmp.path().join("stage"), &[], |_| {}).unwrap();
    assert!(matches!(outcomes[0].result, Err(BackupError::Io { .. })));
    assert!(matches!(outcomes[1].result, Ok(Some(_))));

    // Off Windows a live-registry source cannot be read, which is an error, not an empty export.
    #[cfg(not(windows))]
    {
        let mut windows = game("Windows", vec![]);
        windows.registry_source = Some(RegistrySource::Windows);
        let outcomes = backup_games(
            &mut storage,
            &[windows],
            &tmp.path().join("stage"),
            &[],
            |_| {},
        )
        .unwrap();
        assert!(matches!(
            outcomes[0].result,
            Err(BackupError::NoWindowsRegistry)
        ));
    }
}

/// One reindex plus 20 per-game backups on a local repo, first new and then unchanged
/// (ADR-0010): `cargo test --release -p salvajocs-core time_twenty_game_backup -- --ignored
/// --nocapture`.
#[test]
#[ignore = "timing"]
fn time_twenty_game_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let games: Vec<GameScan> = (0..20)
        .map(|i| {
            game_with_save(
                tmp.path(),
                &format!("Game {i}"),
                &i.to_string().repeat(100_000),
            )
        })
        .collect();
    let stage_root = tmp.path().join("stage");
    for run in ["new", "unchanged"] {
        let started = std::time::Instant::now();
        let outcomes = backup_games(&mut storage, &games, &stage_root, &[], |_| {}).unwrap();
        let count = written(&outcomes).iter().flatten().count();
        println!("{run}: {count} snapshots in {:?}", started.elapsed());
    }
}
