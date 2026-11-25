use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::backup::{Stored, backup_games};
use crate::config::{AppConfig, ListSort, PasswordMode, ThemeChoice};
use crate::launcher::{Launcher, LauncherGame, LauncherPrefix};
use crate::layout::{
    GameVars, LinuxFolders, PrefixOrigin, Root, RootKind, SnapshotMeta, WinFolders,
};
use crate::manifest::{CustomFile, CustomGame, GameEntry, Integration, Pair, Steam};
use crate::registry::{RegData, RegValue};
use crate::restore::Targets;
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
        prefix_origin: None,
        unreadable: vec![],
        selection: BTreeMap::new(),
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
        .backup_dir(stage, &as_path, &tags, "Foo", true)
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

/// What the manifest says of Foo: it declares `<winAppData>/Game` and `FOO`.
fn foo_info() -> GameInfo {
    let manifest = crate::manifest::parse_yaml(
        "Foo:\n  files:\n    <winAppData>/Game: {}\n  registry:\n    \
         HKEY_CURRENT_USER/Software/Foo: {}\n",
    )
    .unwrap();
    game_info("Foo", &manifest, &[])
}

fn foo_vars() -> GameVars {
    GameVars::new("Foo", vec![], &[], None)
}

/// The restore plan of `staged` into the prefix `ctx`, declaring `<winAppData>/Game` and `FOO`.
fn plan_into(staged: &StagedGame, ctx: &Context) -> RestorePlan {
    let targets = Targets {
        linux: None,
        win: Some(ctx.clone()),
    };
    let entry = foo_info().entry;
    super::super::plan(staged, &targets, &foo_vars(), entry.as_ref(), &[], &[]).unwrap()
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

/// `staged` as listed, with no mtimes.
fn listed_of(staged: &StagedGame) -> Listed {
    super::super::tests::listed_at(staged.clone(), &[])
}

#[test]
fn read_refuses_a_json_file_over_the_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let stage = crafted_game_stage(tmp.path().join("stage"));
    let meta_len = fs::metadata(stage.join(META_FILE)).unwrap().len();
    let plain = crafted_snapshot(&mut storage, &stage);
    // Valid JSON one byte over the cap, so only its size can refuse it.
    let mut registry = br#"{"keys":{}}"#.to_vec();
    registry.resize(usize::try_from(meta_len).unwrap() + 1, b' ');
    fs::write(stage.join(REGISTRY_FILE), registry).unwrap();
    let with_registry = crafted_snapshot(&mut storage, &stage);

    let mut read = |snapshot: &SnapshotInfo, cap: u64, work: &str| {
        let work = tmp.path().join(work);
        read_capped(&mut storage, snapshot, &work, cap)
    };
    let too_large = |read: &Result<Listed, FetchError>| matches!(read, Err(FetchError::Io { source, .. }) if source.kind() == io::ErrorKind::FileTooLarge);
    // A file of exactly the cap is read.
    assert!(read(&plain, meta_len, "work0").is_ok());
    assert!(too_large(&read(&plain, meta_len - 1, "work1")));
    assert!(too_large(&read(&with_registry, meta_len, "work2")));
}

#[test]
fn a_hash_list_over_the_cap_is_not_read() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let game = game_with_save(tmp.path(), "Foo", "foo");
    back_up(&mut storage, tmp.path(), &[game], &[]);
    let id = list(&storage).unwrap()[0].snapshots[0].id.clone();
    let dir = format!("/salvajocs/{}", crate::slug::slug("Foo"));
    let mut listing = storage.snapshot_entries(&id, &dir).unwrap();
    let hash_list = |storage: &mut Storage, listing: &[SnapshotFile]| {
        crate::changes::hash_list(storage, &id, &dir, listing).unwrap()
    };
    assert!(hash_list(&mut storage, &listing).is_some());

    for entry in &mut listing {
        if entry.path == crate::layout::HASHES_FILE {
            entry.size = MAX_JSON_BYTES + 1;
        }
    }
    assert_eq!(hash_list(&mut storage, &listing), None);
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

/// A snapshot of Foo's Windows build, `<winAppData>/Foo`, that names no prefix.
fn windows_staged() -> StagedGame {
    StagedGame {
        root: "/stage/foo".into(),
        meta: SnapshotMeta::new(
            layout::SNAPSHOT_FORMAT,
            "Foo".into(),
            vec![(Base::WinAppData, "Foo".into())],
        ),
        files: vec![(Base::WinAppData, "Foo/save.dat".into())],
        registry: None,
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
        default_targets(
            "Foo",
            &steam_game(620),
            &config(vec![]),
            &env,
            Some(&windows_staged())
        )
        .targets,
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

    let targets = default_targets(
        "Foo",
        &steam_game(620),
        &config,
        &env,
        Some(&windows_staged()),
    )
    .targets;
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

    let targets = default_targets(
        "Foo",
        &steam_game(620),
        &config(roots),
        &env,
        Some(&windows_staged()),
    )
    .targets;
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
        name: String::new(),
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

    let targets = default_targets(
        "Foo",
        &steam_game(620),
        &config(vec![]),
        &env,
        Some(&windows_staged()),
    )
    .targets;
    assert_eq!(targets.win, Some(in_prefix(&heroic, "me")));

    let pfx = make_prefix(&steam.join("steamapps/compatdata/620/pfx"), "steamuser");
    let targets = default_targets(
        "Foo",
        &steam_game(620),
        &config(vec![]),
        &env,
        Some(&windows_staged()),
    )
    .targets;
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

    let targets = default_targets(
        "Foo",
        &steam_game(620),
        &config,
        &env,
        Some(&windows_staged()),
    )
    .targets;
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
        default_targets(
            "Foo",
            &steam_game(620),
            &config(vec![]),
            &env,
            Some(&windows_staged())
        )
        .targets,
        Targets {
            linux: Some(linux_folders(&tmp.path().join("home"))),
            win: None,
        }
    );
}

/// A prefix of `launcher` at `path` that it names `name`, tied to no game.
fn launcher_named(launcher: Launcher, path: &Path, name: &str) -> LauncherPrefix {
    let mut prefix = LauncherPrefix::new(launcher, path.to_path_buf(), None);
    prefix.name = name.into();
    prefix
}

/// [`windows_staged`] naming the prefix `origin`.
fn staged_from(origin: PrefixOrigin) -> StagedGame {
    let mut staged = windows_staged();
    staged.meta.prefix = Some(origin);
    staged
}

/// Writes Foo's save into `prefix` of `user`, where [`windows_staged`]'s root resolves.
fn foo_save_in(prefix: &Path, user: &str) {
    let users = prefix.join("drive_c/users").join(user);
    write(&users.join("AppData/Roaming/Foo/save.dat"), "foo");
}

/// A Linux machine whose home is `root/home`, with Bottles bottles named after their folders.
fn with_bottles(root: &Path, bottles: &[&Path]) -> Environment {
    let mut env = linux_env(&root.join("home"), vec![]);
    env.launcher_prefixes = (bottles.iter())
        .map(|bottle| {
            let name = bottle.file_name().unwrap().to_str().unwrap();
            launcher_named(Launcher::Bottles, bottle, name)
        })
        .collect();
    env
}

