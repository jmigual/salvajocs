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
            relative: format!("Software\\salvajocs-test-{}-{nanos}-{n}", process::id()),
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
fn keys_are_matched_case_insensitively() {
    let key = TestKey::new();
    replace_subtrees(&export_of(vec![(
        key.path("Mixed"),
        vec![value("v", RegData::Dword(1))],
    )]))
    .unwrap();

    let found = export(&[key.path("mixed").to_uppercase()]).unwrap();

    assert_eq!(found.keys.len(), 1);
}

#[test]
fn non_canonical_strings_are_kept_as_other() {
    let key = TestKey::new();
    let target = CURRENT_USER.create(&key.relative).unwrap();
    // No terminating NUL, an interior NUL, odd length, and multi-string data after its end.
    let cases: [(&str, Type, &[u8]); 4] = [
        ("unterminated", Type::String, &[b'a', 0]),
        (
            "interior",
            Type::ExpandString,
            &[b'a', 0, 0, 0, b'b', 0, 0, 0],
        ),
        ("odd", Type::String, &[b'a', 0, 0, 0, 7]),
        (
            "trailing",
            Type::MultiString,
            &[b'a', 0, 0, 0, b'b', 0, 0, 0, 0, 0],
        ),
    ];
    for (name, ty, bytes) in cases {
        target.set_bytes(name, ty, bytes).unwrap();
    }

    let exported = key.export();
    for (name, ty, bytes) in cases {
        let data = &exported.keys[&key.path("")]
            .iter()
            .find(|v| v.name == name)
            .unwrap()
            .data;
        assert_eq!(
            data,
            &RegData::Other {
                code: ty.into(),
                bytes: bytes.to_vec()
            }
        );
    }

    // Writing the export back leaves the bytes unchanged.
    replace_subtrees(&exported).unwrap();
    let rewritten = CURRENT_USER.open(&key.relative).unwrap();
    for (name, ty, bytes) in cases {
        let value = rewritten.get_value(name).unwrap();
        assert_eq!((value.ty(), &*value), (ty, bytes));
    }
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
