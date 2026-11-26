use std::collections::BTreeMap;
use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::*;
use crate::config::PasswordMode;
use crate::layout::{
    Base, FileHash, HASHES_FILE, HASHES_FORMAT, LinuxFolders, META_FILE, REGISTRY_FILE,
    SNAPSHOT_FORMAT, SnapshotMeta,
};
use crate::manifest::{CustomFile, CustomGame, GameEntry, Integration, Pair, parse_yaml};
use crate::registry::RegistryExport;
use crate::restore::StagedGame;
use crate::scan::{FoundFile, RegistrySource};
use crate::storage::{Compression, EntryKind, RepoLocation};

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// Writes `bytes` to `path` with mtime `modified`.
fn write(path: &Path, bytes: &[u8], modified: SystemTime) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    path.to_path_buf()
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A listed file of `size` bytes.
fn entry(path: &str, size: usize, modified: SystemTime) -> SnapshotFile {
    SnapshotFile {
        path: path.to_owned(),
        size: size as u64,
        modified: Some(modified),
        kind: EntryKind::File,
    }
}

/// A hash list with each `(path below files/, bytes)`.
fn hashes(files: &[(&str, &[u8])]) -> Hashes {
    Hashes {
        format: HASHES_FORMAT,
        meta: sha256(b"{}"),
        registry: None,
        files: files
            .iter()
            .map(|(path, bytes)| {
                let file = FileHash {
                    sha256: sha256(bytes),
                    size: bytes.len() as u64,
                    modified: None,
                };
                ((*path).to_owned(), file)
            })
            .collect::<BTreeMap<_, _>>(),
        selection: BTreeMap::new(),
    }
}

fn found(base: Base, tail: &str, real: &Path) -> FoundFile {
    let meta = fs::metadata(real).unwrap();
    FoundFile {
        base,
        tail: tail.to_owned(),
        real: real.to_path_buf(),
        size: meta.len(),
        modified: meta.modified().ok(),
    }
}

fn game(files: Vec<FoundFile>) -> GameScan {
    GameScan {
        title: "Game".into(),
        files,
        roots: Vec::new(),
        registry_keys: Vec::new(),
        registry_source: None,
        prefixes: Vec::new(),
        prefix_origin: None,
        unreadable: Vec::new(),
        selection: BTreeMap::new(),
    }
}

/// Compares `path` with an entry of `listed` bytes, stored at mtime `stored`, whose hash is
/// that of `listed` when `hashed`.
fn compare(path: &Path, listed: &[u8], stored: SystemTime, hashed: bool) -> FileState {
    let modified = fs::metadata(path).unwrap().modified().ok();
    let sha256 = hashed.then(|| sha256(listed));
    let entry = entry("a", listed.len(), stored);
    compare_file(path, modified, &entry, sha256.as_deref())
}

#[test]
fn a_different_size_is_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write(&tmp.path().join("a"), b"abcd", at(1));
    assert_eq!(compare(&path, b"abc", at(1), true), FileState::Changed);
}

#[test]
fn the_same_size_and_mtime_is_the_same_without_reading() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write(&tmp.path().join("a"), b"abc", at(1));
    // Another hash in the list: only reading the file would notice.
    assert_eq!(compare(&path, b"xyz", at(1), true), FileState::Same);
}

#[test]
fn the_same_bytes_with_another_mtime_are_the_same_by_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write(&tmp.path().join("a"), b"abc", at(2));
    assert_eq!(compare(&path, b"abc", at(1), true), FileState::Same);
}

#[test]
fn other_bytes_of_the_same_size_are_changed_by_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write(&tmp.path().join("a"), b"abd", at(2));
    assert_eq!(compare(&path, b"abc", at(1), true), FileState::Changed);
}

#[test]
fn without_a_hash_another_mtime_is_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write(&tmp.path().join("a"), b"abc", at(2));
    assert_eq!(compare(&path, b"abc", at(1), false), FileState::Changed);
    assert_eq!(compare(&path, b"abc", at(2), false), FileState::Same);
}

#[test]
fn a_missing_local_file_is_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("missing");
    let state = compare_file(&missing, None, &entry("a", 3, at(1)), None);
    assert!(matches!(state, FileState::Unknown(_)));
}

