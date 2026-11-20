use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use super::*;
use crate::config::{AppConfig, PasswordMode};
use crate::layout::Base;
use crate::manifest::Manifest;
use crate::registry::RegistryExport;
use crate::scan::{Environment, FoundFile, GameScan};
use crate::slug::slug;
use crate::storage::{Compression, RepoLocation, SnapshotInfo};

use super::super::{FileState, Mapper, compare_game};

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// Writes `bytes` to the file `name` of `dir` with mtime `modified`.
fn write(dir: &Path, name: &str, bytes: &[u8], modified: SystemTime) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    path
}

fn snapshot() -> SnapshotInfo {
    let tags = vec!["salvajocs".to_owned(), "game:game".to_owned()];
    SnapshotInfo::new("abc".into(), at(50), "there".into(), "Game".into(), tags)
}

/// The backup's copy of `bytes`, with its hash in the list when `listed`.
fn backed_up(bytes: &[u8], listed: bool) -> BackedUp {
    BackedUp {
        snapshot: snapshot(),
        path: "Home/G/save.dat".into(),
        size: bytes.len() as u64,
        modified: Some(at(10)),
        sha256: listed.then(|| sha256_bytes(bytes)),
    }
}

/// How the file `local` holding these bytes differs from the backup's `backup`.
fn compare(local: &[u8], backup: &[u8], listed: bool) -> FileDetails {
    let tmp = tempfile::tempdir().unwrap();
    let path = write(tmp.path(), "save.dat", local, at(20));
    details(
        &path,
        &backed_up(backup, listed),
        Some(backup),
        Some(TEXT_CAP),
    )
}

fn ends(crlf: usize, lf: usize) -> Option<LineEndings> {
    Some(LineEndings { crlf, lf, cr: 0 })
}

/// The lines of the diff `found` holds, with their kind and whether they miss a final newline.
fn drawn(found: FileDetails) -> Vec<(LineKind, String, bool)> {
    let Content::Diff(hunks) = found.content else {
        panic!("expected a diff, got {:?}", found.content);
    };
    (hunks.into_iter().flat_map(|hunk| hunk.lines))
        .map(|line| (line.kind, line.text, line.missing_newline))
        .collect()
}

#[test]
fn only_line_endings_differ() {
    let found = compare(b"a\r\nb\r\nc\r\n", b"a\nb\nc\n", true);

    let line_endings = Content::FormatOnly {
        line_endings: true,
        bom: false,
    };
    assert_eq!(found.content, line_endings);
    assert_eq!(found.local.unwrap().line_endings, ends(3, 0));
    assert_eq!(found.backup.line_endings, ends(0, 3));
}

#[test]
fn a_changed_line_is_one_hunk_with_context() {
    let lines = |middle: &str| {
        let lines = ["1", "2", "3", "4", middle, "6", "7", "8", "9", "10"];
        lines.map(|line| format!("{line}\n")).concat()
    };

    let found = compare(lines("new").as_bytes(), lines("5").as_bytes(), true);

    let Content::Diff(hunks) = found.content else {
        panic!("expected a diff, got {:?}", found.content);
    };
    assert_eq!(hunks.len(), 1, "the lines 1, 9 and 10 are out of context");
    let shown: Vec<_> = (hunks[0].lines.iter())
        .map(|line| (line.kind, line.text.as_str()))
        .collect();
    assert_eq!(
        shown,
        [
            (LineKind::Context, "2"),
            (LineKind::Context, "3"),
            (LineKind::Context, "4"),
            (LineKind::Removed, "5"),
            (LineKind::Added, "new"),
            (LineKind::Context, "6"),
            (LineKind::Context, "7"),
            (LineKind::Context, "8"),
        ]
    );
    let numbers = |kind| {
        (hunks[0].lines.iter())
            .find(|line| line.kind == kind)
            .map(|line| (line.backup_number, line.local_number))
    };
    assert_eq!(numbers(LineKind::Removed), Some((Some(5), None)));
    assert_eq!(numbers(LineKind::Added), Some((None, Some(5))));
}

#[test]
fn a_diff_reads_crlf_as_lf() {
    let found = compare(b"a\r\nB\r\n", b"a\nb\n", true);

    let Content::Diff(hunks) = found.content else {
        panic!("expected a diff, got {:?}", found.content);
    };
    let texts: Vec<_> = hunks[0]
        .lines
        .iter()
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(texts, ["a", "b", "B"]);
}