/// The default targets of Foo, a game the manifest does not know.
fn defaults(env: &Environment, config: &AppConfig, staged: Option<&StagedGame>) -> DefaultTargets {
    default_targets("Foo", &Manifest::default(), config, env, staged)
}

fn bottles(name: &str) -> PrefixOrigin {
    PrefixOrigin::Bottles(name.into())
}

#[test]
fn default_targets_uses_the_prefix_the_backup_came_from() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let library = tmp.path().join("library");
    let bottle = make_prefix(&tmp.path().join("bottle"), "me");
    let heroic = make_prefix(&tmp.path().join("heroic"), "me");
    let lutris = make_prefix(&tmp.path().join("lutris"), "me");
    let proton = make_prefix(&library.join("steamapps/compatdata/620/pfx"), "steamuser");
    let wine = make_prefix(&home.join("Games/wine"), "me");
    let mut env = linux_env(&home, vec![]);
    env.launcher_prefixes = vec![
        launcher_named(Launcher::Bottles, &bottle, "Generic-Game"),
        launcher_named(Launcher::Heroic, &heroic, "Salmon"),
        launcher_named(Launcher::Lutris, &lutris, "foo-game"),
    ];
    let config = config(vec![Root {
        path: library,
        kind: RootKind::Steam,
    }]);
    let cases = [
        (bottles("Generic-Game"), &bottle, "me"),
        (PrefixOrigin::Heroic("Salmon".into()), &heroic, "me"),
        (PrefixOrigin::Lutris("foo-game".into()), &lutris, "me"),
        (PrefixOrigin::Proton(620), &proton, "steamuser"),
        (PrefixOrigin::Home("<home>/Games/wine".into()), &wine, "me"),
    ];
    for (origin, prefix, user) in cases {
        let found = defaults(&env, &config, Some(&staged_from(origin.clone())));
        assert_eq!(
            found.targets.win,
            Some(in_prefix(prefix, user)),
            "{origin:?}"
        );
        assert!(found.targets.linux.is_some());
        assert_eq!(found.reason, Some(PrefixReason::Backup), "{origin:?}");
        assert_eq!(found.origin.as_ref(), Some(&origin));
        assert!(!found.stored_missing, "{origin:?}");
    }
}

#[test]
fn stored_prefix_missing_here_searches_the_saves() {
    let tmp = tempfile::tempdir().unwrap();
    let other = make_prefix(&tmp.path().join("Other"), "me");
    foo_save_in(&other, "me");
    let env = with_bottles(tmp.path(), &[&other]);

    let found = defaults(&env, &config(vec![]), Some(&staged_from(bottles("Gone"))));
    assert_eq!(found.targets.win, Some(in_prefix(&other, "me")));
    assert_eq!(found.reason, Some(PrefixReason::Saves));
    assert_eq!(found.origin, Some(bottles("Other")));
    assert!(found.stored_missing);
}

#[test]
fn snapshot_without_prefix_finds_the_bottle_holding_its_saves() {
    let tmp = tempfile::tempdir().unwrap();
    let bottles_dir = tmp.path().join("bottles");
    let empty = make_prefix(&bottles_dir.join("Empty"), "me");
    let generic = make_prefix(&bottles_dir.join("Generic-Game"), "me");
    let roaming = generic.join("drive_c/users/me/AppData/Roaming");
    write(
        &roaming.join("Godot/app_userdata/Tower Wizard/save.tres"),
        "x",
    );
    let env = with_bottles(tmp.path(), &[&empty, &generic]);
    let root = "Godot/app_userdata/Tower Wizard";
    let staged = StagedGame {
        root: "/stage/tower-wizard".into(),
        meta: SnapshotMeta::new(
            layout::SNAPSHOT_FORMAT,
            "Tower Wizard".into(),
            vec![(Base::WinAppData, root.into())],
        ),
        files: vec![(Base::WinAppData, format!("{root}/save.tres"))],
        registry: None,
    };

    let found = default_targets(
        "Tower Wizard",
        &Manifest::default(),
        &config(vec![]),
        &env,
        Some(&staged),
    );
    assert_eq!(found.targets.win, Some(in_prefix(&generic, "me")));
    assert_eq!(found.reason, Some(PrefixReason::Saves));
    assert!(!found.stored_missing);
}

#[test]
fn search_resolves_osusername_to_the_prefix_user() {
    let tmp = tempfile::tempdir().unwrap();
    let mine = make_prefix(&tmp.path().join("mine"), "steamuser");
    let other = make_prefix(&tmp.path().join("other"), "me");
    // Only `mine`'s user matches the folder below `Foo`.
    for prefix in [&mine, &other] {
        let user = if prefix == &mine { "steamuser" } else { "me" };
        let local = prefix
            .join("drive_c/users")
            .join(user)
            .join("AppData/Local");
        write(&local.join("Foo/steamuser/s.sav"), "x");
    }
    let env = with_bottles(tmp.path(), &[&other, &mine]);
    let mut staged = windows_staged();
    staged.meta.roots = vec![(Base::WinLocalAppData, "Foo/<osUserName>".into())];
    // Backed up on a machine whose user was `olduser`.
    staged.files = vec![(Base::WinLocalAppData, "Foo/olduser/s.sav".into())];

    let found = defaults(&env, &config(vec![]), Some(&staged));
    assert_eq!(found.targets.win, Some(in_prefix(&mine, "steamuser")));
}

#[test]
fn search_matches_folder_case_insensitively() {
    let tmp = tempfile::tempdir().unwrap();
    let bottle = make_prefix(&tmp.path().join("bottle"), "me");
    let roaming = bottle.join("drive_c/users/me/AppData/Roaming");
    write(
        &roaming.join("godot/App_UserData/tower wizard/save.tres"),
        "x",
    );
    let env = with_bottles(tmp.path(), &[&bottle]);
    let mut staged = windows_staged();
    staged.meta.roots = vec![(Base::WinAppData, "Godot/app_userdata/Tower Wizard".into())];
    let save = "Godot/app_userdata/Tower Wizard/save.tres";
    staged.files = vec![(Base::WinAppData, save.into())];

    let found = defaults(&env, &config(vec![]), Some(&staged));
    assert_eq!(found.targets.win, Some(in_prefix(&bottle, "me")));
}

#[test]
fn search_looks_for_the_files_folders_not_a_generic_root() {
    let tmp = tempfile::tempdir().unwrap();
    let other_game = make_prefix(&tmp.path().join("other-game"), "me");
    let foo = make_prefix(&tmp.path().join("foo"), "me");
    let documents = |prefix: &Path| prefix.join("drive_c/users/me/Documents");
    write(&documents(&other_game).join("My Games/Other/x.sav"), "x");
    write(&documents(&foo).join("My Games/Foo/save.sav"), "x");
    let env = with_bottles(tmp.path(), &[&other_game, &foo]);
    let mut staged = windows_staged();
    staged.meta.roots = vec![(Base::WinDocuments, "My Games".into())];
    staged.files = vec![(Base::WinDocuments, "My Games/Foo/save.sav".into())];

    let found = defaults(&env, &config(vec![]), Some(&staged));
    assert_eq!(found.targets.win, Some(in_prefix(&foo, "me")));
    assert_eq!(found.reason, Some(PrefixReason::Saves));
}