#[test]
fn files_compare_by_path_key_and_unmatched_entries_are_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let same = write(&tmp.path().join("same"), b"same", at(2));
    let changed = write(&tmp.path().join("changed"), b"new!", at(2));
    let new = write(&tmp.path().join("new"), b"new", at(1));
    let scan = game(vec![
        found(Base::Home, "same.sav", &same),
        found(Base::Home, "changed.sav", &changed),
        found(Base::Home, "new.sav", &new),
    ]);
    let baseline = [
        entry("files/Home/same.sav", 4, at(1)),
        entry("files/Home/changed.sav", 4, at(1)),
        entry("files/Home/gone.sav", 4, at(1)),
        entry("salvajocs.json", 2, at(1)),
    ];
    let list = hashes(&[
        ("Home/same.sav", b"same"),
        ("Home/changed.sav", b"old!"),
        ("Home/gone.sav", b"gone"),
    ]);
    let comparison = compare_game(&scan, &baseline, Some(&list));
    assert_eq!(
        comparison.found,
        [FileState::Same, FileState::Changed, FileState::New]
    );
    let gone: Vec<_> = comparison.gone.iter().map(|file| &file.path).collect();
    assert_eq!(gone, ["Home/gone.sav"]);
    let counts = counts(&scan, &comparison, &GameSettings::default()).unwrap();
    assert_eq!(
        counts,
        Counts {
            new: 1,
            changed: 1,
            removed: 1,
            unknown: 0
        }
    );

    // A snapshot without a hash list: every file with another mtime is changed.
    let comparison = compare_game(&scan, &baseline, None);
    assert_eq!(
        comparison.found,
        [FileState::Changed, FileState::Changed, FileState::New]
    );
}

#[test]
fn case_variants_match_below_windows_bases_only() {
    let tmp = tempfile::tempdir().unwrap();
    let save = write(&tmp.path().join("save"), b"save", at(2));
    let scan = game(vec![
        found(Base::WinAppData, "Foo/save.sav", &save),
        found(Base::XdgConfig, "Foo/save.sav", &save),
    ]);
    let baseline = [
        entry("files/WinAppData/foo/save.sav", 4, at(1)),
        entry("files/XdgConfig/foo/save.sav", 4, at(1)),
    ];
    // The hash is looked up by the entry's own path, not the found file's case.
    let list = hashes(&[
        ("WinAppData/foo/save.sav", b"save"),
        ("XdgConfig/foo/save.sav", b"save"),
    ]);
    let comparison = compare_game(&scan, &baseline, Some(&list));
    assert_eq!(comparison.found, [FileState::Same, FileState::New]);
    let gone: Vec<_> = comparison.gone.iter().map(|file| &file.path).collect();
    assert_eq!(gone, ["XdgConfig/foo/save.sav"]);
}

#[test]
fn an_exact_match_wins_over_a_case_variant() {
    let tmp = tempfile::tempdir().unwrap();
    let save = write(&tmp.path().join("save"), b"save", at(2));
    let scan = game(vec![found(Base::WinAppData, "foo/save.sav", &save)]);
    let baseline = [
        entry("files/WinAppData/FOO/save.sav", 4, at(1)),
        entry("files/WinAppData/foo/save.sav", 4, at(1)),
    ];
    let list = hashes(&[
        ("WinAppData/FOO/save.sav", b"SAVE"),
        ("WinAppData/foo/save.sav", b"save"),
    ]);
    let comparison = compare_game(&scan, &baseline, Some(&list));
    assert_eq!(comparison.found, [FileState::Same]);
    assert_eq!(comparison.gone.len(), 1);
}

#[test]
fn an_unticked_file_in_the_snapshot_is_removed_and_a_new_one_is_not_counted() {
    let tmp = tempfile::tempdir().unwrap();
    let kept = write(&tmp.path().join("kept"), b"kept", at(1));
    let new = write(&tmp.path().join("new"), b"new", at(1));
    let scan = game(vec![
        found(Base::Home, "kept.sav", &kept),
        found(Base::Home, "new.sav", &new),
    ]);
    let baseline = [entry("files/Home/kept.sav", 4, at(1))];
    let comparison = compare_game(&scan, &baseline, None);
    let mut settings = GameSettings::default();
    settings.set_path("Home/kept.sav", false);
    settings.set_path("Home/new.sav", false);
    // Nothing is left to back up, so no snapshot would be made.
    assert_eq!(counts(&scan, &comparison, &settings), None);

    let other = write(&tmp.path().join("other"), b"other", at(1));
    let mut scan = scan;
    scan.files.push(found(Base::Home, "other.sav", &other));
    let comparison = compare_game(&scan, &baseline, None);
    assert_eq!(
        counts(&scan, &comparison, &settings),
        Some(Counts {
            new: 1,
            changed: 0,
            removed: 1,
            unknown: 0
        })
    );
}

#[test]
fn a_game_with_unreadable_paths_removes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let save = write(&tmp.path().join("save"), b"save", at(1));
    let mut scan = game(vec![found(Base::Home, "save.sav", &save)]);
    scan.unreadable.push(tmp.path().join("locked"));
    let baseline = [
        entry("files/Home/save.sav", 4, at(1)),
        entry("files/Home/gone.sav", 4, at(1)),
    ];
    let comparison = compare_game(&scan, &baseline, None);
    let mut settings = GameSettings::default();
    assert_eq!(counts(&scan, &comparison, &settings).unwrap().removed, 0);
    settings.skip = true;
    assert_eq!(counts(&scan, &comparison, &settings), None);
}

