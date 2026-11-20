use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::backup::{Stored, backup_games};
use crate::config::{AppConfig, PasswordMode, ThemeChoice};
use crate::launcher::{Launcher, LauncherGame, LauncherPrefix};
use crate::layout::{GameVars, LinuxFolders, Root, RootKind, SnapshotMeta};
use crate::manifest::{CustomFile, CustomGame, GameEntry, Integration, Pair, Steam};
use crate::registry::{RegData, RegValue};
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
            modified: fs::metadata(&real).unwrap().modified().ok(),
            real,
        }],
        roots: vec![(Base::WinAppData, "Game".into())],
        registry_keys: vec![],
        registry_source: None,
        prefixes: Vec::new(),
        unreadable: vec![],
    }
}

fn back_up(storage: &mut Storage, root: &Path, games: &[GameScan], tags: &[&str]) {
    let outcomes = backup_games(storage, games, &root.join("stage"), tags, None, |_| {}).unwrap();
    for outcome in outcomes {
        assert!(matches!(outcome.result, Ok(Stored::New(_))), "{outcome:?}");
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

#[test]
fn fetch_reads_back_the_staged_game_into_an_empty_work_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let game = game_with_save(tmp.path(), "Foo", "foo");
    back_up(&mut storage, tmp.path(), std::slice::from_ref(&game), &[]);
    let snapshot = &list(&storage).unwrap()[0].snapshots[0];
    let work = tmp.path().join("work");

    let staged = fetch(&mut storage, snapshot, &work).unwrap();
    assert_eq!(staged.root, work);
    assert_eq!(staged.meta.title, "Foo");
    assert_eq!(staged.meta.roots, game.roots);
    assert_eq!(staged.files, [(Base::WinAppData, "Game/save.sav".into())]);
    assert_eq!(staged.registry, None);
    // Leftovers of an earlier fetch would show up as staged files.
    assert!(matches!(
        fetch(&mut storage, snapshot, &work),
        Err(FetchError::WorkDirNotEmpty(_))
    ));
}

/// Backs `stage` up as a snapshot of the game `Foo`, as a crafted repository could hold it.
fn crafted_snapshot(storage: &mut Storage, stage: &Path) -> SnapshotInfo {
    let slug = crate::slug::slug("Foo");
    let game_tag = format!("game:{slug}");
    let as_path = format!("/salvajocs/{slug}");
    let tags = ["salvajocs", game_tag.as_str()];
    storage
        .backup_dir(stage, &as_path, &tags, "Foo")
        .unwrap()
        .unwrap()
}

/// A staged game at `stage` with a valid `salvajocs.json`.
fn crafted_stage(stage: PathBuf) -> PathBuf {
    let meta = SnapshotMeta {
        format: crate::layout::SNAPSHOT_FORMAT,
        title: "Foo".into(),
        roots: vec![],
        prefix: None,
    };
    write(
        &stage.join(META_FILE),
        &serde_json::to_string(&meta).unwrap(),
    );
    stage
}

#[test]
fn fetch_refuses_an_entry_of_files_that_is_no_base_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let bogus = crafted_stage(tmp.path().join("bogus"));
    write(&bogus.join("files/Bogus/x.sav"), "x");
    let base_as_file = crafted_stage(tmp.path().join("file"));
    write(&base_as_file.join("files/WinHome"), "x");

    for (n, stage) in [&bogus, &base_as_file].into_iter().enumerate() {
        let snapshot = crafted_snapshot(&mut storage, stage);
        let work = tmp.path().join(format!("work{n}"));
        assert!(
            matches!(
                fetch(&mut storage, &snapshot, &work),
                Err(FetchError::UnknownBase(_))
            ),
            "{stage:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn fetch_refuses_symlinked_files_dir_and_meta() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let outside = tmp.path().join("outside");
    write(&outside.join("WinHome/secret.sav"), "host data");
    let meta = write(
        &outside.join("meta.json"),
        r#"{"format":1,"title":"Foo","roots":[]}"#,
    );
    let linked_files = crafted_stage(tmp.path().join("files-link"));
    std::os::unix::fs::symlink(&outside, linked_files.join(FILES_DIR)).unwrap();
    let linked_meta = tmp.path().join("meta-link");
    fs::create_dir_all(&linked_meta).unwrap();
    std::os::unix::fs::symlink(&meta, linked_meta.join(META_FILE)).unwrap();

    for (n, stage) in [&linked_files, &linked_meta].into_iter().enumerate() {
        let snapshot = crafted_snapshot(&mut storage, stage);
        let work = tmp.path().join(format!("work{n}"));
        assert!(
            matches!(
                fetch(&mut storage, &snapshot, &work),
                Err(FetchError::WrongKind(_))
            ),
            "{stage:?}"
        );
    }
}

/// A staged game at `stage` whose `salvajocs.json` records the root `<winAppData>/Game`.
fn crafted_game_stage(stage: PathBuf) -> PathBuf {
    let meta = SnapshotMeta {
        format: crate::layout::SNAPSHOT_FORMAT,
        title: "Foo".into(),
        roots: vec![(Base::WinAppData, "Game".into())],
        prefix: None,
    };
    write(
        &stage.join(META_FILE),
        &serde_json::to_string(&meta).unwrap(),
    );
    stage
}

/// The restore plan of `staged` into the prefix `ctx`, declaring `<winAppData>/Game` and `FOO`.
fn plan_into(staged: &StagedGame, ctx: &Context) -> RestorePlan {
    let manifest = crate::manifest::parse_yaml(
        "Foo:\n  files:\n    <winAppData>/Game: {}\n  registry:\n    \
         HKEY_CURRENT_USER/Software/Foo: {}\n",
    )
    .unwrap();
    let targets = Targets {
        linux: None,
        win: Some(ctx.clone()),
    };
    let vars = GameVars::new("Foo", vec![], &[], None);
    super::super::plan(staged, &targets, &vars, manifest.games.get("Foo"), &[], &[]).unwrap()
}

#[test]
fn read_matches_fetch() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let prefix = make_prefix(&tmp.path().join("picked"), "me");
    let ctx = in_prefix(&prefix, "me");

    let source = make_prefix(&tmp.path().join("source"), "me");
    user_reg(&source, "a");
    let mut game = game_with_save(tmp.path(), "Foo", "foo");
    game.registry_keys = vec![FOO.into()];
    game.registry_source = Some(RegistrySource::WinePrefix(source));
    back_up(&mut storage, tmp.path(), &[game], &[]);
    let backed_up = list(&storage).unwrap()[0].snapshots[0].clone();

    let nested = crafted_game_stage(tmp.path().join("nested"));
    write(&nested.join("files/WinAppData/Game/a.sav"), "a");
    write(&nested.join("files/WinAppData/Game/deep/er/b.sav"), "b");
    write(&nested.join("files/XdgData/Other/c.sav"), "c");
    // Only the unix-only cases below add to it.
    #[cfg_attr(not(unix), allow(unused_mut, clippy::useless_vec))]
    let mut snapshots = vec![backed_up, crafted_snapshot(&mut storage, &nested)];
    #[cfg(unix)]
    {
        let linked = crafted_game_stage(tmp.path().join("linked"));
        let secret = write(&tmp.path().join("secret.txt"), "host data");
        write(&linked.join("files/WinAppData/Game/save.sav"), "save");
        let link = linked.join("files/WinAppData/Game/link.sav");
        std::os::unix::fs::symlink(&secret, link).unwrap();
        snapshots.push(crafted_snapshot(&mut storage, &linked));
        // A name that is not UTF-8 reads lossily on both sides.
        let named = crafted_game_stage(tmp.path().join("named"));
        let odd = <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(b"\xff.sav");
        write(&named.join("files/WinAppData/Game").join(odd), "odd");
        snapshots.push(crafted_snapshot(&mut storage, &named));
    }
    // A game with no files at all: nothing to write.
    let empty = crafted_game_stage(tmp.path().join("empty"));
    let empty = crafted_snapshot(&mut storage, &empty);

    let with_writes = snapshots.iter().map(|snapshot| (snapshot, true));
    for (n, (snapshot, writes)) in with_writes.chain([(&empty, false)]).enumerate() {
        let work = tmp.path().join(format!("work{n}"));
        let listed = super::read(&mut storage, snapshot, &work).unwrap();
        let planned = plan_into(&listed.staged, &ctx);
        assert_eq!(!planned.writes.is_empty(), writes, "{n}: {planned:?}");
        let fetched = fetch(&mut storage, snapshot, &work).unwrap();
        assert_eq!(listed.staged, fetched, "{n}");
        assert_eq!(planned, plan_into(&fetched, &ctx), "{n}");
    }
}

#[test]
fn read_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let game = game_with_save(tmp.path(), "Foo", "foo");
    back_up(&mut storage, tmp.path(), &[game], &[]);
    let snapshot = &list(&storage).unwrap()[0].snapshots[0];
    let work = tmp.path().join("work");

    let listed = super::read(&mut storage, snapshot, &work).unwrap();
    assert_eq!(listed.staged.root, work);
    let files = [(Base::WinAppData, "Game/save.sav".into())];
    assert_eq!(listed.staged.files, files);
    assert!(!work.exists());
}