#[test]
fn search_skips_an_empty_root_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let bottle = make_prefix(&tmp.path().join("bottle"), "me");
    let folder = bottle.join("drive_c/users/me/AppData/Roaming/Foo");
    fs::create_dir_all(folder).unwrap();
    let env = with_bottles(tmp.path(), &[&bottle]);

    let found = defaults(&env, &config(vec![]), Some(&windows_staged()));
    assert_eq!(found.targets.win, None);
    assert_eq!(found.reason, None);
    assert_eq!(found.ambiguous, Vec::<PathBuf>::new());
}

#[test]
fn search_finds_several_and_chooses_none() {
    let tmp = tempfile::tempdir().unwrap();
    let first = make_prefix(&tmp.path().join("first"), "me");
    let second = make_prefix(&tmp.path().join("second"), "me");
    foo_save_in(&first, "me");
    foo_save_in(&second, "me");
    let env = with_bottles(tmp.path(), &[&first, &second]);

    let found = defaults(&env, &config(vec![]), Some(&windows_staged()));
    assert_eq!(found.targets.win, None);
    assert_eq!(found.reason, None);
    assert_eq!(found.ambiguous, [first, second]);
}

#[test]
fn search_counts_a_prefix_under_two_steam_roots_once() {
    let tmp = tempfile::tempdir().unwrap();
    let steam = tmp.path().join("steam");
    let pfx = make_prefix(&steam.join("steamapps/compatdata/999/pfx"), "steamuser");
    foo_save_in(&pfx, "steamuser");
    let env = linux_env(&tmp.path().join("home"), vec![steam.clone()]);
    // The same library, configured again under another spelling of its path.
    let config = config(vec![Root {
        path: steam.join("steamapps/.."),
        kind: RootKind::Steam,
    }]);

    let found = defaults(&env, &config, Some(&windows_staged()));
    assert_eq!(found.targets.win, Some(in_prefix(&pfx, "steamuser")));
    assert_eq!(found.origin, Some(PrefixOrigin::Proton(999)));
}

#[test]
fn stored_prefix_without_a_user_falls_through() {
    let tmp = tempfile::tempdir().unwrap();
    let unused = tmp.path().join("Generic-Game");
    fs::create_dir_all(unused.join("drive_c/users")).unwrap();
    write(&unused.join("user.reg"), "WINE REGISTRY Version 2\n");
    let other = make_prefix(&tmp.path().join("Other"), "me");
    foo_save_in(&other, "me");
    let env = with_bottles(tmp.path(), &[&unused, &other]);

    let staged = staged_from(bottles("Generic-Game"));
    let found = defaults(&env, &config(vec![]), Some(&staged));
    assert_eq!(found.targets.win, Some(in_prefix(&other, "me")));
    assert_eq!(found.reason, Some(PrefixReason::Saves));
    assert!(found.stored_missing);
}

#[test]
fn custom_wine_prefix_comes_before_the_stored_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let bottle = make_prefix(&tmp.path().join("Generic-Game"), "me");
    let custom = make_prefix(&tmp.path().join("custom"), "me");
    let env = with_bottles(tmp.path(), &[&bottle]);
    let mut game = CustomGame::new("Foo".into(), GameEntry::default(), Integration::Extend);
    game.wine_prefix = Some(custom.clone());
    let mut config = config(vec![]);
    config.custom_games = vec![game];

    let staged = staged_from(bottles("Generic-Game"));
    let found = defaults(&env, &config, Some(&staged));
    assert_eq!(found.targets.win, Some(in_prefix(&custom, "me")));
    assert_eq!(found.reason, Some(PrefixReason::CustomGame));
    assert!(!found.stored_missing);
}

#[test]
fn own_prefix_comes_before_the_stored_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let steam = tmp.path().join("steam");
    let pfx = make_prefix(&steam.join("steamapps/compatdata/620/pfx"), "steamuser");
    let bottle = make_prefix(&tmp.path().join("Generic-Game"), "me");
    let mut env = with_bottles(tmp.path(), &[&bottle]);
    env.steam_roots = vec![steam];

    let staged = staged_from(bottles("Generic-Game"));
    let found = default_targets(
        "Foo",
        &steam_game(620),
        &config(vec![]),
        &env,
        Some(&staged),
    );
    assert_eq!(found.targets.win, Some(in_prefix(&pfx, "steamuser")));
    assert_eq!(found.reason, Some(PrefixReason::OwnPrefix));
}

#[test]
fn a_stored_name_of_several_prefixes_takes_the_one_holding_the_saves() {
    let tmp = tempfile::tempdir().unwrap();
    let native = make_prefix(&tmp.path().join("native/Generic-Game"), "me");
    let flatpak = make_prefix(&tmp.path().join("flatpak/Generic-Game"), "me");
    let other = make_prefix(&tmp.path().join("Other"), "me");
    let env = with_bottles(tmp.path(), &[&native, &flatpak, &other]);
    let staged = staged_from(bottles("Generic-Game"));

    // No prefix holds the saves: none is chosen, and the name's prefixes are named.
    let found = defaults(&env, &config(vec![]), Some(&staged));
    assert_eq!(found.targets.win, None);
    assert_eq!(found.ambiguous, Vec::<PathBuf>::new());
    assert_eq!(found.stored_several, [native.clone(), flatpak.clone()]);
    assert!(!found.stored_missing);

    // Neither bottle of the name holds them, so the search finds the one that does.
    foo_save_in(&other, "me");
    let found = defaults(&env, &config(vec![]), Some(&staged));
    assert_eq!(found.targets.win, Some(in_prefix(&other, "me")));
    assert_eq!(found.reason, Some(PrefixReason::Saves));
    assert_eq!(found.stored_several, Vec::<PathBuf>::new());

    foo_save_in(&flatpak, "me");
    let found = defaults(&env, &config(vec![]), Some(&staged));
    assert_eq!(found.targets.win, Some(in_prefix(&flatpak, "me")));
    assert_eq!(found.reason, Some(PrefixReason::Backup));

    // Both hold them: none is chosen, both are named, and nothing is searched.
    foo_save_in(&native, "me");
    let found = defaults(&env, &config(vec![]), Some(&staged));
    assert_eq!(found.targets.win, None);
    assert_eq!(found.ambiguous, [native, flatpak]);
    assert_eq!(found.stored_several, Vec::<PathBuf>::new());
}