fn snapshot(id: &str, host: &str, tags: &[&str]) -> SnapshotInfo {
    SnapshotInfo {
        id: id.into(),
        time: at(1),
        host: host.into(),
        label: "Game".into(),
        tags: tags.iter().map(|&tag| tag.to_owned()).collect(),
        size: None,
    }
}

#[test]
fn newest_skips_pre_restore_and_other_games_from_any_host() {
    let snapshots = [
        snapshot("other-game", "here", &["salvajocs", "game:other"]),
        snapshot("safety", "here", &["salvajocs", "game:game", "pre-restore"]),
        snapshot("elsewhere", "there", &["salvajocs", "game:game"]),
        snapshot("older", "here", &["salvajocs", "game:game"]),
    ];
    assert_eq!(newest(&snapshots, "game").unwrap().id, "elsewhere");
    assert_eq!(newest(&snapshots, "none"), None);
}

#[test]
fn an_unreadable_included_file_counts_as_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let save = write(&tmp.path().join("save"), b"save", at(2));
    let scan = game(vec![found(Base::Home, "save.sav", &save)]);
    let comparison = Comparison {
        found: vec![FileState::Unknown("denied".into())],
        gone: Vec::new(),
    };
    assert_eq!(
        counts(&scan, &comparison, &GameSettings::default()),
        Some(Counts {
            unknown: 1,
            ..Counts::default()
        })
    );
}

/// A listing with `files/Home/a.sav` (3 bytes at `at(1)`), `salvajocs.json` at `meta`, and
/// `hashes.json` at `written`, with the hash list that matches it.
fn listed(meta: u64, written: u64) -> (Vec<SnapshotFile>, Hashes) {
    let listing = vec![
        entry("files/Home/a.sav", 3, at(1)),
        entry(META_FILE, 2, at(meta)),
        entry(HASHES_FILE, 100, at(written)),
    ];
    let mut hashes = hashes(&[("Home/a.sav", b"abc")]);
    hashes.files.get_mut("Home/a.sav").unwrap().modified = Some(Duration::from_secs(1));
    (listing, hashes)
}

#[test]
fn a_hash_list_is_trusted_only_when_it_describes_the_snapshot() {
    let (listing, list) = listed(5, 5);
    assert_eq!(verified(list.clone(), &listing), Some(list.clone()));

    let mut other_mtime = list.clone();
    other_mtime.files.get_mut("Home/a.sav").unwrap().modified = Some(Duration::from_secs(2));
    let mut other_size = list.clone();
    other_size.files.get_mut("Home/a.sav").unwrap().size = 4;
    let mut no_mtime = list.clone();
    no_mtime.files.get_mut("Home/a.sav").unwrap().modified = None;
    let mut extra = list.clone();
    extra
        .files
        .insert("Home/b.sav".into(), list.files["Home/a.sav"].clone());
    let mut missing = list.clone();
    missing.files.clear();
    let mut registry = list.clone();
    registry.registry = Some("hash".into());
    for stale in [other_mtime, other_size, no_mtime, extra, missing, registry] {
        assert_eq!(verified(stale.clone(), &listing), None, "{stale:?}");
    }

    // `salvajocs.json` rewritten after the list, as an older version does.
    let (newer_meta, list) = listed(6, 5);
    assert_eq!(verified(list.clone(), &newer_meta), None);
    // A `registry.json` the list does not hash.
    let (mut listing, list) = listed(5, 5);
    listing.push(entry(REGISTRY_FILE, 2, at(5)));
    assert_eq!(verified(list, &listing), None);
}

/// This machine as a native Linux user whose home is `home`.
fn linux_env(home: &Path) -> Environment {
    Environment {
        native: Some(Context::Linux(LinuxFolders::new(
            "me".into(),
            home.to_path_buf(),
            home.join(".local/share"),
            home.join(".config"),
        ))),
        ..Environment::default()
    }
}

/// A config whose custom game `Game` extends the manifest with `pairs` and the file `rows`.
fn config(pairs: &[(&str, &str)], rows: &[(&str, &str)]) -> AppConfig {
    let mut game = CustomGame::new("Game".into(), GameEntry::default(), Integration::Extend);
    game.pairs = (pairs.iter())
        .map(|&(linux, windows)| Pair {
            linux: linux.into(),
            windows: windows.into(),
        })
        .collect();
    game.files = (rows.iter())
        .map(|&(linux, windows)| CustomFile {
            linux: Some(linux.into()),
            windows: Some(windows.into()),
        })
        .collect();
    let mut config = AppConfig::new("/repo".into(), PasswordMode::None);
    config.custom_games = vec![game];
    config
}