#[test]
fn read_refuses_an_entry_of_files_that_is_no_base_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let bogus = crafted_stage(tmp.path().join("bogus"));
    write(&bogus.join("files/Bogus/x.sav"), "x");
    let empty = crafted_stage(tmp.path().join("empty"));
    fs::create_dir_all(empty.join("files/Bogus")).unwrap();
    let base_as_file = crafted_stage(tmp.path().join("file"));
    write(&base_as_file.join("files/WinHome"), "x");

    for (n, stage) in [&bogus, &empty, &base_as_file].into_iter().enumerate() {
        let snapshot = crafted_snapshot(&mut storage, stage);
        let work = tmp.path().join(format!("work{n}"));
        assert!(
            matches!(
                super::read(&mut storage, &snapshot, &work),
                Err(FetchError::UnknownBase(_))
            ),
            "{stage:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn read_refuses_symlinked_files_dir_and_meta() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let outside = tmp.path().join("outside");
    write(&outside.join("WinHome/secret.sav"), "host data");
    let meta = write(
        &outside.join("meta.json"),
        r#"{"format":1,"title":"Foo","roots":[]}"#,
    );
    let linked_files = crafted_stage(tmp.path().join("files-link"));
    std::os::unix::fs::symlink(&outside, linked_files.join(FILES_DIR)).unwrap();
    let linked_meta = tmp.path().join("meta-link");
    fs::create_dir_all(&linked_meta).unwrap();
    std::os::unix::fs::symlink(&meta, linked_meta.join(META_FILE)).unwrap();

    for (n, stage) in [&linked_files, &linked_meta].into_iter().enumerate() {
        let snapshot = crafted_snapshot(&mut storage, stage);
        let work = tmp.path().join(format!("work{n}"));
        assert!(
            matches!(
                super::read(&mut storage, &snapshot, &work),
                Err(FetchError::WrongKind(_))
            ),
            "{stage:?}"
        );
    }
}

#[test]
fn read_without_meta_is_no_meta() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let stage = tmp.path().join("stage");
    write(&stage.join("files/WinAppData/Game/save.sav"), "save");
    let snapshot = crafted_snapshot(&mut storage, &stage);

    let read = super::read(&mut storage, &snapshot, &tmp.path().join("work"));
    assert!(matches!(read, Err(FetchError::NoMeta(_))), "{read:?}");
}

#[test]
fn read_keeps_a_verified_hash_list_and_drops_a_stale_one() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let game = game_with_save(tmp.path(), "Foo", "foo");
    back_up(&mut storage, tmp.path(), &[game], &[]);
    let backed_up = list(&storage).unwrap()[0].snapshots[0].clone();
    let without = crafted_game_stage(tmp.path().join("without"));
    write(&without.join("files/WinAppData/Game/save.sav"), "save");
    let without = crafted_snapshot(&mut storage, &without);
    // A list that misses the staged file, as an older version could leave it.
    let stale = crafted_game_stage(tmp.path().join("stale"));
    write(&stale.join("files/WinAppData/Game/save.sav"), "save");
    write(
        &stale.join(crate::layout::HASHES_FILE),
        r#"{"format":1,"meta":"00","files":{}}"#,
    );
    let stale = crafted_snapshot(&mut storage, &stale);

    let mut hashes = |snapshot: &SnapshotInfo, work: &str| {
        let work = tmp.path().join(work);
        super::read(&mut storage, snapshot, &work).unwrap().hashes
    };
    let kept = hashes(&backed_up, "work0").unwrap();
    assert!(kept.files.contains_key("WinAppData/Game/save.sav"));
    assert_eq!(hashes(&without, "work1"), None);
    assert_eq!(hashes(&stale, "work2"), None);
}

#[test]
fn download_fills_the_planned_root() {
    let mut f = fixture();
    let game = game_with_save(f.tmp.path(), "Foo", "saved");
    back_up(&mut f.storage, f.tmp.path(), &[game], &[]);
    let snapshot = list(&f.storage).unwrap()[0].snapshots[0].clone();
    let work = f.tmp.path().join("work");
    let listed = super::read(&mut f.storage, &snapshot, &work).unwrap();
    let plan = plan_into(&listed.staged, &f.ctx());

    download(&mut f.storage, &listed).unwrap();
    let report = f.run(&plan, false).unwrap();
    assert_applied(&report);
    assert_eq!(read(&f.folders.app_data.join("Game/save.sav")), "saved");
}

#[test]
fn download_refuses_another_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let planned = game_with_save(tmp.path(), "Foo", "planned");
    back_up(
        &mut storage,
        tmp.path(),
        std::slice::from_ref(&planned),
        &[],
    );
    // Another backup of the game with one more file, so its listing differs.
    let mut other = planned;
    let extra = write(&tmp.path().join("src/extra.sav"), "extra");
    other.files.push(FoundFile {
        base: Base::WinAppData,
        tail: "Game/extra.sav".into(),
        size: 5,
        modified: fs::metadata(&extra).unwrap().modified().ok(),
        real: extra,
    });
    back_up(&mut storage, tmp.path(), &[other], &[]);
    let snapshots = list(&storage).unwrap().remove(0).snapshots;
    let (newer, older) = (&snapshots[0], &snapshots[1]);
    let work = tmp.path().join("work");
    let listed = super::read(&mut storage, older, &work).unwrap();

    // The newer snapshot paired with the older one's plan.
    let paired = Listed::new(newer.clone(), listed.staged, listed.listing, listed.hashes);
    let downloaded = download(&mut storage, &paired);
    assert!(
        matches!(downloaded, Err(FetchError::NotAsPlanned(_))),
        "{downloaded:?}"
    );
}