#[test]
fn stored_prefix_comes_before_the_search() {
    let tmp = tempfile::tempdir().unwrap();
    let stored = make_prefix(&tmp.path().join("Generic-Game"), "me");
    let saves = make_prefix(&tmp.path().join("Saves"), "me");
    foo_save_in(&saves, "me");
    let env = with_bottles(tmp.path(), &[&saves, &stored]);

    let staged = staged_from(bottles("Generic-Game"));
    let found = defaults(&env, &config(vec![]), Some(&staged));
    assert_eq!(found.targets.win, Some(in_prefix(&stored, "me")));
    assert_eq!(found.reason, Some(PrefixReason::Backup));
}

#[test]
fn stored_and_searched_prefixes_skip_ignored_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let stored = make_prefix(&tmp.path().join("Generic-Game"), "me");
    let saves = make_prefix(&tmp.path().join("Saves"), "me");
    foo_save_in(&saves, "me");
    let env = with_bottles(tmp.path(), &[&stored, &saves]);
    let mut config = config(vec![]);
    config.ignored_prefixes = vec![stored, saves];

    let staged = staged_from(bottles("Generic-Game"));
    let found = defaults(&env, &config, Some(&staged));
    assert_eq!(found.targets.win, None);
    assert_eq!(found.ambiguous, Vec::<PathBuf>::new());
    assert!(found.stored_missing);
}

#[test]
fn stored_home_prefix_with_dotdot_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    make_prefix(&tmp.path().join("outside"), "me");
    let env = linux_env(&tmp.path().join("home"), vec![]);
    fs::create_dir_all(tmp.path().join("home")).unwrap();

    for rest in ["<home>/../outside", "<home>//../outside", "/outside"] {
        let staged = staged_from(PrefixOrigin::Home(rest.into()));
        let found = defaults(&env, &config(vec![]), Some(&staged));
        assert_eq!(found.targets.win, None, "{rest}");
        assert!(found.stored_missing, "{rest}");
    }
}

#[test]
fn default_targets_without_a_snapshot_is_todays_default() {
    let tmp = tempfile::tempdir().unwrap();
    let bottle = make_prefix(&tmp.path().join("bottle"), "me");
    foo_save_in(&bottle, "me");
    let env = with_bottles(tmp.path(), &[&bottle]);

    let found = defaults(&env, &config(vec![]), None);
    assert_eq!(found.targets.win, None);
    assert_eq!(found.reason, None);
    assert!(!found.stored_missing);
}

#[test]
fn describe_prefix_names_a_proton_prefix_by_its_app_id() {
    let pfx = Path::new("/games/steamapps/compatdata/1868140/pfx");
    let env = linux_env(Path::new("/home/me"), vec![]);
    let described = describe_prefix(pfx, "Foo", &Manifest::default(), &config(vec![]), &env);
    assert_eq!(described, Some(PrefixOrigin::Proton(1_868_140)));
}