/// A backup of `Game` with `roots` and each `(path key, bytes)` as a file, as `restore::read`
/// gives it, with a hash list.
fn backup(roots: &[(Base, &str)], files: &[(&str, &[u8])]) -> Listed {
    let staged_files = (files.iter())
        .map(|(key, _)| {
            let (base, tail) = key.split_once('/').unwrap();
            (restore::parse_base(base).unwrap(), tail.to_owned())
        })
        .collect();
    let roots = (roots.iter()).map(|&(base, rest)| (base, rest.to_owned()));
    let meta = SnapshotMeta::new(SNAPSHOT_FORMAT, "Game".into(), roots.collect());
    let staged = StagedGame::new("baseline".into(), meta, staged_files, None);
    let listing = (files.iter())
        .map(|(key, bytes)| entry(&format!("files/{key}"), bytes.len(), at(5)))
        .collect();
    let snapshot = snapshot("theirs", "there", &["salvajocs", "game:game"]);
    Listed::new(snapshot, staged, listing, Some(hashes(files)))
}

/// A scan of `Game` with `roots` and `files`, found in `prefixes`.
fn scan(roots: &[(Base, &str)], files: Vec<FoundFile>, prefixes: &[&Path]) -> GameScan {
    let mut scan = game(files);
    scan.roots = (roots.iter())
        .map(|&(base, rest)| (base, rest.to_owned()))
        .collect();
    scan.prefixes = prefixes.iter().map(|prefix| prefix.to_path_buf()).collect();
    scan
}

/// `listed` mapped onto the build of `game` as [`Mapper::baseline`] maps it.
fn map(mapper: &Mapper, listed: &Listed, game: &GameScan) -> Option<Mapped> {
    let (family, targets) = mapper.targets(game, &listed.listing)?;
    mapper.map(listed, game, family, &targets)
}

/// The paths of `files`.
fn paths(files: &[SnapshotFile]) -> Vec<&str> {
    files.iter().map(|file| file.path.as_str()).collect()
}

/// Factorio's templates: two Linux save folders, so no auto-pair, and one Windows one.
fn factorio() -> Manifest {
    parse_yaml(
        "Game:
  files:
    <base>/game/saves:
      tags: [save]
      when:
        - os: linux
    <home>/.factorio/saves:
      tags: [save]
      when:
        - os: linux
    <home>/.factorio/config:
      when:
        - os: linux
    <winAppData>/Factorio/saves:
      tags: [save]
      when:
        - os: windows
    <winAppData>/Factorio/config:
      when:
        - os: windows
",
    )
    .unwrap()
}

#[test]
fn a_windows_backup_compares_with_a_linux_scan_through_a_saved_pair() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let save = write(&home.join(".factorio/saves/a.zip"), b"save", at(100));
    let game = scan(
        &[(Base::Home, ".factorio/saves")],
        vec![found(Base::Home, ".factorio/saves/a.zip", &save)],
        &[],
    );
    let listed = backup(
        &[(Base::WinAppData, "Factorio/saves")],
        &[("WinAppData/Factorio/saves/a.zip", b"save")],
    );
    let config = config(
        &[("<home>/.factorio/saves", "<winAppData>/Factorio/saves")],
        &[],
    );
    let (manifest, env) = (Manifest::default(), linux_env(&home));
    let mapper = Mapper::new(&manifest, &config, &env);

    let mapped = map(&mapper, &listed, &game).unwrap();
    assert_eq!(paths(&mapped.listing), ["files/Home/.factorio/saves/a.zip"]);
    assert!(mapped.unplaced.is_empty() && mapped.unplaced_roots.is_empty());
    assert!(mapped.entries.is_empty(), "nothing to ask about");
    let comparison = compare_game(&game, &mapped.listing, mapped.hashes.as_ref());
    assert_eq!(
        comparison,
        Comparison::new(vec![FileState::Same], Vec::new())
    );
}

#[test]
fn a_linux_backup_compares_with_a_prefix_scan_through_a_row_pair() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = tmp.path().join("pfx");
    let user = prefix.join("drive_c/users/steamuser");
    let save = write(&user.join("AppData/Roaming/G/save.dat"), b"save", at(100));
    write(
        &prefix.join("user.reg"),
        b"WINE REGISTRY Version 2\n",
        at(1),
    );
    let game = scan(
        &[(Base::WinAppData, "G")],
        vec![found(Base::WinAppData, "G/save.dat", &save)],
        &[&prefix],
    );
    let listed = backup(&[(Base::Home, "G")], &[("Home/G/save.dat", b"save")]);
    let config = config(&[], &[("<home>/G", "<winAppData>/G")]);
    // No native folders: the target is the scan's prefix.
    let (manifest, env) = (Manifest::default(), Environment::default());
    let mapper = Mapper::new(&manifest, &config, &env);

    let mapped = map(&mapper, &listed, &game).unwrap();
    assert_eq!(paths(&mapped.listing), ["files/WinAppData/G/save.dat"]);
    let comparison = compare_game(&game, &mapped.listing, mapped.hashes.as_ref());
    assert_eq!(comparison.found, [FileState::Same]);
}