#[test]
fn download_writes_the_listed_snapshot_where_a_newer_one_has_the_same_paths() {
    let mut f = fixture();
    let game = game_with_save(f.tmp.path(), "Foo", "older");
    back_up(&mut f.storage, f.tmp.path(), &[game], &[]);
    let game = game_with_save(f.tmp.path(), "Foo", "newer");
    back_up(&mut f.storage, f.tmp.path(), &[game], &[]);
    let snapshots = list(&f.storage).unwrap().remove(0).snapshots;
    let work = f.tmp.path().join("work");
    let listed = super::read(&mut f.storage, &snapshots[1], &work).unwrap();
    let plan = plan_into(&listed.staged, &f.ctx());

    download(&mut f.storage, &listed).unwrap();
    assert_applied(&f.run(&plan, false).unwrap());
    assert_eq!(read(&f.folders.app_data.join("Game/save.sav")), "older");
}

#[test]
fn a_forgotten_snapshot_fails_its_download_and_nothing_is_written() {
    let mut f = fixture();
    let game = game_with_save(f.tmp.path(), "Foo", "saved");
    back_up(&mut f.storage, f.tmp.path(), &[game], &[]);
    let snapshot = list(&f.storage).unwrap()[0].snapshots[0].clone();
    let work = f.tmp.path().join("work");
    let listed = super::read(&mut f.storage, &snapshot, &work).unwrap();
    let target = write(&f.folders.app_data.join("Game/save.sav"), "old");

    f.storage.forget(&[&snapshot.id]).unwrap();
    let downloaded = download(&mut f.storage, &listed);
    assert!(
        matches!(downloaded, Err(FetchError::Storage(_))),
        "{downloaded:?}"
    );
    assert_eq!(read(&target), "old");
    assert!(f.storage.snapshots(PRE_RESTORE_TAG).unwrap().is_empty());
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
        launcher_prefixes: Vec::new(),
        launcher_problems: Vec::new(),
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
        default_targets("Foo", &steam_game(620), &config(vec![]), &env),
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
        files: Vec::new(),
        entry: GameEntry::default(),
        integration: Integration::Extend,
        wine_prefix: Some(own.clone()),
        pairs: vec![],
        skips: vec![],
    };

    let mut config = config(vec![]);
    config.custom_games = vec![custom];

    let targets = default_targets("Foo", &steam_game(620), &config, &env);
    assert_eq!(targets.win, Some(in_prefix(&own, "me")));
    assert!(targets.linux.is_some());
}

#[test]
fn default_targets_finds_proton_in_configured_steam_library() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let pfx = make_prefix(&library.join("steamapps/compatdata/620/pfx"), "steamuser");
    let env = linux_env(&tmp.path().join("home"), vec![tmp.path().join("steam")]);
    let roots = vec![Root {
        path: library,
        kind: RootKind::Steam,
    }];

    let targets = default_targets("Foo", &steam_game(620), &config(roots), &env);
    assert_eq!(targets.win, Some(in_prefix(&pfx, "steamuser")));
}

/// `env` with a Heroic prefix at `path` for the game titled `title`.
fn with_launcher_prefix(mut env: Environment, path: &Path, title: &str) -> Environment {
    env.launcher_prefixes.push(LauncherPrefix {
        launcher: Launcher::Heroic,
        path: path.to_path_buf(),
        game: Some(LauncherGame {
            title: Some(title.to_owned()),
            ..LauncherGame::default()
        }),
    });
    env
}

#[test]
fn default_targets_uses_the_launcher_prefix_after_proton() {
    let tmp = tempfile::tempdir().unwrap();
    let steam = tmp.path().join("steam");
    let heroic = make_prefix(&tmp.path().join("heroic"), "me");
    let env = linux_env(&tmp.path().join("home"), vec![steam.clone()]);
    let env = with_launcher_prefix(env, &heroic, "Foo");

    let targets = default_targets("Foo", &steam_game(620), &config(vec![]), &env);
    assert_eq!(targets.win, Some(in_prefix(&heroic, "me")));

    let pfx = make_prefix(&steam.join("steamapps/compatdata/620/pfx"), "steamuser");
    let targets = default_targets("Foo", &steam_game(620), &config(vec![]), &env);
    assert_eq!(targets.win, Some(in_prefix(&pfx, "steamuser")));
}

#[test]
fn default_targets_pass_over_ignored_and_other_games_launcher_prefixes() {
    let tmp = tempfile::tempdir().unwrap();
    let ignored = make_prefix(&tmp.path().join("ignored"), "me");
    let other = make_prefix(&tmp.path().join("other"), "me");
    let env = linux_env(&tmp.path().join("home"), vec![]);
    let env = with_launcher_prefix(env, &ignored, "Foo");
    let env = with_launcher_prefix(env, &other, "Unknown");
    let mut config = config(vec![]);
    config.ignored_prefixes = vec![ignored];

    let targets = default_targets("Foo", &steam_game(620), &config, &env);
    assert_eq!(targets.win, None);
}

#[test]
fn default_targets_linux_only_without_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let steam = tmp.path().join("steam");
    // Another game's prefix is not this game's.
    make_prefix(&steam.join("steamapps/compatdata/999/pfx"), "steamuser");
    let env = linux_env(&tmp.path().join("home"), vec![steam]);

    assert_eq!(
        default_targets("Foo", &steam_game(620), &config(vec![]), &env),
        Targets {
            linux: Some(linux_folders(&tmp.path().join("home"))),
            win: None,
        }
    );
}

#[test]
fn game_info_merges_the_custom_game_and_takes_its_pairs_and_skips() {
    let manifest = crate::manifest::parse_yaml(
        "Foo:\n  steam: {id: 620}\n  files:\n    <winAppData>/Foo: {}\n",
    )
    .unwrap();
    let custom = |name: &str, file: &str| CustomGame {
        name: name.into(),
        files: Vec::new(),
        entry: crate::manifest::parse_yaml(&format!("{name}:\n  files:\n    {file}: {{}}\n"))
            .unwrap()
            .games
            .remove(name)
            .unwrap(),
        integration: Integration::Extend,
        wine_prefix: None,
        pairs: vec![Pair {
            linux: format!("<xdgData>/{name}"),
            windows: format!("<winAppData>/{name}"),
        }],
        skips: vec![format!("<home>/{name}")],
    };
    let games = [
        custom("Foo", "<xdgData>/Foo"),
        custom("Bar", "<xdgData>/Bar"),
    ];

    let info = game_info("Foo", &manifest, &games);

    let entry = info.entry.unwrap();
    let files: Vec<_> = entry.files.keys().map(String::as_str).collect();
    assert_eq!(files, ["<winAppData>/Foo", "<xdgData>/Foo"]);
    assert_eq!(entry.steam.and_then(|steam| steam.id), Some(620));
    assert_eq!(info.pairs, games[0].pairs);
    assert_eq!(info.skips, ["<home>/Foo"]);
    assert_eq!(game_info("Baz", &manifest, &games), GameInfo::default());
}

fn pair(linux: &str, windows: &str) -> Pair {
    Pair {
        linux: linux.into(),
        windows: windows.into(),
    }
}

/// A custom game `Foo` with two-path `rows` and the saved `pairs` and `skips`.
fn rows_game(rows: &[(&str, &str)], pairs: Vec<Pair>, skips: &[&str]) -> CustomGame {
    CustomGame {
        name: "Foo".into(),
        files: (rows.iter())
            .map(|&(linux, windows)| CustomFile {
                linux: Some(linux.into()),
                windows: Some(windows.into()),
            })
            .collect(),
        entry: GameEntry::default(),
        integration: Integration::Extend,
        wine_prefix: None,
        pairs,
        skips: skips.iter().map(|&skip| skip.into()).collect(),
    }
}

