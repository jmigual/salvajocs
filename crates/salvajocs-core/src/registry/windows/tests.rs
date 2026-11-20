//! These run against the real HKCU, each in its own throwaway `Software\salvajocs-test-*` key.
//! None lists a key shared with another test (such as `Software`): tests run in parallel, and a
//! listing that races with a new key is exactly what the module guards against.

use std::process;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

/// Owns a unique test key under HKCU and deletes it on drop, also when the test fails.
struct TestKey {
    relative: String,
}

impl TestKey {
    fn new() -> TestKey {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        TestKey {
            relative: format!("SOFTWARE\\salvajocs-test-{}-{nanos}-{n}", process::id()),
        }
    }

    /// Full path of the key (`child` empty) or of `child` below it.
    fn path(&self, child: &str) -> String {
        match child {
            "" => format!("{HKCU}\\{}", self.relative),
            _ => format!("{HKCU}\\{}\\{child}", self.relative),
        }
    }

    fn export(&self) -> RegistryExport {
        export(&[self.path("")]).unwrap()
    }
}

impl Drop for TestKey {
    fn drop(&mut self) {
        // Best effort: the key may not exist, and a panic here would hide the test failure.
        let _ = CURRENT_USER.remove_tree(&self.relative);
    }
}

fn value(name: &str, data: RegData) -> RegValue {
    RegValue {
        name: name.into(),
        data,
    }
}

fn export_of(entries: Vec<(String, Vec<RegValue>)>) -> RegistryExport {
    RegistryExport {
        keys: entries.into_iter().collect(),
    }
}

/// What the crate itself reads back for each of `names`, one per line, for failure messages.
fn raw_values<const N: usize>(key: &Key, names: [&str; N]) -> String {
    names
        .map(|name| format!("{name}: {:?}", key.get_value(name)))
        .join("\n")
}

#[test]
fn windows_registry_round_trip() {
    let key = TestKey::new();
    // Values are in name order, as `export` returns them.
    let expected = export_of(vec![
        (key.path(""), vec![]),
        (
            key.path("Settings"),
            vec![
                value("", RegData::Sz("default".into())),
                value(
                    "big endian",
                    RegData::Other {
                        code: 5,
                        bytes: vec![0, 0, 0, 1],
                    },
                ),
                value("binary", RegData::Binary(vec![0, 1, 2, 255])),
                value("dword", RegData::Dword(0xdead_beef)),
                value("empty multi", RegData::MultiSz(vec![])),
                value("expand", RegData::ExpandSz("%TEMP%\\x".into())),
                value("multi", RegData::MultiSz(vec!["a".into(), "b c".into()])),
                value("qword", RegData::Qword(u64::MAX)),
                value("sz", RegData::Sz("caf\u{e9} \u{1f30e}".into())),
                value(
                    "unity double",
                    RegData::Other {
                        code: 4,
                        bytes: vec![1, 2, 3, 4, 5, 6, 7, 8],
                    },
                ),
            ],
        ),
        (
            key.path("Settings\\Deep"),
            vec![value("n", RegData::Dword(1))],
        ),
        (key.path("Empty"), vec![]),
    ]);

    replace_subtrees(&expected, &[]).unwrap();

    assert_eq!(key.export(), expected);
}

#[test]
fn replace_deletes_the_old_subtree() {
    let key = TestKey::new();
    replace_subtrees(
        &export_of(vec![
            (key.path(""), vec![value("stale", RegData::Dword(1))]),
            (key.path("Gone"), vec![value("x", RegData::Dword(2))]),
        ]),
        &[],
    )
    .unwrap();

    let new = export_of(vec![(
        key.path(""),
        vec![value("fresh", RegData::Dword(3))],
    )]);
    replace_subtrees(&new, &[]).unwrap();

    assert_eq!(key.export(), new);
}

#[test]
fn export_of_an_absent_key_is_empty() {
    let key = TestKey::new();
    assert_eq!(key.export(), RegistryExport::default());
}

