//! These run against the real HKCU, in a throwaway `Software\salvajocs-test-*` key.

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
            // Spelled `SOFTWARE`, as Windows stores it: exports use the stored spelling.
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

    replace_subtrees(&expected).unwrap();

    assert_eq!(key.export(), expected);
}

#[test]
fn replace_deletes_the_old_subtree() {
    let key = TestKey::new();
    replace_subtrees(&export_of(vec![
        (key.path(""), vec![value("stale", RegData::Dword(1))]),
        (key.path("Gone"), vec![value("x", RegData::Dword(2))]),
    ]))
    .unwrap();

    let new = export_of(vec![(
        key.path(""),
        vec![value("fresh", RegData::Dword(3))],
    )]);
    replace_subtrees(&new).unwrap();

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
    replace_subtrees(&export_of(vec![
        (key.path(""), vec![]),
        (key.path("A"), vec![value("a", RegData::Dword(1))]),
        (key.path("B"), vec![value("b", RegData::Dword(2))]),
    ]))
    .unwrap();

    let only_a = export(&[key.path("A")]).unwrap();

    assert_eq!(only_a.keys.keys().collect::<Vec<_>>(), [&key.path("A")]);
}

#[test]
fn keys_are_matched_case_insensitively_and_spelled_as_stored() {
    let key = TestKey::new();
    replace_subtrees(&export_of(vec![(
        key.path("Mixed"),
        vec![value("v", RegData::Dword(1))],
    )]))
    .unwrap();

    let found = export(&[key.path("mixed").to_uppercase()]).unwrap();

    assert_eq!(found.keys.keys().collect::<Vec<_>>(), [&key.path("Mixed")]);
}

#[test]
fn replace_leaves_siblings_and_prefix_sharing_keys_alone() {
    let key = TestKey::new();
    let sibling = |name: &str| (key.path(name), vec![value(name, RegData::Dword(9))]);
    replace_subtrees(&export_of(vec![
        sibling("A"),
        sibling("B"),
        sibling("AB"),
        (key.path("A\\Child"), vec![]),
    ]))
    .unwrap();

    let new_a = export_of(vec![(key.path("A"), vec![value("new", RegData::Dword(1))])]);
    replace_subtrees(&new_a).unwrap();

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
    replace_subtrees(&exported).unwrap();
    assert_eq!(key.export(), exported);
}

#[test]
fn invalid_keys_are_refused_before_anything_is_deleted() {
    let key = TestKey::new();
    let kept = export_of(vec![(key.path(""), vec![value("v", RegData::Dword(1))])]);
    replace_subtrees(&kept).unwrap();

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
            replace_subtrees(&mixed),
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