#[test]
fn game_info_keeps_row_pairs_apart_from_saved_pairs() {
    let game = rows_game(
        &[("<xdgData>/Foo/*.sav", "<winDocuments>/Foo/*.sav")],
        vec![pair("<xdgConfig>/Foo", "<winAppData>/Foo")],
        &[],
    );
    let info = game_info("Foo", &Manifest::default(), slice::from_ref(&game));
    assert_eq!(info.pairs, [pair("<xdgConfig>/Foo", "<winAppData>/Foo")]);
    assert_eq!(
        info.row_pairs,
        [pair("<xdgData>/Foo", "<winDocuments>/Foo")]
    );
}

#[test]
fn a_saved_pair_wins_a_tie_with_a_row_pair() {
    let game = rows_game(
        &[("<xdgData>/Foo/*.sav", "<winDocuments>/Foo/*.sav")],
        vec![pair("<xdgData>/Foo", "<winAppData>/Saved")],
        &[],
    );
    let info = game_info("Foo", &Manifest::default(), slice::from_ref(&game));
    let staged = StagedGame {
        root: "/stage/foo".into(),
        meta: SnapshotMeta {
            format: layout::SNAPSHOT_FORMAT,
            title: "Foo".into(),
            roots: vec![(Base::XdgData, "Foo".into())],
            prefix: None,
        },
        files: vec![(Base::XdgData, "Foo/a.sav".into())],
        registry: None,
    };
    let targets = Targets {
        linux: None,
        win: Some(Context::WindowsNative(WinFolders::for_prefix(
            Path::new("/win"),
            "Alice",
        ))),
    };
    let vars = GameVars::new("Foo", vec![], &[], None);
    let plan = super::super::prepare(&staged, &targets, &vars, &info, &[]).unwrap();
    let written: Vec<_> = plan.writes.iter().map(|w| w.target.as_path()).collect();
    assert_eq!(
        written,
        [Path::new(
            "/win/drive_c/users/Alice/AppData/Roaming/Saved/a.sav"
        )]
    );
}

#[test]
fn a_saved_skip_drops_the_row_pair_of_its_root() {
    for skip in [
        "<home>/.local/share/Foo",
        "<winDocuments>/Foo",
        "<winDocuments>/foo",
    ] {
        let game = rows_game(
            &[("<xdgData>/Foo/*.sav", "<winDocuments>/Foo/*.sav")],
            Vec::new(),
            &[skip],
        );
        let info = game_info("Foo", &Manifest::default(), slice::from_ref(&game));
        assert_eq!(info.row_pairs, [], "{skip}");
    }
    let game = rows_game(
        &[("<xdgData>/Foo/*.sav", "<winDocuments>/Foo/*.sav")],
        Vec::new(),
        &["<xdgData>/foo"],
    );
    let info = game_info("Foo", &Manifest::default(), slice::from_ref(&game));
    assert_eq!(info.row_pairs.len(), 1, "Linux sides keep their case");
}

#[test]
fn game_vars_finds_the_install_dir_under_steam_and_configured_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let steam = tmp.path().join("steam");
    let other = tmp.path().join("games");
    let manifest =
        crate::manifest::parse_yaml("Foo:\n  steam: {id: 620}\n  installDir:\n    FooDir: {}\n")
            .unwrap();
    let entry = manifest.games.get("Foo");
    let env = linux_env(&tmp.path().join("home"), vec![steam.clone()]);
    let config = config(vec![Root {
        path: other.clone(),
        kind: RootKind::Other,
    }]);
    let ctx = Context::Linux(linux_folders(&tmp.path().join("home")));

    fs::create_dir_all(other.join("FooDir")).unwrap();
    let vars = game_vars("Foo", entry, &config, &env);
    assert_eq!(vars.store_game_id.as_deref(), Some("620"));
    let game_dir = layout::base_dir(Base::GameDir, &ctx, &vars);
    assert_eq!(game_dir, Some(other.join("FooDir")));

    // The detected Steam root comes first, as in the scan.
    fs::create_dir_all(steam.join("steamapps/common/FooDir")).unwrap();
    let game_dir = layout::base_dir(Base::GameDir, &ctx, &vars);
    assert_eq!(game_dir, Some(steam.join("steamapps/common/FooDir")));
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
        launcher_prefixes: Vec::new(),
        launcher_problems: Vec::new(),
    };

    assert_eq!(
        default_targets("Foo", &steam_game(620), &config(vec![]), &env),
        Targets {
            linux: None,
            win: Some(native),
        }
    );
}

const FOO: &str = "HKEY_CURRENT_USER\\Software\\Foo";

/// Writes a `user.reg` holding `Software\Foo` with `v` = `value`, and `Software\Other`.
fn user_reg(prefix: &Path, value: &str) {
    write(
        &prefix.join("user.reg"),
        &format!(
            "WINE REGISTRY Version 2\n\n[Software\\\\Foo] 1\n#time=0\n\"v\"=\"{value}\"\n\n\
             [Software\\\\Other] 1\n#time=0\n\"keep\"=\"me\"\n"
        ),
    );
}

fn foo_registry(value: &str) -> RegistryExport {
    let mut export = RegistryExport::default();
    export.keys.insert(
        FOO.into(),
        vec![RegValue {
            name: "v".into(),
            data: RegData::Sz(value.into()),
        }],
    );
    export
}

