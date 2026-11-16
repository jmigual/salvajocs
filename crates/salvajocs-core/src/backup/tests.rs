use std::time::{Duration, SystemTime};

use super::*;
use crate::layout::Base;
use crate::registry::{HKCU, RegData, RegValue};
use crate::scan::FoundFile;
use crate::slug::slug;

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