#[test]
fn export_of_a_subkey_excludes_its_siblings() {
    let key = TestKey::new();
    replace_subtrees(
        &export_of(vec![
            (key.path(""), vec![]),
            (key.path("A"), vec![value("a", RegData::Dword(1))]),
            (key.path("B"), vec![value("b", RegData::Dword(2))]),
        ]),
        &[],
    )
    .unwrap();

    let only_a = export(&[key.path("A")]).unwrap();

    assert_eq!(only_a.keys.keys().collect::<Vec<_>>(), [&key.path("A")]);
}

#[test]
fn the_exported_root_keeps_the_callers_spelling_and_children_the_stored_one() {
    let key = TestKey::new();
    replace_subtrees(
        &export_of(vec![(
            key.path("Mixed\\Child"),
            vec![value("v", RegData::Dword(1))],
        )]),
        &[],
    )
    .unwrap();

    let root = key.path("mixed").to_uppercase();
    let found = export(&[&root]).unwrap();

    assert_eq!(
        found.keys.keys().collect::<Vec<_>>(),
        [&root, &format!("{root}\\Child")]
    );
}

#[test]
fn a_listing_is_returned_once_two_reads_agree() {
    let mut reads = [vec![1], vec![1, 2], vec![1, 2], vec![1, 2]].into_iter();
    let listed = stable("key", || Ok(reads.next().unwrap())).unwrap();
    assert_eq!(listed, [1, 2]);
}

#[test]
fn a_listing_that_never_settles_is_an_error() {
    let mut len = 0;
    let result = stable("key", || {
        len += 1;
        Ok(vec![0; len])
    });
    assert!(matches!(result, Err(WindowsRegistryError::Unstable { .. })));
}

#[test]
fn replace_leaves_siblings_and_prefix_sharing_keys_alone() {
    let key = TestKey::new();
    let sibling = |name: &str| (key.path(name), vec![value(name, RegData::Dword(9))]);
    replace_subtrees(
        &export_of(vec![
            sibling("A"),
            sibling("B"),
            sibling("AB"),
            (key.path("A\\Child"), vec![]),
        ]),
        &[],
    )
    .unwrap();

    let new_a = export_of(vec![(key.path("A"), vec![value("new", RegData::Dword(1))])]);
    replace_subtrees(&new_a, &[]).unwrap();

    let all = key.export();
    assert_eq!(all.keys[&key.path("A")], new_a.keys[&key.path("A")]);
    assert!(!all.keys.contains_key(&key.path("A\\Child")));
    assert_eq!(all.keys[&key.path("B")], sibling("B").1);
    assert_eq!(all.keys[&key.path("AB")], sibling("AB").1);
}

#[test]
fn values_written_by_the_library_export_typed() {
    let key = TestKey::new();
    let target = CURRENT_USER.create(&key.relative).unwrap();
    target.set_u32("dword", 7).unwrap();
    target.set_u64("qword", 1 << 40).unwrap();
    target.set_string("sz", "caf\u{e9}").unwrap();
    target.set_expand_string("expand", "%TEMP%").unwrap();
    target.set_multi_string("multi", &["a", "b"]).unwrap();
    target.set_multi_string("empty multi", &[]).unwrap();

    let values = key.export().keys.remove(&key.path("")).unwrap();

    assert_eq!(
        values,
        [
            value("dword", RegData::Dword(7)),
            value("empty multi", RegData::MultiSz(vec![])),
            value("expand", RegData::ExpandSz("%TEMP%".into())),
            value("multi", RegData::MultiSz(vec!["a".into(), "b".into()])),
            value("qword", RegData::Qword(1 << 40)),
            value("sz", RegData::Sz("caf\u{e9}".into())),
        ]
    );
}