/// `key` and its subtree as the `user.reg` of `prefix` holds them.
fn reg(prefix: &Path, key: &str) -> RegistryExport {
    let text = fs::read_to_string(prefix.join("user.reg")).unwrap();
    RegFile::parse(&text).unwrap().export(&[key]).unwrap()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

/// A repository and a user-picked prefix with the user `me`.
struct Fixture {
    tmp: tempfile::TempDir,
    storage: Storage,
    prefix: PathBuf,
    folders: WinFolders,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let storage = init_repo(tmp.path());
    let prefix = make_prefix(&tmp.path().join("picked"), "me");
    let folders = WinFolders::for_prefix(&prefix, "me");
    Fixture {
        tmp,
        storage,
        prefix,
        folders,
    }
}

impl Fixture {
    fn staged(&self) -> PathBuf {
        self.tmp.path().join("staged")
    }

    /// A write of the staged `WinAppData` file `tail`, holding `contents`, into the prefix.
    fn planned(&self, tail: &str, contents: &str) -> Write {
        Write {
            source: write(&self.staged().join("files/WinAppData").join(tail), contents),
            target: self.folders.app_data.join(tail),
            target_file: Some((Base::WinAppData, tail.into())),
            target_root: Some((Base::WinAppData, "<game>".into())),
        }
    }

    fn ctx(&self) -> Context {
        in_prefix(&self.prefix, "me")
    }

    fn run(
        &mut self,
        plan: &RestorePlan,
        wineserver_running: bool,
    ) -> Result<RestoreReport, RunError> {
        let stage_root = self.tmp.path().join("pre-restore");
        run(
            &mut self.storage,
            "Foo",
            plan,
            &stage_root,
            wineserver_running,
        )
    }

    /// The staged game of the safety backup `info`.
    fn restored(&mut self, info: &SnapshotInfo) -> PathBuf {
        let dir = self.tmp.path().join("restored").join(&info.id);
        let path = format!("/salvajocs/{}", crate::slug::slug("Foo"));
        self.storage.restore_path(&info.id, &path, &dir).unwrap();
        dir
    }
}

fn plan_of(writes: Vec<Write>, registry: Option<(RegistryExport, Context)>) -> RestorePlan {
    RestorePlan {
        writes,
        registry,
        skipped: vec![],
        cross_build: false,
        notes: vec![],
    }
}

fn staged_meta(dir: &Path) -> SnapshotMeta {
    serde_json::from_slice(&fs::read(dir.join(META_FILE)).unwrap()).unwrap()
}

fn created(report: &RestoreReport) -> &SnapshotInfo {
    match &report.safety_backup {
        SafetyBackup::Created(info) => info,
        other => panic!("no safety backup: {other:?}"),
    }
}

#[test]
fn no_safety_backup_when_no_planned_key_or_file_exists() {
    let mut f = fixture();
    write(
        &f.prefix.join("user.reg"),
        "WINE REGISTRY Version 2\n\n[Software\\\\Other] 1\n#time=0\n\"keep\"=\"me\"\n",
    );
    let writes = vec![f.planned("Game/save.sav", "new")];
    let plan = plan_of(writes, Some((foo_registry("new"), f.ctx())));

    let report = f.run(&plan, false).unwrap();
    assert_eq!(report.safety_backup, SafetyBackup::NothingExisted);
    assert!(f.storage.snapshots("salvajocs").unwrap().is_empty());
    assert!(matches!(report.registry, Some(Ok(()))));
    assert_eq!(reg(&f.prefix, FOO), foo_registry("new"));
    let kept = reg(&f.prefix, "HKEY_CURRENT_USER\\Software\\Other");
    assert_eq!(kept.keys.len(), 1);
}

#[test]
fn registry_restore_refused_while_wineserver_runs() {
    let mut f = fixture();
    user_reg(&f.prefix, "old");
    let target = write(&f.folders.app_data.join("Game/save.sav"), "old");
    let writes = vec![f.planned("Game/save.sav", "new")];
    let plan = plan_of(writes.clone(), Some((foo_registry("new"), f.ctx())));

    assert!(matches!(
        f.run(&plan, true),
        Err(RunError::WineserverRunning)
    ));
    assert_eq!(read(&target), "old");
    assert_eq!(reg(&f.prefix, FOO), foo_registry("old"));
    assert!(f.storage.snapshots("salvajocs").unwrap().is_empty());

    // Files alone leave the registry alone, so they restore while a wineserver runs.
    let report = f.run(&plan_of(writes, None), true).unwrap();
    assert!(report.files.iter().all(Result::is_ok), "{report:?}");
    assert_eq!(read(&target), "new");
}

#[test]
fn apply_aborts_when_safety_backup_fails() {
    let mut f = fixture();
    let target = write(&f.folders.app_data.join("Game/save.sav"), "old");
    let fresh = f.folders.app_data.join("Game/fresh.sav");
    let writes = vec![
        f.planned("Game/save.sav", "new"),
        f.planned("Game/fresh.sav", "new"),
    ];
    let repo = f.tmp.path().join("repo");
    let moved = f.tmp.path().join("moved");
    fs::rename(&repo, &moved).unwrap();

    let plan = plan_of(writes.clone(), None);
    assert!(matches!(
        f.run(&plan, false),
        Err(RunError::SafetyBackup(BackupError::Storage(_)))
    ));
    assert_eq!(read(&target), "old");
    assert!(!fresh.exists());

    // The repository is back, but the game's own backup fails on an unparsable `user.reg`.
    fs::rename(&moved, &repo).unwrap();
    write(&f.prefix.join("user.reg"), "not a registry");
    let plan = plan_of(writes, Some((foo_registry("new"), f.ctx())));
    assert!(matches!(
        f.run(&plan, false),
        Err(RunError::SafetyBackup(BackupError::WineRegistry { .. }))
    ));
    assert_eq!(read(&target), "old");
    assert!(!fresh.exists());
    assert_eq!(read(&f.prefix.join("user.reg")), "not a registry");
    assert!(f.storage.snapshots("salvajocs").unwrap().is_empty());
}

#[test]
fn safety_backup_covers_planned_targets_in_user_picked_prefix() {
    let mut f = fixture();
    user_reg(&f.prefix, "old");
    let target = write(&f.folders.app_data.join("Game/save.sav"), "old");
    let writes = vec![f.planned("Game/save.sav", "new")];
    let plan = plan_of(writes, Some((foo_registry("new"), f.ctx())));

    let report = f.run(&plan, false).unwrap();
    let info = created(&report);
    assert_eq!(info.label, "Foo");
    assert!(info.tags.iter().any(|tag| tag == PRE_RESTORE_TAG));
    let game_tag = format!("game:{}", crate::slug::slug("Foo"));
    assert!(info.tags.contains(&game_tag));
    let dir = f.restored(info);
    assert_eq!(read(&dir.join("files/WinAppData/Game/save.sav")), "old");
    let registry: RegistryExport =
        serde_json::from_slice(&fs::read(dir.join(REGISTRY_FILE)).unwrap()).unwrap();
    assert_eq!(registry, foo_registry("old"));

    assert!(report.files.iter().all(Result::is_ok), "{report:?}");
    assert!(matches!(report.registry, Some(Ok(()))));
    assert_eq!(read(&target), "new");
    assert_eq!(reg(&f.prefix, FOO), foo_registry("new"));
}

#[test]
fn safety_backup_meta_has_roots() {
    let mut f = fixture();
    write(&f.folders.app_data.join("Game/save.sav"), "old");
    let missing = Write {
        source: write(&f.staged().join("files/WinDocuments/x.sav"), "new"),
        target: f.folders.documents.join("My Games/Game/x.sav"),
        target_file: Some((Base::WinDocuments, "My Games/Game/x.sav".into())),
        target_root: Some((Base::WinDocuments, "My Games/<game>".into())),
    };
    let plan = plan_of(vec![f.planned("Game/save.sav", "new"), missing], None);

    let report = f.run(&plan, false).unwrap();
    let dir = f.restored(created(&report));
    let meta = staged_meta(&dir);
    assert_eq!(meta.title, "Foo");
    // Only the root of a target that existed, raw as planned.
    assert_eq!(meta.roots, [(Base::WinAppData, "<game>".to_owned())]);
    assert!(!dir.join("files/WinDocuments").exists());
}

#[test]
fn safety_backup_covers_overridden_target() {
    let mut f = fixture();
    let elsewhere = write(&f.tmp.path().join("elsewhere/save.sav"), "old");
    let mut plan = plan_of(vec![f.planned("Game/save.sav", "new")], None);
    plan.override_target(0, elsewhere.clone()).unwrap();

    let report = f.run(&plan, false).unwrap();
    let dir = f.restored(created(&report));
    let base = if cfg!(windows) {
        Base::WinAbsolute
    } else {
        Base::Absolute
    };
    let tail = layout::tail_of(&elsewhere, Path::new("")).unwrap();
    let staged = layout::target_path(&dir.join("files").join(base.dir_name()), &tail, false);
    assert_eq!(read(&staged.unwrap()), "old");
    assert_eq!(staged_meta(&dir).roots, [(base, tail)]);
    assert_eq!(read(&elsewhere), "new");
    assert!(!f.folders.app_data.join("Game/save.sav").exists());
}

#[test]
fn staged_target_keeps_the_planned_key_or_takes_the_absolute_one() {
    let planned = Write {
        source: PathBuf::from("staged"),
        target: PathBuf::from("/home/me/.local/share/Game/x.sav"),
        target_file: Some((Base::XdgData, "Game/x.sav".into())),
        target_root: Some((Base::XdgData, "Game".into())),
    };
    let root = (Base::XdgData, "Game".to_owned());
    let key = (Base::XdgData, "Game/x.sav".to_owned());
    assert_eq!(staged_target(&planned), Some((key, root)));

    let elsewhere = PathBuf::from("/elsewhere/x.sav");
    let overridden = Write {
        target: elsewhere.clone(),
        target_file: None,
        target_root: None,
        ..planned.clone()
    };
    let base = if cfg!(windows) {
        Base::WinAbsolute
    } else {
        Base::Absolute
    };
    let tail = layout::tail_of(&elsewhere, Path::new("")).unwrap();
    assert_eq!(
        staged_target(&overridden),
        Some(((base, tail.clone()), (base, tail)))
    );

    let unstageable = Write {
        target: PathBuf::from("/a/../x.sav"),
        ..overridden
    };
    assert_eq!(staged_target(&unstageable), None);
}

#[cfg(unix)]
#[test]
fn staged_symlink_not_restored() {
    let mut f = fixture();
    let secret = write(&f.tmp.path().join("secret.txt"), "secret");
    let link = f.planned("Game/link.sav", "");
    fs::remove_file(&link.source).unwrap();
    std::os::unix::fs::symlink(&secret, &link.source).unwrap();
    let plan = plan_of(vec![link, f.planned("Game/save.sav", "new")], None);

    let report = f.run(&plan, false).unwrap();
    assert!(matches!(report.files[0], Err(ApplyError::SourceNotAFile)));
    assert!(fs::symlink_metadata(f.folders.app_data.join("Game/link.sav")).is_err());
    assert!(report.files[1].is_ok());
    assert_eq!(read(&f.folders.app_data.join("Game/save.sav")), "new");
}

fn tmp_of(target: &Path) -> PathBuf {
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(".salvajocs-tmp");
    tmp.into()
}

#[test]
fn leftover_temp_file_is_replaced() {
    let mut f = fixture();
    let write_ = f.planned("Game/save.sav", "new");
    let tmp = write(&tmp_of(&write_.target), "left by a crash");

    let report = f.run(&plan_of(vec![write_.clone()], None), false).unwrap();
    assert!(report.files[0].is_ok(), "{report:?}");
    assert_eq!(read(&write_.target), "new");
    assert!(!tmp.exists());
}

#[cfg(unix)]
#[test]
fn leftover_temp_symlink_is_not_followed() {
    let mut f = fixture();
    let outside = write(&f.tmp.path().join("outside.txt"), "outside");
    let write_ = f.planned("Game/save.sav", "new");
    let tmp = tmp_of(&write_.target);
    fs::create_dir_all(tmp.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &tmp).unwrap();

    let report = f.run(&plan_of(vec![write_.clone()], None), false).unwrap();
    assert!(report.files[0].is_ok(), "{report:?}");
    assert_eq!(read(&write_.target), "new");
    assert_eq!(read(&outside), "outside");
    assert!(fs::symlink_metadata(&tmp).is_err());
}

#[test]
fn failed_write_leaves_no_temp_file() {
    let mut f = fixture();
    let write_ = f.planned("Game/save.sav", "new");
    // A non-empty directory where the file goes: the rename fails.
    let inside = write(&write_.target.join("keep.txt"), "keep");

    let report = f.run(&plan_of(vec![write_.clone()], None), false).unwrap();
    assert!(matches!(report.files[0], Err(ApplyError::Io { .. })));
    assert!(!tmp_of(&write_.target).exists());
    assert_eq!(read(&inside), "keep");
}

/// Reads `snapshot` of Foo into the fixture's work dir and plans it into the fixture's prefix.
fn read_and_plan(f: &mut Fixture, snapshot: &SnapshotInfo) -> (Listed, RestorePlan) {
    let work = f.tmp.path().join("work");
    let listed = super::read(&mut f.storage, snapshot, &work).unwrap();
    let plan = plan_into(&listed.staged, &f.ctx());
    (listed, plan)
}

/// Backs up Foo with each `(name, contents)` as the save `Game/<name>`, then reads and plans the
/// snapshot as [`read_and_plan`] does.
fn backed_up_plan(f: &mut Fixture, files: &[(&str, &str)]) -> (Listed, RestorePlan) {
    let files = (files.iter())
        .map(|(name, contents)| {
            let real = write(&f.tmp.path().join("src").join(name), contents);
            FoundFile {
                base: Base::WinAppData,
                tail: format!("Game/{name}"),
                size: contents.len() as u64,
                modified: fs::metadata(&real).unwrap().modified().ok(),
                real,
            }
        })
        .collect();
    let game = GameScan {
        title: "Foo".into(),
        files,
        roots: vec![(Base::WinAppData, "Game".into())],
        registry_keys: vec![],
        registry_source: None,
        prefixes: Vec::new(),
        unreadable: vec![],
    };
    back_up(&mut f.storage, f.tmp.path(), &[game], &[]);
    let snapshot = list(&f.storage).unwrap()[0].snapshots[0].clone();
    read_and_plan(f, &snapshot)
}

/// What [`compare_writes`] says of each write, by its target's file name.
fn states(plan: &RestorePlan, listed: &Listed) -> Vec<(String, WriteState)> {
    let states = compare_writes(plan, listed).unwrap();
    (plan.writes.iter().zip(states))
        .map(|(write, state)| {
            let name = write.target.file_name().unwrap().to_string_lossy();
            (name.into_owned(), state.unwrap())
        })
        .collect()
}

fn named(states: &[(&str, WriteState)]) -> Vec<(String, WriteState)> {
    (states.iter())
        .map(|(name, state)| ((*name).to_owned(), *state))
        .collect()
}

/// Sets the mtime of the file `path` to `secs` after the epoch.
fn set_mtime(path: &Path, secs: u64) {
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn compare_writes_labels_new_changed_and_unchanged_from_the_hash_list() {
    let mut f = fixture();
    let files = [
        ("missing.sav", "new"),
        ("other_size.sav", "new"),
        ("same.sav", "same"),
        ("same_size.sav", "new"),
    ];
    let (listed, plan) = backed_up_plan(&mut f, &files);
    assert!(listed.hashes.is_some());
    let game = f.folders.app_data.join("Game");
    write(&game.join("other_size.sav"), "older");
    // The same bytes with another mtime, as another machine's copy has them.
    set_mtime(&write(&game.join("same.sav"), "same"), 1_000_000);
    write(&game.join("same_size.sav"), "old");

    let expected = named(&[
        ("missing.sav", WriteState::New),
        ("other_size.sav", WriteState::Changed),
        ("same.sav", WriteState::Unchanged),
        ("same_size.sav", WriteState::Changed),
    ]);
    assert_eq!(states(&plan, &listed), expected);
}

#[test]
fn compare_writes_without_a_hash_list_compares_size_and_mtime() {
    let mut f = fixture();
    let stage = crafted_game_stage(f.tmp.path().join("stage"));
    set_mtime(
        &write(&stage.join("files/WinAppData/Game/kept.sav"), "kept"),
        2_000_000,
    );
    set_mtime(
        &write(&stage.join("files/WinAppData/Game/same.sav"), "same"),
        2_000_000,
    );
    let snapshot = crafted_snapshot(&mut f.storage, &stage);
    let (listed, plan) = read_and_plan(&mut f, &snapshot);
    assert_eq!(listed.hashes, None);
    let game = f.folders.app_data.join("Game");
    set_mtime(&write(&game.join("kept.sav"), "kept"), 2_000_000);
    set_mtime(&write(&game.join("same.sav"), "same"), 1_000_000);

    let expected = named(&[
        ("kept.sav", WriteState::Unchanged),
        ("same.sav", WriteState::Changed),
    ]);
    assert_eq!(states(&plan, &listed), expected);
}

#[test]
fn compare_writes_finds_a_file_restored_from_the_snapshot_unchanged() {
    let mut f = fixture();
    // No hash list, so only the mtime the restore sets can make the file unchanged.
    let stage = crafted_game_stage(f.tmp.path().join("stage"));
    write(&stage.join("files/WinAppData/Game/new.sav"), "new");
    write(&stage.join("files/WinAppData/Game/old.sav"), "saved");
    let snapshot = crafted_snapshot(&mut f.storage, &stage);
    let (listed, plan) = read_and_plan(&mut f, &snapshot);
    write(&f.folders.app_data.join("Game/old.sav"), "older");

    download(&mut f.storage, &listed).unwrap();
    assert_applied(&f.run(&plan, false).unwrap());
    let expected = named(&[
        ("new.sav", WriteState::Unchanged),
        ("old.sav", WriteState::Unchanged),
    ]);
    assert_eq!(states(&plan, &listed), expected);
}

#[test]
fn compare_writes_reads_no_snapshot_content() {
    let mut f = fixture();
    let (listed, plan) = backed_up_plan(&mut f, &[("save.sav", "new")]);
    write(&f.folders.app_data.join("Game/save.sav"), "old");

    let states = compare_writes(&plan, &listed).unwrap();
    assert!(
        matches!(states[..], [Ok(WriteState::Changed)]),
        "{states:?}"
    );
    assert!(!listed.staged.root.exists());
}

#[cfg(unix)]
#[test]
fn compare_writes_says_a_symlinked_source_will_fail() {
    let mut f = fixture();
    let stage = crafted_game_stage(f.tmp.path().join("stage"));
    let secret = write(&f.tmp.path().join("secret.txt"), "host data");
    write(&stage.join("files/WinAppData/Game/save.sav"), "save");
    let link = stage.join("files/WinAppData/Game/link.sav");
    std::os::unix::fs::symlink(&secret, link).unwrap();
    let snapshot = crafted_snapshot(&mut f.storage, &stage);
    let (listed, plan) = read_and_plan(&mut f, &snapshot);

    let states = compare_writes(&plan, &listed).unwrap();
    assert!(
        matches!(
            states[..],
            [Err(ApplyError::SourceNotAFile), Ok(WriteState::New)]
        ),
        "{states:?}"
    );
}

#[test]
fn compare_writes_refuses_a_directory_at_the_target() {
    let mut f = fixture();
    let (listed, plan) = backed_up_plan(&mut f, &[("save.sav", "new")]);
    fs::create_dir_all(f.folders.app_data.join("Game/save.sav")).unwrap();

    let states = compare_writes(&plan, &listed).unwrap();
    assert!(
        matches!(states[..], [Err(ApplyError::Io { .. })]),
        "{states:?}"
    );
}

#[cfg(unix)]
#[test]
fn compare_writes_refuses_a_symlinked_target() {
    let mut f = fixture();
    let (listed, plan) = backed_up_plan(&mut f, &[("save.sav", "new")]);
    let outside = write(&f.tmp.path().join("outside.txt"), "new");
    let target = f.folders.app_data.join("Game/save.sav");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &target).unwrap();

    let states = compare_writes(&plan, &listed).unwrap();
    assert!(
        matches!(states[..], [Err(ApplyError::TargetIsSymlink)]),
        "{states:?}"
    );
}

#[cfg(unix)]
#[test]
fn compare_writes_finds_an_unreadable_target_that_blocks_the_whole_restore() {
    use std::os::unix::fs::PermissionsExt;

    let mut f = fixture();
    let files = [("locked.sav", "new"), ("other.sav", "new")];
    let (listed, plan) = backed_up_plan(&mut f, &files);
    let locked = write(&f.folders.app_data.join("Game/locked.sav"), "old");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // Root reads it anyway, so there is nothing to test.
    if fs::read(&locked).is_ok() {
        return;
    }

    let compared = compare_writes(&plan, &listed);
    download(&mut f.storage, &listed).unwrap();
    let result = f.run(&plan, false);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        matches!(&compared, Err(UnreadableTarget { path, .. }) if *path == locked),
        "{compared:?}"
    );
    // What the compare foresees: the safety backup fails, so nothing is written.
    assert!(
        matches!(result, Err(RunError::SafetyBackup(_))),
        "{result:?}"
    );
    assert!(!f.folders.app_data.join("Game/other.sav").exists());
}

