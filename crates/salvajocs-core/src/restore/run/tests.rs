use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::backup::backup_games;
use crate::layout::{Base, LinuxFolders};
use crate::manifest::{GameEntry, Integration, Steam};
use crate::scan::{FoundFile, GameScan};
use crate::storage::{Compression, RepoLocation};

/// Writes `contents` to `path`, creating its parent directories.
fn write(path: &Path, contents: &str) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
    path.to_path_buf()
}

fn init_repo(root: &Path) -> Storage {
    let location = RepoLocation {
        repository: root.join("repo").to_str().unwrap().into(),
        options: BTreeMap::new(),
    };
    Storage::init(&location, "", Compression::Default).unwrap()
}

/// A scan of `title` with one save holding `contents`.
fn game_with_save(root: &Path, title: &str, contents: &str) -> GameScan {
    let real = write(&root.join("src").join(format!("{title}.sav")), contents);
    GameScan {
        title: title.into(),
        files: vec![FoundFile {
            base: Base::WinAppData,
            tail: "Game/save.sav".into(),
            size: contents.len() as u64,
            real,
        }],
        roots: vec![(Base::WinAppData, "Game".into())],
        registry_keys: vec![],
        registry_source: None,
        unreadable: vec![],
    }
}

fn back_up(storage: &mut Storage, root: &Path, games: &[GameScan], tags: &[&str]) {
    let outcomes = backup_games(storage, games, &root.join("stage"), tags, |_| {}).unwrap();
    for outcome in outcomes {
        assert!(matches!(outcome.result, Ok(Some(_))), "{outcome:?}");
    }
}

fn is_pre_restore(snapshot: &SnapshotInfo) -> bool {
    snapshot.tags.iter().any(|tag| tag == PRE_RESTORE_TAG)
}

#[test]
fn list_default_skips_pre_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let root = tmp.path();
    back_up(
        &mut storage,
        root,
        &[game_with_save(root, "Foo", "old")],
        &[],
    );
    back_up(
        &mut storage,
        root,
        &[game_with_save(root, "Bar", "bar")],
        &[],
    );
    // Newer than the normal one: the state a restore overwrote.
    let pre = [game_with_save(root, "Foo", "overwritten")];
    back_up(&mut storage, root, &pre, &[PRE_RESTORE_TAG]);
    let only = [game_with_save(root, "Baz", "baz")];
    back_up(&mut storage, root, &only, &[PRE_RESTORE_TAG]);

    let list = list(&storage).unwrap();
    let titles: Vec<&str> = list.iter().map(|game| game.title.as_str()).collect();
    assert_eq!(titles, ["Bar", "Baz", "Foo"]);
    let foo = &list[2];
    assert_eq!(foo.slug, crate::slug::slug("Foo"));
    assert_eq!(foo.snapshots.len(), 2);
    assert!(is_pre_restore(&foo.snapshots[0]));
    assert!(!is_pre_restore(&foo.snapshots[1]));
    assert_eq!(foo.default, Some(1));
    assert_eq!(list[0].default, Some(0));
    assert_eq!(list[1].default, None);
}

/// Creates a Wine prefix at `path` with one user and returns its path.
fn make_prefix(path: &Path, user: &str) -> PathBuf {
    fs::create_dir_all(path.join("drive_c/users").join(user)).unwrap();
    write(&path.join("user.reg"), "WINE REGISTRY Version 2\n");
    path.to_path_buf()
}

fn linux_folders(home: &Path) -> LinuxFolders {
    LinuxFolders {
        user: "me".into(),
        home: home.to_path_buf(),
        xdg_data: home.join(".local/share"),
        xdg_config: home.join(".config"),
    }
}

fn linux_env(home: &Path, steam_roots: Vec<PathBuf>) -> Environment {
    Environment {
        native: Some(Context::Linux(linux_folders(home))),
        steam_roots,
    }
}

fn steam_game(id: u32) -> Manifest {
    let entry = GameEntry {
        steam: Some(Steam { id: Some(id) }),
        ..GameEntry::default()
    };
    Manifest {
        games: [("Foo".to_owned(), entry)].into(),
    }
}

fn in_prefix(prefix: &Path, user: &str) -> Context {
    Context::WinePrefix {
        prefix: prefix.to_path_buf(),
        folders: WinFolders::for_prefix(prefix, user),
    }
}

#[test]
fn default_targets_uses_proton_compatdata() {
    let tmp = tempfile::tempdir().unwrap();
    let steam = tmp.path().join("steam");
    let pfx = make_prefix(&steam.join("steamapps/compatdata/620/pfx"), "steamuser");
    let env = linux_env(&tmp.path().join("home"), vec![steam]);

    assert_eq!(
        default_targets("Foo", &steam_game(620), &[], &env),
        Targets {
            linux: Some(linux_folders(&tmp.path().join("home"))),
            win: Some(in_prefix(&pfx, "steamuser")),
        }
    );
}

#[test]
fn default_targets_prefers_custom_wine_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let steam = tmp.path().join("steam");
    make_prefix(&steam.join("steamapps/compatdata/620/pfx"), "steamuser");
    let own = make_prefix(&tmp.path().join("own"), "me");
    let env = linux_env(&tmp.path().join("home"), vec![steam]);
    let custom = CustomGame {
        name: "Foo".into(),
        entry: GameEntry::default(),
        integration: Integration::Extend,
        wine_prefix: Some(own.clone()),
        pairs: vec![],
        skips: vec![],
    };

    let targets = default_targets("Foo", &steam_game(620), &[custom], &env);
    assert_eq!(targets.win, Some(in_prefix(&own, "me")));
    assert!(targets.linux.is_some());
}

#[test]
fn default_targets_linux_only_without_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let steam = tmp.path().join("steam");
    // Another game's prefix is not this game's.
    make_prefix(&steam.join("steamapps/compatdata/999/pfx"), "steamuser");
    let env = linux_env(&tmp.path().join("home"), vec![steam]);

    assert_eq!(
        default_targets("Foo", &steam_game(620), &[], &env),
        Targets {
            linux: Some(linux_folders(&tmp.path().join("home"))),
            win: None,
        }
    );
}

#[test]
fn default_targets_on_windows_host_is_native_only() {
    let tmp = tempfile::tempdir().unwrap();
    let steam = tmp.path().join("steam");
    make_prefix(&steam.join("steamapps/compatdata/620/pfx"), "steamuser");
    let native = Context::WindowsNative(WinFolders::for_prefix(tmp.path(), "Alice"));
    let env = Environment {
        native: Some(native.clone()),
        steam_roots: vec![steam],
    };

    assert_eq!(
        default_targets("Foo", &steam_game(620), &[], &env),
        Targets {
            linux: None,
            win: Some(native),
        }
    );
}