#[test]
fn a_bottle_backup_restores_into_that_bottle_without_cross_build() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = init_repo(tmp.path());
    let home = tmp.path().join("home");
    let bottle = make_prefix(
        &home.join(".local/share/bottles/bottles/Generic-Game"),
        "me",
    );
    let roaming = bottle.join("drive_c/users/me/AppData/Roaming");
    write(
        &roaming.join("Godot/app_userdata/Tower Wizard/save.tres"),
        "x",
    );
    let manifest = crate::manifest::parse_yaml(
        "Tower Wizard:\n  files:\n    <winAppData>/Godot/app_userdata/Tower Wizard: {}\n",
    )
    .unwrap();
    let env = with_bottles(tmp.path(), &[&bottle]);
    let config = config(vec![]);
    let report = crate::scan::scan(&manifest, &config, &env, |_, _| {});
    assert_eq!(report.problems, []);
    back_up(&mut storage, tmp.path(), &report.games, &[]);
    let game = list(&storage).unwrap().remove(0);
    let snapshot = &game.snapshots[game.default.unwrap()];
    let listed = super::read(&mut storage, snapshot, &tmp.path().join("work")).unwrap();
    assert_eq!(listed.staged.meta.prefix, Some(bottles("Generic-Game")));

    let found = default_targets(
        "Tower Wizard",
        &manifest,
        &config,
        &env,
        Some(&listed.staged),
    );
    assert_eq!(found.targets.win, Some(in_prefix(&bottle, "me")));
    assert_eq!(found.reason, Some(PrefixReason::Backup));
    let info = game_info("Tower Wizard", &manifest, &[]);
    let vars = game_vars("Tower Wizard", info.entry.as_ref(), &config, &env);
    let plan = super::super::prepare(&listed, &found.targets, &vars, &info, &[]).unwrap();
    assert!(!plan.cross_build);
    let save = roaming.join("Godot/app_userdata/Tower Wizard/save.tres");
    assert_eq!(plan.writes[0].target, save);
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
    let plan = super::super::prepare(&listed_of(&staged), &targets, &vars, &info, &[]).unwrap();
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
        default_targets(
            "Foo",
            &steam_game(620),
            &config(vec![]),
            &env,
            Some(&staged_from(PrefixOrigin::Proton(620)))
        )
        .targets,
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
        self.run_probing(plan, || wineserver_running)
    }

    fn run_probing(
        &mut self,
        plan: &RestorePlan,
        wineserver_running: impl Fn() -> bool,
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
        registry_kept: vec![],
        skipped: vec![],
        cross_build: false,
        notes: vec![],
        deletes: vec![],
        placed: vec![],
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
fn nothing_is_written_when_a_wineserver_starts_during_the_safety_backup() {
    let mut f = fixture();
    user_reg(&f.prefix, "old");
    let target = write(&f.folders.app_data.join("Game/save.sav"), "old");
    let writes = vec![f.planned("Game/save.sav", "new")];
    let plan = plan_of(writes, Some((foo_registry("new"), f.ctx())));

    // Quiet at the early check, running once the safety backup is made.
    let asked = std::cell::Cell::new(0);
    let result = f.run_probing(&plan, || {
        asked.set(asked.get() + 1);
        asked.get() > 1
    });
    assert!(matches!(result, Err(RunError::WineserverRunning)));
    assert_eq!(read(&target), "old");
    assert_eq!(reg(&f.prefix, FOO), foo_registry("old"));
    // The backup it made stays.
    assert_eq!(f.storage.snapshots("salvajocs").unwrap().len(), 1);
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
fn a_removed_registry_root_is_neither_replaced_nor_backed_up() {
    const OTHER: &str = "HKEY_CURRENT_USER\\Software\\Other";
    let mut f = fixture();
    user_reg(&f.prefix, "old");
    let before = reg(&f.prefix, OTHER);
    let mut export = foo_registry("new");
    let value = RegValue::new("keep".into(), RegData::Sz("new".into()));
    export.keys.insert(OTHER.into(), vec![value]);
    export.keys.insert(format!("{OTHER}\\Sub"), Vec::new());

    let unticked = [OTHER.to_owned(), format!("{OTHER}\\Sub")];
    let (write, kept) = selected(&export, &unticked);
    assert_eq!(kept, unticked);
    let plan = plan_kept(write, kept, f.ctx());
    let report = f.run(&plan, false).unwrap();

    assert!(matches!(report.registry, Some(Ok(()))));
    assert_eq!(reg(&f.prefix, FOO), foo_registry("new"));
    assert_eq!(
        reg(&f.prefix, OTHER),
        before,
        "the removed root is left alone"
    );
    let dir = f.restored(created(&report));
    let registry: RegistryExport =
        serde_json::from_slice(&fs::read(dir.join(REGISTRY_FILE)).unwrap()).unwrap();
    assert_eq!(
        registry,
        foo_registry("old"),
        "only the kept root is backed up"
    );
}

/// What a restore of `export` writes and keeps with the nodes `unticked` (by key) unticked, as
/// the GUI asks for it.
fn selected(export: &RegistryExport, unticked: &[String]) -> (RegistryExport, Vec<String>) {
    let tree = export.tree();
    let ticked: Vec<bool> = (tree.iter())
        .map(|node| !unticked.contains(&node.key))
        .collect();
    export.select(&tree, &ticked)
}

/// A registry-only plan writing `write` into `ctx` and keeping `kept`.
fn plan_kept(write: RegistryExport, kept: Vec<String>, ctx: Context) -> RestorePlan {
    let mut plan = plan_of(Vec::new(), Some((write, ctx)));
    plan.registry_kept = kept;
    plan
}

/// `key` relative to HKCU.
fn hkcu(key: &str) -> String {
    format!("HKEY_CURRENT_USER\\{key}")
}

/// The block of `key` (relative to HKCU, `\`-separated) with the value `name` = `value`, as
/// [`FOO_TREE`] writes it.
fn old_block(key: &str, name: &str, value: &str) -> String {
    let key = key.replace('\\', "\\\\");
    format!("[{key}] 1\n#time=0\n\"{name}\"=\"{value}\"\n")
}

/// `Software\Foo` with `KEEP` (spelled in another case than the backup), `Keep\Deep`,
/// `Keep\Deep\Back`, `Keep\OnlyHere`, `OnlyHere`, `Swap` and `Gap\Leaf` without a `Gap` block,
/// then `Software\Other`.
const FOO_TREE: [(&str, &str); 9] = [
    (r"Software\Foo", "v"),
    (r"Software\Foo\Gap\Leaf", "l"),
    (r"Software\Foo\KEEP", "k"),
    (r"Software\Foo\Keep\Deep", "d"),
    (r"Software\Foo\Keep\Deep\Back", "b"),
    (r"Software\Foo\Keep\OnlyHere", "o"),
    (r"Software\Foo\OnlyHere", "o"),
    (r"Software\Foo\Swap", "s"),
    (r"Software\Other", "keep"),
];

/// Writes [`FOO_TREE`] with every value `old` as the `user.reg` of `prefix`.
fn foo_tree(prefix: &Path) {
    let blocks: Vec<String> = (FOO_TREE.iter())
        .map(|(key, name)| old_block(key, name, "old"))
        .collect();
    let text = format!("WINE REGISTRY Version 2\n\n{}", blocks.join("\n"));
    write(&prefix.join("user.reg"), &text);
}

/// The backup's export of `Software\Foo`: `Foo`, `Gap\Leaf`, `Keep`, `Keep\Deep`,
/// `Keep\Deep\Back`, `Swap` and `NewKey`, each with `x` = `new`.
fn foo_backup() -> RegistryExport {
    let keys = [
        r"Software\Foo",
        r"Software\Foo\Gap\Leaf",
        r"Software\Foo\Keep",
        r"Software\Foo\Keep\Deep",
        r"Software\Foo\Keep\Deep\Back",
        r"Software\Foo\NewKey",
        r"Software\Foo\Swap",
    ];
    let mut export = RegistryExport::default();
    for key in keys {
        let value = RegValue::new("x".into(), RegData::Sz("new".into()));
        export.keys.insert(hkcu(key), vec![value]);
    }
    export
}

/// The registry of the safety backup `report` made.
fn safety_registry(f: &mut Fixture, report: &RestoreReport) -> RegistryExport {
    let dir = f.restored(created(report));
    serde_json::from_slice(&fs::read(dir.join(REGISTRY_FILE)).unwrap()).unwrap()
}

/// Restores [`foo_backup`] over [`foo_tree`] with `Keep` and `Keep\Deep` unticked. Returns the
/// report and `Software\Foo` as it was before.
fn partial_restore(f: &mut Fixture) -> (RestoreReport, RegistryExport) {
    partial_restore_unticking(f, &[r"Software\Foo\Keep", r"Software\Foo\Keep\Deep"])
}

/// Restores [`foo_backup`] over [`foo_tree`] with the nodes `unticked` (relative to HKCU)
/// unticked. Returns the report and `Software\Foo` as it was before.
fn partial_restore_unticking(
    f: &mut Fixture,
    unticked: &[&str],
) -> (RestoreReport, RegistryExport) {
    foo_tree(&f.prefix);
    let before = reg(&f.prefix, FOO);
    let unticked: Vec<String> = unticked.iter().map(|key| hkcu(key)).collect();
    let (write, kept) = selected(&foo_backup(), &unticked);
    assert_eq!(kept, unticked);
    let report = f.run(&plan_kept(write, kept, f.ctx()), false).unwrap();
    assert!(matches!(report.registry, Some(Ok(()))), "{report:?}");
    (report, before)
}

/// Plans the safety backup `report` made into the fixture's prefix and restores it with the
/// nodes `unticked` unticked.
fn undo(f: &mut Fixture, report: &RestoreReport, unticked: &[String]) {
    let (_, plan) = read_and_plan(f, created(report));
    let Some((export, ctx)) = plan.registry else {
        panic!("the safety backup has no registry: {plan:?}");
    };
    let (write, kept) = selected(&export, unticked);
    let report = f.run(&plan_kept(write, kept, ctx), false).unwrap();
    assert!(matches!(report.registry, Some(Ok(()))), "{report:?}");
}

#[test]
fn a_partial_registry_restore_leaves_unticked_subkeys_alone() {
    let mut f = fixture();

    let (report, before) = partial_restore(&mut f);

    let text = read(&f.prefix.join("user.reg"));
    for (key, name) in [
        (r"Software\Foo\KEEP", "k"),
        (r"Software\Foo\Keep\Deep", "d"),
        (r"Software\Foo\Keep\OnlyHere", "o"),
        (r"Software\Other", "keep"),
    ] {
        assert!(
            text.contains(&old_block(key, name, "old")),
            "{key}:\n{text}"
        );
    }
    let after = reg(&f.prefix, FOO);
    let new = foo_backup();
    for key in [
        r"Software\Foo",
        r"Software\Foo\Gap\Leaf",
        r"Software\Foo\Keep\Deep\Back",
        r"Software\Foo\NewKey",
        r"Software\Foo\Swap",
    ] {
        assert_eq!(
            after.keys.get(&hkcu(key)),
            new.keys.get(&hkcu(key)),
            "{key}"
        );
    }
    assert!(!after.keys.contains_key(&hkcu(r"Software\Foo\OnlyHere")));
    assert!(!text.contains("[Software\\\\Foo\\\\Gap]"), "{text}");
    // F1-A: the ticked keys' whole subtrees, the unticked ones below included.
    assert_eq!(safety_registry(&mut f, &report), before);
}

#[test]
fn undo_of_a_partial_registry_restore_restores_the_old_state_and_nothing_else() {
    let mut f = fixture();
    let (report, before) = partial_restore(&mut f);
    let other = reg(&f.prefix, &hkcu(r"Software\Other"));

    undo(&mut f, &report, &[]);

    assert_eq!(reg(&f.prefix, FOO), before);
    assert_eq!(reg(&f.prefix, &hkcu(r"Software\Other")), other);
    let text = read(&f.prefix.join("user.reg"));
    assert!(
        !text.contains("NewKey") && !text.contains("[Software\\\\Foo\\\\Gap]"),
        "{text}"
    );
}

#[test]
fn undo_removes_a_key_created_under_an_unticked_key() {
    let mut f = fixture();
    // `NewKey` is not on this PC; its parent `Foo` keeps its values.
    let (report, before) = partial_restore_unticking(&mut f, &[r"Software\Foo"]);
    assert!(
        reg(&f.prefix, FOO)
            .keys
            .contains_key(&hkcu(r"Software\Foo\NewKey"))
    );
    // Foo whole; the written added `Foo\Gap` may show as an empty key besides.
    let safety = safety_registry(&mut f, &report);
    for (key, values) in &before.keys {
        assert_eq!(safety.keys.get(key), Some(values), "{key}");
    }

    undo(&mut f, &report, &[]);

    assert_eq!(reg(&f.prefix, FOO), before);
}

/// Writes the blocks of `keys` (relative to HKCU, each with `v` = `old`) as the `user.reg` of
/// `prefix`, and returns `Software\Foo` as it then is.
fn user_reg_of(prefix: &Path, keys: &[&str]) -> RegistryExport {
    let blocks: Vec<String> = keys.iter().map(|key| old_block(key, "v", "old")).collect();
    let text = format!("WINE REGISTRY Version 2\n\n{}", blocks.join("\n"));
    write(&prefix.join("user.reg"), &text);
    reg(prefix, FOO)
}

/// An export of `keys` (relative to HKCU), each with `v` = `new`.
fn new_export(keys: &[&str]) -> RegistryExport {
    let mut export = RegistryExport::default();
    for key in keys {
        let value = RegValue::new("v".into(), RegData::Sz("new".into()));
        export.keys.insert(hkcu(key), vec![value]);
    }
    export
}

#[test]
fn undo_removes_a_key_created_under_a_kept_key_below_a_written_one() {
    let mut f = fixture();
    let before = user_reg_of(&f.prefix, &[r"Software\Foo", r"Software\Foo\K"]);
    let backup = new_export(&[r"Software\Foo", r"Software\Foo\K", r"Software\Foo\K\New"]);
    let (write, kept) = selected(&backup, &[hkcu(r"Software\Foo\K")]);
    let report = f.run(&plan_kept(write, kept, f.ctx()), false).unwrap();
    assert!(
        reg(&f.prefix, FOO)
            .keys
            .contains_key(&hkcu(r"Software\Foo\K\New"))
    );

    undo(&mut f, &report, &[]);

    assert_eq!(reg(&f.prefix, FOO), before);
}

#[test]
fn undo_keeps_a_created_root_without_an_existing_ancestor_in_the_plan() {
    let mut f = fixture();
    // `Foo` exists but is above the plan's roots `Foo\A` and `Foo\B`.
    let before = user_reg_of(&f.prefix, &[r"Software\Foo", r"Software\Foo\A"]);
    let backup = new_export(&[r"Software\Foo\A", r"Software\Foo\B", r"Software\Foo\B\Sub"]);
    let (write, kept) = selected(&backup, &[hkcu(r"Software\Foo\B\Sub")]);
    let report = f.run(&plan_kept(write, kept, f.ctx()), false).unwrap();

    undo(&mut f, &report, &[]);

    let mut want = before;
    let created = hkcu(r"Software\Foo\B");
    want.keys
        .insert(created.clone(), backup.keys[&created].clone());
    assert_eq!(
        reg(&f.prefix, FOO),
        want,
        "documented: a created root stays"
    );
}

#[test]
fn undo_with_an_unticked_subkey_keeps_its_current_state() {
    let mut f = fixture();
    let (report, before) = partial_restore(&mut f);
    let path = f.prefix.join("user.reg");
    let changed = read(&path).replace("\"k\"=\"old\"", "\"k\"=\"changed\"");
    write(&path, &changed);

    undo(&mut f, &report, &[hkcu(r"Software\Foo\KEEP")]);

    let mut want = before;
    let keep = want.keys.get_mut(&hkcu(r"Software\Foo\KEEP")).unwrap();
    *keep = vec![RegValue::new("k".into(), RegData::Sz("changed".into()))];
    assert_eq!(reg(&f.prefix, FOO), want);
}

#[test]
fn a_restore_of_a_key_folder_without_its_values_writes_no_empty_block_and_undoes_exactly() {
    let mut f = fixture();
    let blocks = [
        old_block(r"Software\Foo", "v", "old"),
        old_block(r"Software\Foo\A\B", "b", "old"),
        old_block(r"Software\Foo\A\Only", "o", "old"),
    ];
    let text = format!("WINE REGISTRY Version 2\n\n{}", blocks.join("\n"));
    write(&f.prefix.join("user.reg"), &text);
    let before = reg(&f.prefix, FOO);
    let mut backup = foo_registry("new");
    let value = RegValue::new("b".into(), RegData::Sz("new".into()));
    backup.keys.insert(hkcu(r"Software\Foo\A\B"), vec![value]);

    // Foo's "(values)" row unticked: the added `Foo\A` owns `Only`.
    let (write, kept) = selected(&backup, &[FOO.to_owned()]);
    assert!(write.keys.contains_key(&hkcu(r"Software\Foo\A")));
    let report = f.run(&plan_kept(write, kept, f.ctx()), false).unwrap();

    let after = read(&f.prefix.join("user.reg"));
    assert!(after.contains(&blocks[0]), "{after}");
    assert!(
        !after.contains("Only") && !after.contains("[Software\\\\Foo\\\\A]"),
        "{after}"
    );
    assert_eq!(
        reg(&f.prefix, FOO).keys[&hkcu(r"Software\Foo\A\B")],
        backup.keys[&hkcu(r"Software\Foo\A\B")]
    );

    undo(&mut f, &report, &[]);

    assert_eq!(reg(&f.prefix, FOO), before);
    let undone = read(&f.prefix.join("user.reg"));
    assert!(!undone.contains("[Software\\\\Foo\\\\A]"), "{undone}");
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

#[cfg(unix)]
#[test]
fn restored_files_keep_the_mode_of_the_files_they_replace() {
    use std::os::unix::fs::PermissionsExt;

    let mut f = fixture();
    user_reg(&f.prefix, "old");
    let target = write(&f.folders.app_data.join("Game/save.sav"), "old");
    let user_reg = f.prefix.join("user.reg");
    // Modes the umask would not give a new file.
    for (path, mode) in [(&target, 0o640), (&user_reg, 0o600)] {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
    let writes = vec![f.planned("Game/save.sav", "new")];
    let plan = plan_of(writes, Some((foo_registry("new"), f.ctx())));

    let report = f.run(&plan, false).unwrap();
    assert!(report.files.iter().all(Result::is_ok), "{report:?}");
    assert!(matches!(report.registry, Some(Ok(()))));
    assert_eq!(read(&target), "new");
    for (path, mode) in [(&target, 0o640), (&user_reg, 0o600)] {
        let found = fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(found & 0o777, mode, "{}", path.display());
    }
}

fn tmp_of(target: &Path) -> PathBuf {
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(".salvajocs-tmp");
    tmp.into()
}

#[cfg(windows)]
#[test]
#[allow(clippy::permissions_set_readonly_false)]
fn a_failed_replace_of_a_read_only_file_leaves_no_temp_file() {
    use std::io::Write;
    use std::os::windows::fs::OpenOptionsExt;

    let dir = tempfile::tempdir().unwrap();
    let target = write(&dir.path().join("save.sav"), "old");
    let set_read_only = |read_only| {
        let mut permissions = fs::metadata(&target).unwrap().permissions();
        permissions.set_readonly(read_only);
        fs::set_permissions(&target, permissions).unwrap();
    };
    set_read_only(true);
    // Open without sharing, so the rename over it fails after the temp file was made.
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&target)
        .unwrap();
    assert!(replace(&target, |out| out.write_all(b"new")).is_err());
    assert!(!tmp_of(&target).exists());
    drop(held);

    // A temp file left read-only by an older version does not stop the next restore.
    write(&tmp_of(&target), "left");
    let mut permissions = fs::metadata(tmp_of(&target)).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(tmp_of(&target), permissions).unwrap();
    set_read_only(false);
    replace(&target, |out| out.write_all(b"new")).unwrap();
    assert_eq!(read(&target), "new");
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
        prefix_origin: None,
        unreadable: vec![],
        selection: BTreeMap::new(),
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

#[test]
fn backed_up_names_the_snapshots_file_with_its_listed_hash() {
    let mut f = fixture();
    let (listed, plan) = backed_up_plan(&mut f, &[("save.sav", "saved")]);

    let found = listed.backed_up(&plan.writes[0]).unwrap();

    assert_eq!(listed.slug(), Some(crate::slug::slug("Foo").as_str()));
    assert_eq!(found.snapshot, listed.snapshot);
    assert_eq!(found.path, "WinAppData/Game/save.sav");
    assert_eq!(found.size, 5);
    let hash = &listed.hashes.as_ref().unwrap().files["WinAppData/Game/save.sav"];
    assert_eq!(found.sha256.as_deref(), Some(hash.sha256.as_str()));
}

#[test]
fn backed_up_without_a_hash_list_has_no_hash() {
    let mut f = fixture();
    let stage = crafted_game_stage(f.tmp.path().join("stage"));
    write(&stage.join("files/WinAppData/Game/save.sav"), "saved");
    let snapshot = crafted_snapshot(&mut f.storage, &stage);
    let (listed, plan) = read_and_plan(&mut f, &snapshot);

    let found = listed.backed_up(&plan.writes[0]).unwrap();

    assert_eq!(found.sha256, None);
}

#[test]
fn backed_up_is_none_for_a_write_the_listing_lacks() {
    let mut f = fixture();
    let (listed, plan) = backed_up_plan(&mut f, &[("save.sav", "saved")]);
    let mut stray = plan.writes[0].clone();
    stray.source = f.tmp.path().join("elsewhere");

    assert_eq!(listed.backed_up(&stray), None);
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
        backup_sort: ListSort::default(),
        restore_sort: ListSort::default(),
        custom_games_repository: None,
        custom_games_unsent: Vec::new(),
        custom_games_not_now: None,
        retention_unsent: None,
        restore_deletes: Default::default(),
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
        &listed_of(&staged),
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
    let report = run(&mut storage, "Foo", &plan, &stage_root, || false).unwrap();

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
        &listed_of(&staged),
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
    let report = run(&mut storage, "Foo", &plan, &stage_root, || false).unwrap();

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

/// Foo backed up with `Game/save.sav`, downloaded and planned into the prefix, with the
/// deletions of everything else in `Game` ticked.
fn deleting_plan(f: &mut Fixture) -> (Listed, RestorePlan) {
    let (listed, mut plan) = backed_up_plan(f, &[("save.sav", "new")]);
    download(&mut f.storage, &listed).unwrap();
    let today = crate::selection::GameSettings::default();
    let found = super::super::deletions(&listed, &plan, &foo_vars(), &foo_info(), Some(&today));
    plan.deletes = found.files;
    (listed, plan)
}

#[test]
fn run_backs_up_a_file_before_deleting_it() {
    let mut f = fixture();
    let old = write(&f.folders.app_data.join("Game/old.sav"), "old bytes");
    let (_, plan) = deleting_plan(&mut f);
    let targets: Vec<&Path> = plan.deletes.iter().map(|d| d.target.as_path()).collect();
    assert_eq!(targets, [old.as_path()]);

    let report = f.run(&plan, false).unwrap();
    assert!(matches!(report.deletes[..], [Ok(())]), "{report:?}");
    assert!(!old.exists());
    assert_eq!(read(&f.folders.app_data.join("Game/save.sav")), "new");
    let dir = f.restored(created(&report));
    assert_eq!(
        read(&dir.join("files/WinAppData/Game/old.sav")),
        "old bytes"
    );
    assert!(
        staged_meta(&dir)
            .roots
            .contains(&(Base::WinAppData, "Game".into()))
    );
}

#[test]
fn undo_brings_back_a_deleted_file() {
    let mut f = fixture();
    let bytes = "old\r\nbytes\0";
    let old = write(&f.folders.app_data.join("Game/old.sav"), bytes);
    let (_, plan) = deleting_plan(&mut f);
    let report = f.run(&plan, false).unwrap();
    assert!(!old.exists());

    let work = f.tmp.path().join("undo");
    let listed = super::read(&mut f.storage, created(&report), &work).unwrap();
    download(&mut f.storage, &listed).unwrap();
    let undo = plan_into(&listed.staged, &f.ctx());
    let report = f.run(&undo, false).unwrap();
    assert!(report.files.iter().all(Result::is_ok), "{report:?}");
    assert_eq!(fs::read(&old).unwrap(), bytes.as_bytes());
}

#[test]
fn a_failed_safety_backup_deletes_nothing() {
    let mut f = fixture();
    let old = write(&f.folders.app_data.join("Game/old.sav"), "old");
    let (_, plan) = deleting_plan(&mut f);
    let repo = f.tmp.path().join("repo");
    fs::rename(&repo, f.tmp.path().join("moved")).unwrap();

    assert!(matches!(
        f.run(&plan, false),
        Err(RunError::SafetyBackup(BackupError::Storage(_)))
    ));
    assert_eq!(read(&old), "old");
}

#[cfg(unix)]
#[test]
fn an_unreadable_deletion_target_blocks_the_restore() {
    use std::os::unix::fs::PermissionsExt;

    let mut f = fixture();
    let old = write(&f.folders.app_data.join("Game/old.sav"), "old");
    let (listed, plan) = deleting_plan(&mut f);
    fs::set_permissions(&old, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&old).is_ok() {
        // Root reads it anyway.
        return;
    }

    assert!(compare_writes(&plan, &listed).is_err());
    assert!(matches!(
        f.run(&plan, false),
        Err(RunError::SafetyBackup(_))
    ));
    assert!(old.exists());
    assert!(!f.folders.app_data.join("Game/save.sav").exists());
    // Unticked, it blocks nothing.
    let unticked = RestorePlan {
        deletes: Vec::new(),
        ..plan
    };
    assert!(compare_writes(&unticked, &listed).is_ok());
}

#[test]
fn run_never_deletes_a_folder() {
    let mut f = fixture();
    let old = write(&f.folders.app_data.join("Game/old.sav"), "old");
    let (_, mut plan) = deleting_plan(&mut f);
    // The file became a folder since the plan.
    fs::remove_file(&old).unwrap();
    write(&old.join("inside.sav"), "inside");

    let report = f.run(&plan, false).unwrap();
    assert!(matches!(
        report.deletes[..],
        [Err(ApplyError::TargetNotAFile)]
    ));
    assert_eq!(read(&old.join("inside.sav")), "inside");
    // A file gone since the plan is no failure.
    fs::remove_dir_all(&old).unwrap();
    plan.writes.clear();
    let report = f.run(&plan, false).unwrap();
    assert!(matches!(report.deletes[..], [Ok(())]));
}

#[test]
fn run_keeps_a_file_a_write_of_the_restore_targets() {
    let mut f = fixture();
    let old = write(&f.folders.app_data.join("Game/old.sav"), "old");
    let (_, mut plan) = deleting_plan(&mut f);
    plan.override_target(0, old.clone()).unwrap();

    let report = f.run(&plan, false).unwrap();
    assert!(
        matches!(report.deletes[..], [Err(ApplyError::WrittenByTheRestore)]),
        "{report:?}"
    );
    assert_eq!(read(&old), "new");
}

#[test]
fn run_keeps_a_file_a_write_names_with_other_components() {
    // A `.` and a repeated separator, as a mix of `/` and `\` names one file on Windows.
    for spelling in ["./old.sav", "/old.sav"] {
        let mut f = fixture();
        let game = f.folders.app_data.join("Game");
        let old = write(&game.join("old.sav"), "old");
        let (_, mut plan) = deleting_plan(&mut f);
        let target = PathBuf::from(format!("{}/{spelling}", game.display()));
        plan.override_target(0, target).unwrap();

        let report = f.run(&plan, false).unwrap();
        assert!(
            matches!(report.deletes[..], [Err(ApplyError::WrittenByTheRestore)]),
            "{spelling}: {report:?}"
        );
        assert_eq!(read(&old), "new", "{spelling}");
    }
}

/// [`deleting_plan`] with Foo's registry too, so `run` asks whether a wineserver runs before
/// and after the safety backup.
fn deleting_plan_with_registry(f: &mut Fixture) -> RestorePlan {
    user_reg(&f.prefix, "old");
    let (_, mut plan) = deleting_plan(f);
    plan.registry = Some((foo_registry("new"), f.ctx()));
    plan
}

/// Runs `plan`, calling `after_backup` once the safety backup is made.
fn run_then(f: &mut Fixture, plan: &RestorePlan, after_backup: impl Fn()) -> RestoreReport {
    let asked = std::cell::Cell::new(0);
    f.run_probing(plan, || {
        asked.set(asked.get() + 1);
        if asked.get() == 2 {
            after_backup();
        }
        false
    })
    .unwrap()
}

#[test]
fn run_keeps_a_file_changed_after_the_safety_backup() {
    let mut f = fixture();
    let old = write(&f.folders.app_data.join("Game/old.sav"), "old");
    let plan = deleting_plan_with_registry(&mut f);

    let report = run_then(&mut f, &plan, || {
        // The same size: the mtime tells.
        write(&old, "OLD");
        set_mtime(&old, 1);
    });
    assert!(
        matches!(
            report.deletes[..],
            [Err(ApplyError::ChangedSinceSafetyBackup)]
        ),
        "{report:?}"
    );
    assert_eq!(read(&old), "OLD");
}

#[test]
fn run_keeps_a_file_created_after_the_safety_backup() {
    let mut f = fixture();
    let old = write(&f.folders.app_data.join("Game/old.sav"), "old");
    let plan = deleting_plan_with_registry(&mut f);
    fs::remove_file(&old).unwrap();

    let report = run_then(&mut f, &plan, || {
        write(&old, "created");
    });
    assert!(matches!(report.deletes[..], [Ok(())]), "{report:?}");
    assert_eq!(read(&old), "created");
}

#[cfg(unix)]
#[test]
fn run_refuses_a_file_to_delete_that_became_a_symlink() {
    let mut f = fixture();
    let old = write(&f.folders.app_data.join("Game/old.sav"), "old");
    let (_, plan) = deleting_plan(&mut f);
    let elsewhere = write(&f.tmp.path().join("elsewhere.sav"), "elsewhere");
    fs::remove_file(&old).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &old).unwrap();

    let report = f.run(&plan, false).unwrap();
    assert!(
        matches!(report.deletes[..], [Err(ApplyError::TargetIsSymlink)]),
        "{report:?}"
    );
    assert!(fs::symlink_metadata(&old).unwrap().is_symlink());
    assert_eq!(read(&elsewhere), "elsewhere");
}

#[test]
fn a_safety_backup_records_no_selection() {
    let mut f = fixture();
    write(&f.folders.app_data.join("Game/save.sav"), "old");
    let rules = BTreeMap::from([("WinAppData/Game/other".to_owned(), false)]);
    let game = GameScan {
        selection: rules.clone(),
        ..game_with_save(f.tmp.path(), "Foo", "new")
    };
    back_up(&mut f.storage, f.tmp.path(), &[game], &[]);
    let snapshot = list(&f.storage).unwrap()[0].snapshots[0].clone();
    let (listed, plan) = read_and_plan(&mut f, &snapshot);
    assert_eq!(listed.hashes.as_ref().unwrap().selection, rules);
    download(&mut f.storage, &listed).unwrap();

    let report = f.run(&plan, false).unwrap();
    let work = f.tmp.path().join("safety");
    let safety = super::read(&mut f.storage, created(&report), &work).unwrap();
    assert!(safety.hashes.unwrap().selection.is_empty());
}