#[cfg(unix)]
#[test]
fn unreadable_user_reg_blocks_the_restore() {
    use std::os::unix::fs::PermissionsExt;

    let mut f = fixture();
    user_reg(&f.prefix, "old");
    let user_reg = f.prefix.join("user.reg");
    fs::set_permissions(&user_reg, fs::Permissions::from_mode(0o000)).unwrap();
    // Root reads it anyway, so there is nothing to test.
    if fs::read(&user_reg).is_ok() {
        fs::set_permissions(&user_reg, fs::Permissions::from_mode(0o644)).unwrap();
        return;
    }
    let plan = plan_of(vec![], Some((foo_registry("new"), f.ctx())));

    let result = f.run(&plan, false);
    fs::set_permissions(&user_reg, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        matches!(result, Err(RunError::SafetyBackup(_))),
        "{result:?}"
    );
    assert_eq!(reg(&f.prefix, FOO), foo_registry("old"));
    assert!(f.storage.snapshots("salvajocs").unwrap().is_empty());
}

#[test]
fn prefix_without_user_reg_still_restores_files() {
    let mut f = fixture();
    fs::remove_file(f.prefix.join("user.reg")).unwrap();
    let target = write(&f.folders.app_data.join("Game/save.sav"), "old");
    let writes = vec![f.planned("Game/save.sav", "new")];
    let plan = plan_of(writes, Some((foo_registry("new"), f.ctx())));

    let report = f.run(&plan, false).unwrap();
    let dir = f.restored(created(&report));
    assert_eq!(read(&dir.join("files/WinAppData/Game/save.sav")), "old");
    assert!(!dir.join(REGISTRY_FILE).exists());
    assert!(report.files[0].is_ok(), "{report:?}");
    assert_eq!(read(&target), "new");
    assert!(matches!(
        &report.registry,
        Some(Err(ApplyError::Io { source, .. })) if source.kind() == io::ErrorKind::NotFound
    ));
}