#[test]
fn an_engine_rule_maps_without_any_config() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let save = write(&home.join(".config/unity3d/C/P/s.dat"), b"save", at(100));
    let game = scan(
        &[(Base::XdgConfig, "unity3d/C/P")],
        vec![found(Base::XdgConfig, "unity3d/C/P/s.dat", &save)],
        &[],
    );
    let listed = backup(
        &[(Base::WinLocalAppDataLow, "C/P")],
        &[("WinLocalAppDataLow/C/P/s.dat", b"save")],
    );
    let manifest = parse_yaml("Game:\n  files:\n    <xdgConfig>/unity3d/C/P: {}\n").unwrap();
    let config = AppConfig::new("/repo".into(), PasswordMode::None);
    let env = linux_env(&home);
    let mapper = Mapper::new(&manifest, &config, &env);

    let mapped = map(&mapper, &listed, &game).unwrap();
    assert_eq!(
        paths(&mapped.listing),
        ["files/XdgConfig/unity3d/C/P/s.dat"]
    );
}

#[test]
fn an_unknown_root_stays_at_its_snapshot_path() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let options = write(&home.join(".factorio/config/o.ini"), b"opts", at(100));
    let game = scan(
        &[(Base::Home, ".factorio/config")],
        vec![found(Base::Home, ".factorio/config/o.ini", &options)],
        &[],
    );
    let listed = backup(
        &[
            (Base::WinAppData, "Factorio/config"),
            (Base::WinAppData, "Factorio/saves"),
        ],
        &[
            ("WinAppData/Factorio/config/o.ini", b"opts"),
            ("WinAppData/Factorio/saves/a.zip", b"save"),
        ],
    );
    let config = config(
        &[("<home>/.factorio/config", "<winAppData>/Factorio/config")],
        &[],
    );
    let (manifest, env) = (factorio(), linux_env(&home));
    let mapper = Mapper::new(&manifest, &config, &env);

    let mapped = map(&mapper, &listed, &game).unwrap();
    let saves = (Base::WinAppData, "Factorio/saves".to_owned());
    assert_eq!(mapped.unplaced_roots, std::slice::from_ref(&saves));
    // What the mapping dialog would show: the saved pair prefilled, the saves to choose.
    let asked: Vec<_> = (mapped.entries.iter())
        .map(|entry| (entry.root.clone(), entry.confidence))
        .collect();
    let config_root = (Base::WinAppData, "Factorio/config".to_owned());
    assert_eq!(
        asked,
        [
            (config_root, Confidence::Saved),
            (saves, Confidence::Unknown)
        ]
    );
    let unplaced = "WinAppData/Factorio/saves/a.zip".to_owned();
    assert_eq!(mapped.unplaced, BTreeSet::from([unplaced]));
    let comparison = compare_game(&game, &mapped.listing, mapped.hashes.as_ref());
    assert_eq!(comparison.found, [FileState::Same]);
    assert_eq!(paths(&comparison.gone), ["WinAppData/Factorio/saves/a.zip"]);
}

#[test]
fn an_undeclared_root_of_the_other_build_is_unplaced() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let save = write(&home.join("g/a.sav"), b"save", at(100));
    let game = scan(
        &[(Base::Home, "g")],
        vec![found(Base::Home, "g/a.sav", &save)],
        &[],
    );
    let listed = backup(
        &[(Base::WinAppData, "G"), (Base::WinDocuments, "Elsewhere")],
        &[
            ("WinAppData/G/a.sav", b"save"),
            ("WinDocuments/Elsewhere/x", b"x"),
        ],
    );
    let config = config(&[("<home>/g", "<winAppData>/G")], &[]);
    let (manifest, env) = (Manifest::default(), linux_env(&home));
    let mapper = Mapper::new(&manifest, &config, &env);

    let mapped = map(&mapper, &listed, &game).unwrap();
    let elsewhere = (Base::WinDocuments, "Elsewhere".to_owned());
    assert_eq!(mapped.unplaced_roots, [elsewhere]);
    let unplaced = "WinDocuments/Elsewhere/x".to_owned();
    assert_eq!(mapped.unplaced, BTreeSet::from([unplaced]));
}