#[test]
fn a_missing_final_newline_is_marked() {
    // Without the mark, both sides would read as the same line `b`.
    let lines = drawn(compare(b"a\nb\n", b"a\nb", true));
    let (context, removed, added) = (LineKind::Context, LineKind::Removed, LineKind::Added);
    let expected = [
        (context, "a".to_owned(), false),
        (removed, "b".to_owned(), true),
        (added, "b".to_owned(), false),
    ];
    assert_eq!(lines, expected);
    // CRLF read as LF, the last line still has no ending on its side.
    assert_eq!(drawn(compare(b"a\nb\n", b"a\r\nb", true)), expected);
}

#[test]
fn a_lone_cr_is_a_line_ending_and_never_drawn() {
    let found = compare(b"a\rb\r", b"a\nb\n", true);
    assert_eq!(
        found.content,
        Content::FormatOnly {
            line_endings: true,
            bom: false
        }
    );
    let counted = LineEndings {
        crlf: 0,
        lf: 0,
        cr: 2,
    };
    assert_eq!(found.local.unwrap().line_endings, Some(counted));
    assert_eq!(found.backup.line_endings, ends(0, 2));

    // `\r\r\n` is a CR and a CRLF: two line endings, so two lines.
    let found = compare(b"a\r\r\nB\n", b"a\n\nb\n", true);
    let counted = found.local.as_ref().unwrap().line_endings;
    assert_eq!(
        counted,
        Some(LineEndings {
            crlf: 1,
            lf: 1,
            cr: 1
        })
    );
    let lines = drawn(found);
    assert!(lines.iter().all(|(_, text, _)| !text.contains('\r')));
    let texts: Vec<_> = lines.iter().map(|(_, text, _)| text.as_str()).collect();
    assert_eq!(texts, ["a", "", "b", "B"]);
}

#[test]
fn an_empty_copy_is_text() {
    let lines = drawn(compare(b"", b"a\n", true));
    assert_eq!(lines, [(LineKind::Removed, "a".to_owned(), false)]);
    let lines = drawn(compare(b"a\n", b"", true));
    assert_eq!(lines, [(LineKind::Added, "a".to_owned(), false)]);
    // Without a hash list, two empty files are found the same from their content.
    let found = compare(b"", b"", false);
    assert_eq!(found.content, Content::Same);
    assert_eq!(found.backup.sha256, BackupHash::Content(sha256_bytes(b"")));
    assert_eq!(found.local.unwrap().size, 0);
}

#[test]
fn a_byte_order_mark_on_one_side_is_named_and_left_out_of_the_diff() {
    let marked = "\u{feff}a\nb\n".as_bytes();

    let found = compare(marked, b"a\nb\n", true);
    assert_eq!(
        found.content,
        Content::FormatOnly {
            line_endings: false,
            bom: true
        }
    );
    assert!(found.local.as_ref().unwrap().bom);
    assert!(!found.backup.bom);

    let found = compare("\u{feff}a\r\nb\r\n".as_bytes(), b"a\nb\n", true);
    assert_eq!(
        found.content,
        Content::FormatOnly {
            line_endings: true,
            bom: true
        }
    );

    // A change next to a mark on one side: no line differs only by the mark.
    let lines = drawn(compare("\u{feff}a\nB\n".as_bytes(), b"a\nb\n", true));
    let texts: Vec<_> = lines.iter().map(|(_, text, _)| text.as_str()).collect();
    assert_eq!(texts, ["a", "b", "B"]);
    // Marks on both sides are no difference.
    let found = compare(marked, "\u{feff}a\nb\n".as_bytes(), true);
    assert_eq!(found.content, Content::Same);
}

#[test]
fn nul_or_invalid_utf8_is_not_text() {
    let nul = compare(b"a\0b", b"a\0c", true);
    assert_eq!(nul.content, Content::NotText);
    assert_eq!(nul.local.unwrap().line_endings, None);
    assert_eq!(nul.backup.line_endings, None);
    assert_eq!(compare(b"abc", b"\xff\xfe", true).content, Content::NotText);
    assert_eq!(compare(b"\xff\xfe", b"abc", true).content, Content::NotText);
}

/// A cap small enough to cross without a large file.
const CAP: u64 = 16;