#[cfg(unix)]
#[test]
fn symlinked_target_file_refused() {
    let mut f = fixture();
    let outside = write(&f.tmp.path().join("outside.txt"), "outside");
    let link = f.folders.app_data.join("Game/save.sav");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    // Proton links `Documents` to a real directory; writing below it is fine.
    let real_docs = f.tmp.path().join("real-docs");
    fs::create_dir_all(&real_docs).unwrap();
    std::os::unix::fs::symlink(&real_docs, &f.folders.documents).unwrap();
    let below_link = Write {
        source: write(&f.staged().join("files/WinDocuments/Game/doc.sav"), "new"),
        target: f.folders.documents.join("Game/doc.sav"),
        target_file: Some((Base::WinDocuments, "Game/doc.sav".into())),
        target_root: Some((Base::WinDocuments, "<game>".into())),
    };
    let plan = plan_of(vec![f.planned("Game/save.sav", "new"), below_link], None);

    let report = f.run(&plan, false).unwrap();
    assert!(matches!(report.files[0], Err(ApplyError::TargetIsSymlink)));
    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(read(&outside), "outside");
    // The link was neither followed into the safety backup nor replaced.
    assert_eq!(report.safety_backup, SafetyBackup::NothingExisted);
    assert!(report.files[1].is_ok(), "{report:?}");
    assert_eq!(read(&real_docs.join("Game/doc.sav")), "new");
}