#[test]
fn a_file_removed_since_a_mapped_backup_is_gone_at_this_machines_path() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let save = write(&home.join("g/a.sav"), b"save", at(100));
    let game = scan(
        &[(Base::Home, "g")],
        vec![found(Base::Home, "g/a.sav", &save)],
        &[],
    );
    let listed = backup(
        &[(Base::WinAppData, "G")],
        &[
            ("WinAppData/G/a.sav", b"save"),
            ("WinAppData/G/b.sav", b"old"),
        ],
    );
    let config = config(&[("<home>/g", "<winAppData>/G")], &[]);
    let (manifest, env) = (Manifest::default(), linux_env(&home));
    let mapper = Mapper::new(&manifest, &config, &env);

    let mapped = map(&mapper, &listed, &game).unwrap();
    let comparison = compare_game(&game, &mapped.listing, mapped.hashes.as_ref());
    assert_eq!(paths(&comparison.gone), ["Home/g/b.sav"]);
    assert!(mapped.unplaced.is_empty());
}

#[test]
fn two_files_mapped_onto_one_path_leave_the_second_unplaced() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let save = write(&home.join("g/a.sav"), b"save", at(100));
    let game = scan(
        &[(Base::Home, "g")],
        vec![found(Base::Home, "g/a.sav", &save)],
        &[],
    );
    let listed = backup(
        &[(Base::WinAppData, "G"), (Base::WinDocuments, "G")],
        &[
            ("WinAppData/G/a.sav", b"save"),
            ("WinDocuments/G/a.sav", b"other"),
        ],
    );
    let config = config(
        &[
            ("<home>/g", "<winAppData>/G"),
            ("<home>/g", "<winDocuments>/G"),
        ],
        &[],
    );
    let (manifest, env) = (Manifest::default(), linux_env(&home));
    let mapper = Mapper::new(&manifest, &config, &env);

    let mapped = map(&mapper, &listed, &game).unwrap();
    assert_eq!(
        paths(&mapped.listing),
        ["files/Home/g/a.sav", "files/WinDocuments/G/a.sav"]
    );
    let unplaced = "WinDocuments/G/a.sav".to_owned();
    assert_eq!(mapped.unplaced, BTreeSet::from([unplaced]));
    // Its folder has a place; only this file does not.
    assert!(mapped.unplaced_roots.is_empty());
}

#[test]
fn a_proton_scan_and_a_windows_backup_are_the_same_build() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = tmp.path().join("pfx");
    let user = prefix.join("drive_c/users/steamuser");
    let save = write(&user.join("AppData/Roaming/G/a.sav"), b"save", at(100));
    let game = scan(
        &[(Base::WinAppData, "G")],
        vec![found(Base::WinAppData, "G/a.sav", &save)],
        &[&prefix],
    );
    let listed = backup(
        &[(Base::WinAppData, "G")],
        &[("WinAppData/G/a.sav", b"save")],
    );
    let config = config(&[("<home>/g", "<winAppData>/G")], &[]);
    let (manifest, env) = (Manifest::default(), linux_env(tmp.path()));
    let mapper = Mapper::new(&manifest, &config, &env);

    assert_eq!(map(&mapper, &listed, &game), None);
}

#[test]
fn a_scan_of_both_builds_is_not_mapped() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let save = write(&home.join("g/a.sav"), b"save", at(100));
    let game = scan(
        &[(Base::Home, "g"), (Base::WinAppData, "G")],
        vec![found(Base::Home, "g/a.sav", &save)],
        &[],
    );
    let listed = backup(
        &[(Base::WinAppData, "G")],
        &[("WinAppData/G/a.sav", b"save")],
    );
    let config = config(&[("<home>/g", "<winAppData>/G")], &[]);
    let (manifest, env) = (Manifest::default(), linux_env(&home));
    let mapper = Mapper::new(&manifest, &config, &env);

    assert_eq!(map(&mapper, &listed, &game), None);
}

/// A staged hash list of this build with `files` and the `registry` hash.
fn staged(files: &[(&str, &[u8])], registry: Option<&str>) -> Hashes {
    let mut hashes = hashes(files);
    hashes.meta = sha256(b"this build's roots");
    hashes.registry = registry.map(Into::into);
    hashes
}

/// A baseline mapped to hold `files`, with `unplaced` keys and the `registry` hash.
fn mapped_baseline(files: &[(&str, &[u8])], unplaced: &[&str], registry: Option<&str>) -> Baseline {
    let mut hashes = hashes(files);
    hashes.registry = registry.map(Into::into);
    let listing: Vec<SnapshotFile> = (files.iter())
        .map(|(key, bytes)| entry(&format!("files/{key}"), bytes.len(), at(5)))
        .collect();
    let unplaced = unplaced.iter().map(|&key| key.to_owned()).collect();
    let hashes_of_mapped = Some(hashes.clone());
    let sources = BTreeMap::new();
    let mapped = Mapped::new(
        listing.clone(),
        hashes_of_mapped,
        sources,
        vec![],
        unplaced,
        vec![],
    );
    let snapshot = snapshot("theirs", "there", &["salvajocs", "game:game"]);
    Baseline::new(snapshot, listing, Some(hashes), Some(mapped))
}