#[test]
fn over_the_cap_reads_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let location = RepoLocation::new(
        tmp.path().join("repo").to_str().unwrap().into(),
        BTreeMap::new(),
    );
    let mut storage = Storage::init(&location, "", Compression::Default).unwrap();
    let small = write(tmp.path(), "small", b"abc", at(1));
    let mut copy = backed_up(b"abc", true);
    // No such file in the repository: reading it would be an error.
    assert!(read_backed_up(&mut storage, "game", &copy, &small, Some(CAP)).is_err());

    // Each of these would be that error too, if anything were read.
    let mut nothing_read = |copy: &BackedUp, local: &Path| {
        read_backed_up(&mut storage, "game", copy, local, Some(CAP))
            .unwrap()
            .is_none()
    };
    copy.size = CAP + 1;
    assert!(nothing_read(&copy, &small));
    let big = write(tmp.path(), "big", &vec![b'x'; CAP as usize + 1], at(1));
    let copy = backed_up(b"abc", true);
    assert!(nothing_read(&copy, &big));
    assert!(nothing_read(&copy, &tmp.path().join("gone")));

    let found = details(&big, &copy, None, Some(CAP));
    assert_eq!(found.content, Content::TooLarge);
    let local = found.local.unwrap();
    assert_eq!(local.size, CAP + 1);
    assert_eq!(local.line_endings, None);
    assert_eq!(found.backup.sha256, BackupHash::List(sha256_bytes(b"abc")));
}

#[test]
fn without_a_cap_a_large_file_is_read_and_diffed() {
    let tmp = tempfile::tempdir().unwrap();
    let location = RepoLocation::new(
        tmp.path().join("repo").to_str().unwrap().into(),
        BTreeMap::new(),
    );
    let mut storage = Storage::init(&location, "", Compression::Default).unwrap();
    let stage = tmp.path().join("stage");
    let saved = "line one is long enough to cross the cap\nline two\n";
    let content = stage.join("files/Home/G/save.dat");
    fs::create_dir_all(content.parent().unwrap()).unwrap();
    fs::write(&content, saved).unwrap();
    fs::write(stage.join("salvajocs.json"), "{}").unwrap();
    let tags = ["salvajocs", "game:game"];
    let id = storage.backup_dir(&stage, "/salvajocs/game", &tags, "Game");
    let snapshot = id.unwrap().unwrap();
    let mut copy = backed_up(saved.as_bytes(), true);
    copy.snapshot = snapshot;
    let local = write(
        tmp.path(),
        "save.dat",
        b"line one is long enough to cross the cap\n2\n",
        at(20),
    );

    let capped = read_backed_up(&mut storage, "game", &copy, &local, Some(CAP)).unwrap();
    assert_eq!(capped, None);
    assert_eq!(
        details(&local, &copy, None, Some(CAP)).content,
        Content::TooLarge
    );

    let bytes = read_backed_up(&mut storage, "game", &copy, &local, None).unwrap();
    assert_eq!(bytes.as_deref(), Some(saved.as_bytes()));
    let found = details(&local, &copy, bytes.as_deref(), None);
    let Content::Diff(hunks) = found.content else {
        panic!("{:?}", found.content);
    };
    let removed: Vec<_> = (hunks.iter().flat_map(|hunk| &hunk.lines))
        .filter(|line| line.kind == LineKind::Removed)
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(removed, ["line two"]);
}

#[test]
fn without_a_cap_a_large_binary_file_is_still_not_text() {
    let tmp = tempfile::tempdir().unwrap();
    let local = write(tmp.path(), "save.dat", &vec![0; CAP as usize + 1], at(20));
    let backup = vec![1; CAP as usize + 1];

    let found = details(&local, &backed_up(&backup, true), Some(&backup), None);

    assert_eq!(found.content, Content::NotText);
    assert_eq!(found.local.unwrap().size, CAP + 1);
}

#[test]
fn same_bytes_by_mtime_fallback_are_same() {
    // The backup has no hash list, so the file was only marked changed by its mtime.
    let found = compare(b"same\n", b"same\n", false);

    assert_eq!(found.content, Content::Same);
    assert_eq!(found.local.unwrap().sha256, sha256_bytes(b"same\n"));
    assert_eq!(
        found.backup.sha256,
        BackupHash::Content(sha256_bytes(b"same\n"))
    );
}