fn config(roots: Vec<Root>) -> AppConfig {
    AppConfig {
        repository: String::new(),
        repository_options: vec![],
        password: PasswordMode::None,
        roots,
        custom_games: vec![],
        check_updates: true,
        ignored_prefixes: Vec::new(),
        theme: ThemeChoice::System,
    }
}

/// Scans `manifest`, backs every found game up and fetches the default snapshot of the first
/// listed game.
fn back_up_and_fetch(
    storage: &mut Storage,
    root: &Path,
    manifest: &Manifest,
    config: &AppConfig,
    env: &Environment,
) -> StagedGame {
    let report = crate::scan::scan(manifest, config, env, |_, _| {});
    assert_eq!(report.problems, []);
    back_up(storage, root, &report.games, &[]);
    let game = list(storage).unwrap().remove(0);
    let snapshot = &game.snapshots[game.default.unwrap()];
    fetch(storage, snapshot, &root.join("work")).unwrap()
}

fn assert_applied(report: &RestoreReport) {
    assert!(report.files.iter().all(Result::is_ok), "{report:?}");
    assert!(
        report.registry.as_ref().is_none_or(Result::is_ok),
        "{report:?}"
    );
}

#[test]
fn proton_backup_restores_into_windows_layout() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let steam = tmp.path().join("steam");
    let pfx = make_prefix(&steam.join("steamapps/compatdata/620/pfx"), "steamuser");
    let home = pfx.join("drive_c/users/steamuser");
    write(&home.join("AppData/Roaming/Foo/save.dat"), "proton");
    write(&home.join("Documents/My Games/Foo/slot1.sav"), "slot");
    let manifest = crate::manifest::parse_yaml(
        "Foo:\n  steam: {id: 620}\n  files:\n    <winAppData>/Foo: {}\n    \
         <winDocuments>/My Games/Foo: {}\n",
    )
    .unwrap();
    let env = Environment {
        native: None,
        steam_roots: vec![steam],
        launcher_prefixes: Vec::new(),
        launcher_problems: Vec::new(),
    };
    let staged = back_up_and_fetch(&mut storage, tmp.path(), &manifest, &config(vec![]), &env);

    // A native Windows user, as plain data.
    let win = WinFolders::for_prefix(&tmp.path().join("win"), "Alice");
    let targets = Targets {
        linux: None,
        win: Some(Context::WindowsNative(win.clone())),
    };
    let vars = GameVars::new("Foo", vec![], &[], Some("620".into()));
    let entry = manifest.games.get("Foo");
    let plan = super::super::prepare(
        &staged,
        &targets,
        &vars,
        &GameInfo {
            entry: entry.cloned(),
            ..GameInfo::default()
        },
        &[],
    )
    .unwrap();
    assert!(plan.skipped.is_empty(), "{plan:?}");
    let stage_root = tmp.path().join("pre-restore");
    let report = run(&mut storage, "Foo", &plan, &stage_root, false).unwrap();

    assert_applied(&report);
    // Nothing existed at the targets.
    assert_eq!(report.safety_backup, SafetyBackup::NothingExisted);
    assert_eq!(read(&win.app_data.join("Foo/save.dat")), "proton");
    assert_eq!(read(&win.documents.join("My Games/Foo/slot1.sav")), "slot");
}

#[test]
fn prefix_registry_restores_into_other_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let source = make_prefix(&tmp.path().join("source"), "me");
    user_reg(&source, "a");
    write(
        &source.join("drive_c/users/me/AppData/Roaming/Foo/save.dat"),
        "a",
    );
    let manifest = crate::manifest::parse_yaml(
        "Foo:\n  files:\n    <winAppData>/Foo: {}\n  registry:\n    \
         HKEY_CURRENT_USER/Software/Foo: {}\n",
    )
    .unwrap();
    let roots = vec![Root {
        path: source,
        kind: RootKind::WinePrefix,
    }];
    let env = Environment {
        native: None,
        steam_roots: vec![],
        launcher_prefixes: Vec::new(),
        launcher_problems: Vec::new(),
    };
    let staged = back_up_and_fetch(&mut storage, tmp.path(), &manifest, &config(roots), &env);
    assert_eq!(staged.registry, Some(foo_registry("a")));

    // Another prefix and user, with its own `Foo` (and a subkey) next to an unrelated key.
    let other = make_prefix(&tmp.path().join("other"), "steamuser");
    write(
        &other.join("user.reg"),
        "WINE REGISTRY Version 2\n\n[Software\\\\Foo] 1\n#time=0\n\"v\"=\"b\"\n\n\
         [Software\\\\Foo\\\\Old] 1\n#time=0\n\n[Software\\\\Other] 1\n#time=0\n\"keep\"=\"me\"\n",
    );
    let targets = Targets {
        linux: None,
        win: Some(in_prefix(&other, "steamuser")),
    };
    let vars = GameVars::new("Foo", vec![], &[], None);
    let entry = manifest.games.get("Foo");
    let plan = super::super::prepare(
        &staged,
        &targets,
        &vars,
        &GameInfo {
            entry: entry.cloned(),
            ..GameInfo::default()
        },
        &[],
    )
    .unwrap();
    let stage_root = tmp.path().join("pre-restore");
    let report = run(&mut storage, "Foo", &plan, &stage_root, false).unwrap();

    assert_applied(&report);
    assert!(report.registry.is_some());
    // The subtree is replaced, `Old` included; the unrelated key stays.
    assert_eq!(reg(&other, FOO), foo_registry("a"));
    let kept = reg(&other, "HKEY_CURRENT_USER\\Software\\Other");
    assert_eq!(kept.keys.len(), 1);
    assert!(!other.join("user.reg.salvajocs-tmp").exists());
    let save = other.join("drive_c/users/steamuser/AppData/Roaming/Foo/save.dat");
    assert_eq!(read(&save), "a");

    // The safety backup holds the other prefix's old subtree.
    let info = created(&report);
    let dir = tmp.path().join("restored");
    let path = format!("/salvajocs/{}", crate::slug::slug("Foo"));
    storage.restore_path(&info.id, &path, &dir).unwrap();
    let old: RegistryExport =
        serde_json::from_slice(&fs::read(dir.join(REGISTRY_FILE)).unwrap()).unwrap();
    let keys: Vec<&str> = old.keys.keys().map(String::as_str).collect();
    assert_eq!(keys, [FOO, "HKEY_CURRENT_USER\\Software\\Foo\\Old"]);
    assert_eq!(old.keys[FOO], foo_registry("b").keys[FOO]);
}