#[test]
fn mapped_same_content_ignores_salvajocs_json_but_not_the_registry() {
    let files: &[(&str, &[u8])] = &[("Home/g/a.sav", b"save")];
    let baseline = mapped_baseline(files, &[], None);
    assert!(baseline.same_content(&staged(files, None)));
    assert!(!baseline.same_content(&staged(files, Some("keys"))));
    assert!(!baseline.same_content(&staged(&[("Home/g/a.sav", b"edit")], None)));
    assert!(!baseline.same_content(&staged(&[("Home/g/b.sav", b"save")], None)));
    let with_keys = mapped_baseline(files, &[], Some("keys"));
    assert!(with_keys.same_content(&staged(files, Some("keys"))));
    assert!(!with_keys.same_content(&staged(files, None)));
    // Unmapped, `salvajocs.json` still counts.
    let mut same_build = baseline.clone();
    same_build.mapped = None;
    assert!(!same_build.same_content(&staged(files, None)));
}

#[test]
fn mapped_same_content_folds_case_below_windows_bases_only() {
    let windows = mapped_baseline(&[("WinAppData/G/Save.dat", b"save")], &[], None);
    assert!(windows.same_content(&staged(&[("WinAppData/g/save.dat", b"save")], None)));
    let linux = mapped_baseline(&[("Home/G/Save.dat", b"save")], &[], None);
    assert!(!linux.same_content(&staged(&[("Home/g/save.dat", b"save")], None)));
}

#[test]
fn mapped_same_content_needs_every_file_placed() {
    let files: &[(&str, &[u8])] = &[("Home/g/a.sav", b"save")];
    let baseline = mapped_baseline(files, &["WinAppData/H/b.sav"], None);
    assert!(!baseline.same_content(&staged(files, None)));
}

#[test]
fn mapped_same_content_needs_every_file_of_the_backup() {
    // This machine deleted `b.sav` since; skipping would lose that.
    let baseline = mapped_baseline(
        &[("Home/g/a.sav", b"save"), ("Home/g/b.sav", b"old")],
        &[],
        None,
    );
    assert!(!baseline.same_content(&staged(&[("Home/g/a.sav", b"save")], None)));
}

#[test]
fn mapped_same_content_matches_each_file_of_the_backup_once() {
    let baseline = mapped_baseline(
        &[("WinAppData/G/a", b"x"), ("WinAppData/G/b", b"y")],
        &[],
        None,
    );
    // Both case variants fold onto `a`, so `b` is matched by none.
    let staged = staged(&[("WinAppData/G/A", b"x"), ("WinAppData/G/a", b"x")], None);
    assert!(!baseline.same_content(&staged));
}

#[test]
fn skip_blocker_says_registry_when_either_side_has_keys() {
    let files: &[(&str, &[u8])] = &[("Home/g/a.sav", b"save")];
    let tmp = tempfile::tempdir().unwrap();
    let save = write(&tmp.path().join("a.sav"), b"save", at(1));
    let mut game = game(vec![found(Base::Home, "g/a.sav", &save)]);
    let baseline = mapped_baseline(files, &[], None);
    assert_eq!(baseline.skip_blocker(&game), None);
    assert_eq!(
        mapped_baseline(files, &[], Some("keys")).skip_blocker(&game),
        Some(NoSkip::Registry)
    );
    assert_eq!(
        mapped_baseline(files, &["WinAppData/H/b"], None).skip_blocker(&game),
        Some(NoSkip::Unmapped)
    );
    let key = "HKEY_CURRENT_USER/Software/G";
    game.registry_keys = vec![key.into()];
    game.registry_source = Some(RegistrySource::Windows);
    assert_eq!(baseline.skip_blocker(&game), Some(NoSkip::Registry));
    // Every key unticked: the backup stores no registry, so the skip holds.
    let mut settings = GameSettings::default();
    settings.registry.insert(key.into(), false);
    let applied = selection::apply(&game, &settings).unwrap();
    assert_eq!(baseline.skip_blocker(&applied), None);
    // The same build never previews the registry, as before.
    let mut same_build = baseline.clone();
    same_build.mapped = None;
    assert_eq!(same_build.skip_blocker(&game), None);
    same_build.hashes = None;
    assert_eq!(same_build.skip_blocker(&game), Some(NoSkip::NoHashList));
}