#[test]
fn non_canonical_values_are_kept_as_other() {
    let key = TestKey::new();
    let target = CURRENT_USER.create(&key.relative).unwrap();
    // Type code and bytes. The string types have no terminating NUL, an interior NUL, an odd
    // length, no data at all, or multi-string data after its end; the last is a code above 11.
    let cases: [(&str, u32, &[u8]); 7] = [
        ("unterminated", 1, &[b'a', 0]),
        ("interior", 2, &[b'a', 0, 0, 0, b'b', 0, 0, 0]),
        ("odd", 1, &[b'a', 0, 0, 0, 7]),
        ("empty", 1, &[]),
        ("one byte", 2, &[0]),
        ("trailing", 7, &[b'a', 0, 0, 0, 0, 0, b'b', 0, 0, 0, 0, 0]),
        ("high type", 100, &[1, 2, 3]),
    ];
    for (name, code, bytes) in cases {
        // `Type::Other` bypasses the crate's debug assertion on terminated string data.
        target.set_bytes(name, Type::Other(code), bytes).unwrap();
    }

    let written = raw_values(&target, cases.map(|(name, ..)| name));
    let exported = key.export();
    for (name, code, bytes) in cases {
        let data = &exported.keys[&key.path("")]
            .iter()
            .find(|v| v.name == name)
            .unwrap()
            .data;
        assert_eq!(
            data,
            &RegData::Other {
                code,
                bytes: bytes.to_vec()
            }
        );
    }

    // Writing the export back leaves the bytes unchanged.
    replace_subtrees(&exported, &[]).unwrap();
    let restored = raw_values(
        &CURRENT_USER.open(&key.relative).unwrap(),
        cases.map(|(name, ..)| name),
    );
    let mut after = key.export();
    // Some Windows versions append a NUL when writing an unterminated REG_SZ (CI run 43938136740).
    for value in after.keys.get_mut(&key.path("")).unwrap() {
        if value.name == "unterminated" && value.data == RegData::Sz("a".into()) {
            value.data = RegData::Other {
                code: 1,
                bytes: vec![b'a', 0],
            };
        }
    }
    assert_eq!(
        after, exported,
        "raw values after the initial write:\n{written}\nraw values after replace_subtrees:\n{restored}"
    );
}

#[test]
fn invalid_keys_are_refused_before_anything_is_deleted() {
    let key = TestKey::new();
    let kept = export_of(vec![(key.path(""), vec![value("v", RegData::Dword(1))])]);
    replace_subtrees(&kept, &[]).unwrap();

    for bad in [
        HKCU.to_owned(),
        format!("{HKCU}\\"),
        key.path("A\\\\B"),
        format!("{}\\", key.path("")),
    ] {
        assert!(matches!(
            export(&[&bad]),
            Err(WindowsRegistryError::Key(RegistryError::InvalidKey { .. }))
        ));
        let mixed = export_of(vec![(key.path(""), vec![]), (bad, vec![])]);
        assert!(matches!(
            replace_subtrees(&mixed, &[]),
            Err(WindowsRegistryError::Key(RegistryError::InvalidKey { .. }))
        ));
    }
    assert!(matches!(
        export(&["HKEY_LOCAL_MACHINE\\Software"]),
        Err(WindowsRegistryError::Key(
            RegistryError::UnsupportedRoot { .. }
        ))
    ));

    assert_eq!(key.export(), kept);
}

/// The children of [`old_tree`]: `Here`, `Keep` (holding `Deep` and `Here`) and `Swap`.
const OLD_CHILDREN: [&str; 5] = ["Here", "Keep", "Keep\\Deep", "Keep\\Here", "Swap"];

fn old_values(child: &str) -> Vec<RegValue> {
    let old = value("old", RegData::Dword(1));
    match child {
        "" => vec![value("", RegData::Sz("default".into())), old],
        _ => vec![old],
    }
}

/// Writes the test key, with a default value, and [`OLD_CHILDREN`], each holding `old`.
fn old_tree(key: &TestKey) {
    let keys = std::iter::once("").chain(OLD_CHILDREN);
    let tree = export_of(
        keys.map(|child| (key.path(child), old_values(child)))
            .collect(),
    );
    replace_subtrees(&tree, &[]).unwrap();
}

/// An export of `children` of the test key (`""` for itself), each holding `new`.
fn new_values(key: &TestKey, children: &[&str]) -> RegistryExport {
    let new = || vec![value("new", RegData::Dword(2))];
    export_of(
        children
            .iter()
            .map(|child| (key.path(child), new()))
            .collect(),
    )
}

/// The test key's export after a replace: `old` children as [`old_tree`] wrote them, `new` ones
/// as [`new_values`] does, and `empty` ones without values.
fn expected(key: &TestKey, old: &[&str], new: &[&str], empty: &[&str]) -> RegistryExport {
    let mut want = new_values(key, new);
    for child in old {
        want.keys.insert(key.path(child), old_values(child));
    }
    for child in empty {
        want.keys.insert(key.path(child), Vec::new());
    }
    want
}