#[test]
fn the_backup_hash_comes_from_content_without_a_list() {
    let listed = compare(b"a\n", b"b\n", true);
    assert_eq!(listed.backup.sha256, BackupHash::List(sha256_bytes(b"b\n")));
    assert_eq!(
        listed.backup.sha256.hex(),
        Some(sha256_bytes(b"b\n").as_str())
    );

    let read = compare(b"a\n", b"b\n", false);
    assert_eq!(
        read.backup.sha256,
        BackupHash::Content(sha256_bytes(b"b\n"))
    );

    let tmp = tempfile::tempdir().unwrap();
    let path = write(tmp.path(), "save.dat", b"a\n", at(20));
    let unread = details(&path, &backed_up(b"b\n", false), None, Some(TEXT_CAP));
    assert_eq!(unread.backup.sha256, BackupHash::Unknown);
    assert_eq!(unread.backup.sha256.hex(), None);
    assert_eq!(unread.content, Content::TooLarge);
}

#[test]
fn an_unreadable_local_file_is_unavailable() {
    let tmp = tempfile::tempdir().unwrap();
    let gone = tmp.path().join("gone");
    let found = details(&gone, &backed_up(b"a", true), Some(b"a"), Some(TEXT_CAP));

    assert!(found.local.is_err());
    assert_eq!(found.content, Content::Unavailable);
    assert_eq!(found.backup.size, 1);
}

#[test]
fn a_backed_up_file_changed_since_gives_the_list_hash_the_node_mtime_and_the_diff() {
    let tmp = tempfile::tempdir().unwrap();
    let location = RepoLocation::new(
        tmp.path().join("repo").to_str().unwrap().into(),
        BTreeMap::new(),
    );
    let mut storage = Storage::init(&location, "", Compression::Default).unwrap();
    let saves = tmp.path().join("saves");
    fs::create_dir(&saves).unwrap();
    let local = write(&saves, "save.dat", b"one\ntwo\n", at(100));
    let file = |local: &Path, size: u64| {
        let modified = fs::metadata(local).unwrap().modified().ok();
        let tail = "saves/save.dat".into();
        FoundFile::new(Base::Home, tail, local.to_path_buf(), size, modified)
    };
    let mut game = GameScan {
        title: "Game".into(),
        roots: vec![(Base::Home, "saves".into())],
        files: vec![file(&local, 8)],
        ..GameScan::default()
    };
    let stage_dir = tmp.path().join("stage");
    crate::backup::stage(&game, &RegistryExport::default(), &stage_dir).unwrap();
    let slug = slug("Game");
    let tags = ["salvajocs", &format!("game:{slug}")];
    let as_path = format!("/salvajocs/{slug}");
    let newest = (storage.backup_dir(&stage_dir, &as_path, &tags, "Game"))
        .unwrap()
        .unwrap();
    write(&saves, "save.dat", b"one\n2\n", at(200));
    game.files = vec![file(&local, 6)];
    let (manifest, env) = (Manifest::default(), Environment::default());
    let config = AppConfig::new("/repo".into(), PasswordMode::None);
    let mapper = Mapper::new(&manifest, &config, &env);
    let baseline = mapper.baseline(&mut storage, &newest, &game).unwrap();
    let (listing, hashes) = baseline.compared();
    assert_eq!(
        compare_game(&game, listing, hashes).found,
        [FileState::Changed]
    );

    let copy = baseline.backed_up("Home/saves/save.dat").unwrap();
    let bytes = read_backed_up(&mut storage, &slug, &copy, &local, Some(TEXT_CAP)).unwrap();
    let found = details(&local, &copy, bytes.as_deref(), Some(TEXT_CAP));

    assert_eq!(
        found.backup.sha256,
        BackupHash::List(sha256_bytes(b"one\ntwo\n"))
    );
    assert_eq!(found.backup.modified, Some(at(100)));
    assert_eq!(found.backup.size, 8);
    assert_eq!(found.local.as_ref().unwrap().modified, Some(at(200)));
    let Content::Diff(hunks) = found.content else {
        panic!("expected a diff, got {:?}", found.content);
    };
    let changed: Vec<_> = (hunks[0].lines.iter())
        .filter(|line| line.kind != LineKind::Context)
        .map(|line| (line.kind, line.text.as_str()))
        .collect();
    assert_eq!(
        changed,
        [(LineKind::Removed, "two"), (LineKind::Added, "2")]
    );
}