#[test]
fn the_baseline_maps_only_a_snapshot_of_the_other_build() {
    let tmp = tempfile::tempdir().unwrap();
    let location = RepoLocation::new(
        tmp.path().join("repo").to_str().unwrap().into(),
        BTreeMap::new(),
    );
    let mut storage = Storage::init(&location, "", Compression::Default).unwrap();
    let theirs = write(&tmp.path().join("theirs/save.sav"), b"save", at(100));
    let windows = scan(
        &[(Base::WinAppData, "G")],
        vec![found(Base::WinAppData, "G/save.sav", &theirs)],
        &[],
    );
    let stage_dir = tmp.path().join("stage");
    crate::backup::stage(&windows, &RegistryExport::default(), &stage_dir).unwrap();
    let slug = slug("Game");
    let tags = ["salvajocs", &format!("game:{slug}")];
    let as_path = format!("/salvajocs/{slug}");
    let newest = (storage.backup_dir(&stage_dir, &as_path, &tags, "Game", true))
        .unwrap()
        .unwrap();
    let home = tmp.path().join("home");
    let mine = write(&home.join("g/save.sav"), b"save", at(200));
    let linux = scan(
        &[(Base::Home, "g")],
        vec![found(Base::Home, "g/save.sav", &mine)],
        &[],
    );
    let config = config(&[("<home>/g", "<winAppData>/G")], &[]);
    let (manifest, env) = (Manifest::default(), linux_env(&home));
    let mapper = Mapper::new(&manifest, &config, &env);

    let same = mapper.baseline(&mut storage, &newest, &windows).unwrap();
    assert_eq!(same.mapped, None);
    assert!(same.hashes.is_some());
    let other = mapper.baseline(&mut storage, &newest, &linux).unwrap();
    assert_eq!(other.hashes, same.hashes);
    let mapped = other.mapped.as_ref().unwrap();
    assert_eq!(paths(&mapped.listing), ["files/Home/g/save.sav"]);
    let comparison = compare_game(&linux, other.compared().0, other.compared().1);
    assert_eq!(comparison.found, [FileState::Same]);
    // Read from the listing: the work dir is never made.
    assert!(!Path::new("baseline").exists());
}

/// `listed` as a baseline, mapped onto the build of `game` when `map` can.
fn baseline_of(listed: &Listed, mapper: &Mapper, game: &GameScan) -> Baseline {
    let mapped = map(mapper, listed, game);
    let (snapshot, listing) = (listed.snapshot.clone(), listed.listing.clone());
    Baseline::new(snapshot, listing, listed.hashes.clone(), mapped)
}

#[test]
fn backed_up_gives_the_snapshot_path_of_a_mapped_file() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let save = write(&home.join(".factorio/saves/a.zip"), b"save", at(100));
    let game = scan(
        &[(Base::Home, ".factorio/saves")],
        vec![found(Base::Home, ".factorio/saves/a.zip", &save)],
        &[],
    );
    let listed = backup(
        &[(Base::WinAppData, "Factorio/saves")],
        &[("WinAppData/Factorio/saves/a.zip", b"save")],
    );
    let config = config(
        &[("<home>/.factorio/saves", "<winAppData>/Factorio/saves")],
        &[],
    );
    let (manifest, env) = (Manifest::default(), linux_env(&home));
    let baseline = baseline_of(&listed, &Mapper::new(&manifest, &config, &env), &game);

    let backed_up = baseline.backed_up("Home/.factorio/saves/a.zip").unwrap();

    assert_eq!(backed_up.path, "WinAppData/Factorio/saves/a.zip");
    assert_eq!(backed_up.snapshot, listed.snapshot);
    assert_eq!(backed_up.size, 4);
    assert_eq!(backed_up.modified, Some(at(5)));
    assert_eq!(backed_up.sha256, Some(sha256(b"save")));
    assert_eq!(baseline.backed_up("Home/.factorio/saves/none.zip"), None);
}

#[test]
fn backed_up_finds_a_case_variant_below_windows_bases() {
    let files: &[(&str, &[u8])] = &[("WinAppData/G/Save.dat", b"x")];
    let listed = backup(&[(Base::WinAppData, "G")], files);
    let baseline = Baseline::new(listed.snapshot, listed.listing, listed.hashes, None);

    let backed_up = baseline.backed_up("WinAppData/g/save.dat").unwrap();

    assert_eq!(backed_up.path, "WinAppData/G/Save.dat");
    assert_eq!(baseline.backed_up("Home/g/save.dat"), None);
}

#[test]
fn backed_up_prefers_the_exact_key() {
    let files: &[(&str, &[u8])] = &[
        ("WinAppData/G/SAVE.dat", b"upper"),
        ("WinAppData/G/save.dat", b"lower"),
    ];
    let listed = backup(&[(Base::WinAppData, "G")], files);
    let baseline = Baseline::new(listed.snapshot, listed.listing, listed.hashes, None);

    let exact = baseline.backed_up("WinAppData/G/save.dat").unwrap();

    assert_eq!(exact.path, "WinAppData/G/save.dat");
    assert_eq!(exact.sha256, Some(sha256(b"lower")));
    // No exact entry: the first of the case variants.
    let folded = baseline.backed_up("WinAppData/G/Save.dat").unwrap();
    assert_eq!(folded.path, "WinAppData/G/SAVE.dat");
}