fn kept(key: &TestKey, children: &[&str]) -> Vec<String> {
    children.iter().map(|child| key.path(child)).collect()
}

#[test]
fn replace_keeps_an_unticked_subkey_with_its_values_and_subkeys() {
    let key = TestKey::new();
    old_tree(&key);

    replace_subtrees(&new_values(&key, &[""]), &kept(&key, &["Keep"])).unwrap();

    let old = ["Keep", "Keep\\Deep", "Keep\\Here"];
    assert_eq!(key.export(), expected(&key, &old, &[""], &[]));
}

#[test]
fn replace_writes_the_ticked_sibling_of_a_kept_key() {
    let key = TestKey::new();
    old_tree(&key);

    let write = new_values(&key, &["", "Swap"]);
    replace_subtrees(&write, &kept(&key, &["Keep"])).unwrap();

    let old = ["Keep", "Keep\\Deep", "Keep\\Here"];
    assert_eq!(key.export(), expected(&key, &old, &["", "Swap"], &[]));
}

#[test]
fn replace_deletes_keys_only_here_under_a_ticked_key_and_keeps_them_under_a_kept_one() {
    let key = TestKey::new();
    old_tree(&key);

    let write = new_values(&key, &["", "Keep\\Deep"]);
    replace_subtrees(&write, &kept(&key, &["Keep"])).unwrap();

    let old = ["Keep", "Keep\\Here"];
    assert_eq!(key.export(), expected(&key, &old, &["", "Keep\\Deep"], &[]));
}

#[test]
fn replace_writes_a_ticked_key_below_a_kept_one() {
    let key = TestKey::new();
    old_tree(&key);

    let write = new_values(&key, &["Keep\\Deep"]);
    replace_subtrees(&write, &kept(&key, &[""])).unwrap();

    let old = ["", "Here", "Keep", "Keep\\Here", "Swap"];
    assert_eq!(key.export(), expected(&key, &old, &["Keep\\Deep"], &[]));
}

#[test]
fn replace_matches_kept_keys_case_insensitively() {
    let key = TestKey::new();
    old_tree(&key);

    let mut write = RegistryExport::default();
    let values = vec![value("new", RegData::Dword(2))];
    write.keys.insert(key.path("").to_uppercase(), values);
    replace_subtrees(&write, &[key.path("keep").to_lowercase()]).unwrap();

    let old = ["Keep", "Keep\\Deep", "Keep\\Here"];
    assert_eq!(key.export(), expected(&key, &old, &[""], &[]));
}

#[test]
fn replace_with_a_kept_key_equal_to_an_export_key_changes_nothing() {
    let key = TestKey::new();
    old_tree(&key);
    let before = key.export();

    let write = new_values(&key, &["", "Keep"]);
    let result = replace_subtrees(&write, &[key.path("KEEP")]);

    assert!(matches!(
        result,
        Err(WindowsRegistryError::Key(
            RegistryError::KeptAndWritten { .. }
        ))
    ));
    assert_eq!(key.export(), before);
}

#[test]
fn replace_validates_kept_keys_before_changes() {
    let key = TestKey::new();
    old_tree(&key);
    let before = key.export();

    let write = new_values(&key, &[""]);
    for bad in [
        HKCU.to_owned(),
        key.path("A\\\\B"),
        format!("{}\\", key.path("")),
    ] {
        assert!(matches!(
            replace_subtrees(&write, &[bad]),
            Err(WindowsRegistryError::Key(RegistryError::InvalidKey { .. }))
        ));
    }
    assert!(matches!(
        replace_subtrees(&write, &["HKEY_LOCAL_MACHINE\\Software".to_owned()]),
        Err(WindowsRegistryError::Key(
            RegistryError::UnsupportedRoot { .. }
        ))
    ));
    assert_eq!(key.export(), before);
}

#[test]
fn an_owned_key_with_a_kept_descendant_loses_its_old_values_but_stays() {
    let key = TestKey::new();
    old_tree(&key);

    replace_subtrees(&new_values(&key, &[""]), &kept(&key, &["Keep\\Deep"])).unwrap();

    let want = expected(&key, &["Keep\\Deep"], &[""], &["Keep"]);
    assert_eq!(key.export(), want);
}
