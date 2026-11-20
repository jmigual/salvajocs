use super::*;

/// Trimmed excerpt of a real Proton prefix's `user.reg` (header, emoji key, `str(2)`,
/// `str(7)`, `dword:` and wrapped `hex:` values).
const FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/user.reg"
));
const SECS: u64 = 1_774_238_072;

fn fixture() -> RegFile {
    RegFile::parse(FIXTURE).unwrap()
}

fn full(relative: &str) -> String {
    format!("{HKCU}\\{relative}")
}

fn sz(name: &str, text: &str) -> RegValue {
    RegValue {
        name: name.into(),
        data: RegData::Sz(text.into()),
    }
}

/// A one-key file around the given value lines.
fn one_key(lines: &str) -> RegFile {
    RegFile::parse(&format!("{MAGIC}\n\n[Software\\\\T] 1\n#time=0\n{lines}\n")).unwrap()
}

fn values_of(lines: &str) -> Vec<RegValue> {
    let export = one_key(lines).export(&[full("Software\\T")]).unwrap();
    export.keys[&full("Software\\T")].clone()
}

/// Parses `line` (possibly several physical lines), checks the model, and writes it back.
fn assert_value(line: &str, expected: RegValue) {
    let values = values_of(line);
    assert_eq!(values, [expected]);
    assert_eq!(value_text(&values[0]), format!("{line}\n"));
}

#[test]
fn parse_then_write_is_byte_identical() {
    assert_eq!(fixture().to_string(), FIXTURE);
}

#[test]
fn writer_reproduces_wine_text_for_every_fixture_block() {
    let file = fixture();
    assert!(file.blocks.len() > 5);
    for block in &file.blocks {
        let header = block.raw.lines().next().unwrap();
        let path = &header[1..header.find("] ").unwrap()];
        let written = key_text(&block.path, &block.values().unwrap(), SECS);
        assert_eq!(written.lines().next().unwrap(), format!("[{path}] {SECS}"));
        let wine_values = block
            .raw
            .lines()
            .skip(1)
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .map(|l| format!("{l}\n"))
            .collect::<String>();
        let our_values = written
            .lines()
            .skip(2)
            .filter(|l| !l.is_empty())
            .map(|l| format!("{l}\n"))
            .collect::<String>();
        assert_eq!(our_values, wine_values, "key {}", block.path);
    }
}

#[test]
fn key_text_carries_wine_style_time() {
    let text = key_text("A", &[], SECS);
    assert_eq!(text, "[A] 1774238072\n#time=1dcba78c1878c00\n\n");
}

#[test]
fn value_sz() {
    assert_value(r#""Name"="C:\\Games\\Foo""#, sz("Name", r"C:\Games\Foo"));
}

#[test]
fn value_default_and_empty_sz() {
    assert_value(r#"@="""#, sz("", ""));
}

#[test]
fn value_expand_sz() {
    assert_value(
        r#""AppData"=str(2):"%USERPROFILE%\\AppData\\Roaming""#,
        RegValue {
            name: "AppData".into(),
            data: RegData::ExpandSz(r"%USERPROFILE%\AppData\Roaming".into()),
        },
    );
}

#[test]
fn value_multi_sz() {
    assert_value(
        r#""List"=str(7):"United States keyboard layout\0British keyboard layout\0""#,
        RegValue {
            name: "List".into(),
            data: RegData::MultiSz(vec![
                "United States keyboard layout".into(),
                "British keyboard layout".into(),
            ]),
        },
    );
    assert_value(
        r#""Empty"=str(7):"""#,
        RegValue {
            name: "Empty".into(),
            data: RegData::MultiSz(vec![]),
        },
    );
}

#[test]
fn value_dword() {
    assert_value(
        r#""PID"=dword:0000fffe"#,
        RegValue {
            name: "PID".into(),
            data: RegData::Dword(0xfffe),
        },
    );
}

#[test]
fn value_qword() {
    assert_value(
        r#""FirstNotDefault"=hex(b):c7,08,ad,56,a2,b2,2f,00"#,
        RegValue {
            name: "FirstNotDefault".into(),
            data: RegData::Qword(0x002f_b2a2_56ad_08c7),
        },
    );
}

#[test]
fn value_binary_with_line_continuation() {
    let line = "\"CaptionFont\"=hex:0a,00,00,00,00,00,00,00,00,00,00,00,00,00,00,00,90,01,00,00,\\\n  \
                00,00,00,01,00,00,00,00,54,00,61,00,68,00,6f,00,6d,00,61,00,00,00,00,00,00,\\\n  \
                00,00";
    let bytes = line
        .split_once("hex:")
        .unwrap()
        .1
        .split(|c: char| !c.is_ascii_hexdigit())
        .filter(|t| !t.is_empty())
        .map(|t| u8::from_str_radix(t, 16).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(bytes.len(), 47);
    assert_value(
        line,
        RegValue {
            name: "CaptionFont".into(),
            data: RegData::Binary(bytes),
        },
    );
}

#[test]
fn value_other_keeps_unity_double_stored_as_dword_type() {
    assert_value(
        r#""BgmVolume_h362709765"=hex(4):00,00,00,00,00,00,e0,3f"#,
        RegValue {
            name: "BgmVolume_h362709765".into(),
            data: RegData::Other {
                code: 4,
                bytes: vec![0, 0, 0, 0, 0, 0, 0xe0, 0x3f],
            },
        },
    );
    assert_value(
        r#""Empty"=hex(1):"#,
        RegValue {
            name: "Empty".into(),
            data: RegData::Other {
                code: 1,
                bytes: vec![],
            },
        },
    );
}

#[test]
fn non_ascii_key_name_from_real_prefix_round_trips() {
    // Written by wineserver: two surrogate pairs, one `\x` escape per UTF-16 unit.
    let file = fixture();
    let key = full("Control Panel\\International\\\u{1f30e}\u{1f30f}\u{1f30d}");
    let export = file.export(&[&key]).unwrap();
    assert_eq!(export.keys[&key], [sz("Currencies", "USD")]);
    let mut other = fixture();
    other.replace_subtrees_at(&export, &[], SECS).unwrap();
    assert!(other.to_string().contains(
        "[Control Panel\\\\International\\\\\\xd83c\\xdf0e\\xd83c\\xdf0f\\xd83c\\xdf0d] 1774238072"
    ));
}

#[test]
fn non_ascii_string_values_follow_dump_strw_rules() {
    // Same code path as the real key name above; no prefix here has a non-ASCII value.
    assert_value(r#""N"="caf\xe9""#, sz("N", "caf\u{e9}"));
    // Padded to 4 digits only when the next character is a hex digit.
    assert_value(r#""N"="\x00e9a""#, sz("N", "\u{e9}a"));
    assert_value(r#""N"="\xe9z""#, sz("N", "\u{e9}z"));
    assert_value(r#""N"="\xd83c\xdf0e""#, sz("N", "\u{1f30e}"));
    assert_value(r#""N"="a\tb\\\"c\1\0012""#, sz("N", "a\tb\\\"c\u{1}\u{1}2"));
}

#[test]
fn reader_accepts_raw_utf8_and_unknown_escapes() {
    let values = values_of("\"N\"=\"caf\u{e9}\\q\\x\"");
    assert_eq!(values, [sz("N", "caf\u{e9}qx")]);
}

#[test]
fn lone_surrogate_string_is_kept_as_raw_bytes() {
    let values = values_of(r#""N"="\xd800""#);
    let data = RegData::Other {
        code: 1,
        bytes: vec![0, 0xd8, 0, 0],
    };
    assert_eq!(values[0].data, data);
}

#[test]
fn export_collects_subtree_and_implicit_root() {
    let file = fixture();
    // `Software\Valve` has no block of its own.
    let export = file.export(&[full("software\\VALVE")]).unwrap();
    let keys = export.keys.keys().cloned().collect::<Vec<_>>();
    assert_eq!(
        keys,
        [
            full("Software\\Valve"),
            full("Software\\Valve\\Steam"),
            full("Software\\Valve\\Steam\\ActiveProcess"),
        ]
    );
    assert!(export.keys[&full("Software\\Valve")].is_empty());
}

#[test]
fn export_matches_whole_elements_only() {
    let file = RegFile::parse(&format!(
        "{MAGIC}\n\n[Software\\\\Steam] 1\n\"a\"=\"1\"\n\n[Software\\\\SteamX] 1\n\"b\"=\"2\"\n"
    ))
    .unwrap();
    let export = file.export(&[full("Software\\Steam")]).unwrap();
    assert_eq!(
        export.keys.keys().collect::<Vec<_>>(),
        [&full("Software\\Steam")]
    );
}

#[test]
fn export_of_absent_key_is_empty() {
    let export = fixture().export(&[full("Software\\Nope")]).unwrap();
    assert_eq!(export, RegistryExport::default());
}

#[test]
fn export_rejects_other_roots() {
    let err = fixture()
        .export(&["HKEY_LOCAL_MACHINE\\Software\\Foo"])
        .unwrap_err();
    assert!(matches!(err, RegistryError::UnsupportedRoot { .. }));
}

#[test]
fn export_then_replace_round_trips() {
    // Spans a valueless parent, a hex value, str(2) and the emoji key.
    let keys = [
        full("Software\\Valve"),
        full("Software\\Microsoft"),
        full("Control Panel"),
    ];
    let export = fixture().export(&keys).unwrap();
    assert!(export.keys.len() > 5);

    let mut target = fixture();
    target.replace_subtrees_at(&export, &[], SECS).unwrap();
    assert_eq!(target.export(&keys).unwrap(), export);

    // The result is still valid Wine text.
    let reparsed = RegFile::parse(&target.to_string()).unwrap();
    assert_eq!(reparsed.export(&keys).unwrap(), export);
}

#[test]
fn replace_leaves_other_keys_verbatim() {
    let export = fixture().export(&[full("Software\\Valve")]).unwrap();
    let mut target = fixture();
    target.replace_subtrees_at(&export, &[], SECS).unwrap();
    for block in fixture()
        .blocks
        .iter()
        .filter(|b| !b.path.starts_with("Software\\Valve"))
    {
        assert!(target.to_string().contains(&block.raw), "{}", block.path);
    }
}

#[test]
fn replace_drops_old_subkeys() {
    let steam = full("Software\\Valve\\Steam");
    let mut export = fixture().export(&[&steam]).unwrap();
    export.keys.remove(&format!("{steam}\\ActiveProcess"));
    export.keys.get_mut(&steam).unwrap().push(sz("New", "1"));

    let mut target = fixture();
    target.replace_subtrees_at(&export, &[], SECS).unwrap();
    let after = target.export(&[&steam]).unwrap();
    assert_eq!(after, export);
    assert!(!target.to_string().contains("ActiveProcess"));
}

#[test]
fn replace_with_empty_export_changes_nothing() {
    let mut target = fixture();
    target
        .replace_subtrees_at(&RegistryExport::default(), &[], SECS)
        .unwrap();
    assert_eq!(target.to_string(), FIXTURE);
}

#[test]
fn replace_rejects_other_roots_without_changes() {
    let mut export = RegistryExport::default();
    export.keys.insert(full("Software\\Valve"), vec![]);
    export
        .keys
        .insert("HKEY_LOCAL_MACHINE\\Software\\Foo".into(), vec![]);
    let mut target = fixture();
    let err = target.replace_subtrees_at(&export, &[], SECS).unwrap_err();
    assert!(matches!(err, RegistryError::UnsupportedRoot { .. }));
    assert_eq!(target.to_string(), FIXTURE);
}

#[test]
fn replace_separates_appended_key_with_blank_line() {
    let mut target = RegFile::parse(&format!("{MAGIC}\n\n[A] 1\n\"a\"=\"1\"\n")).unwrap();
    let mut export = RegistryExport::default();
    export.keys.insert(full("B"), vec![sz("b", "2")]);
    target.replace_subtrees_at(&export, &[], SECS).unwrap();
    assert!(
        target
            .to_string()
            .contains("\"a\"=\"1\"\n\n[B] 1774238072\n")
    );
}

#[test]
fn parse_rejects_files_without_wine_header() {
    assert!(matches!(
        RegFile::parse("Windows Registry Editor Version 5.00\n"),
        Err(RegistryError::NotRegistryFile)
    ));
}

#[test]
fn parse_rejects_unterminated_key() {
    let err = RegFile::parse(&format!("{MAGIC}\n\n[Software\\\\Foo\n")).unwrap_err();
    assert!(matches!(err, RegistryError::MalformedKey { line: 3 }));
}

#[test]
fn export_rejects_malformed_value() {
    let file = one_key("\"a\"=hex:zz");
    let err = file.export(&[full("Software\\T")]).unwrap_err();
    assert!(matches!(err, RegistryError::MalformedValue { line: 3 }));
}

fn steam_pair() -> RegFile {
    RegFile::parse(&format!(
        "{MAGIC}\n\n[Software\\\\Steam] 1\n\"a\"=\"1\"\n\n[Software\\\\Steam\\\\Old] 1\n\"o\"=\"0\"\n\n\
         [Software\\\\SteamX] 1\n\"b\"=\"2\"\n"
    ))
    .unwrap()
}

#[test]
fn replace_matches_whole_elements_only() {
    let before = steam_pair();
    let mut export = RegistryExport::default();
    export
        .keys
        .insert(full("Software\\Steam"), vec![sz("n", "3")]);
    let mut target = steam_pair();
    target.replace_subtrees_at(&export, &[], SECS).unwrap();
    let steam_x = before
        .blocks
        .iter()
        .find(|b| b.path.ends_with("SteamX"))
        .unwrap();
    assert!(target.to_string().contains(&steam_x.raw));
    assert!(!target.to_string().contains("Old"));
}

#[test]
fn replace_matches_case_insensitively_without_duplicates() {
    let mut export = RegistryExport::default();
    export
        .keys
        .insert(full("software\\STEAM"), vec![sz("n", "3")]);
    let mut target = steam_pair();
    target.replace_subtrees_at(&export, &[], SECS).unwrap();
    let after = target.export(&[full("Software\\Steam")]).unwrap();
    assert_eq!(after.keys.len(), 1);
    assert_eq!(after.keys.values().next().unwrap(), &[sz("n", "3")]);
    assert_eq!(
        target
            .blocks
            .iter()
            .filter(|b| b.path.to_lowercase() == "software\\steam")
            .count(),
        1
    );
}

#[test]
fn root_and_empty_elements_are_invalid_keys() {
    for key in [
        HKCU.to_owned(),
        format!("{HKCU}\\"),
        full("Software\\Valve\\"),
        full("Software\\\\Valve"),
    ] {
        let err = fixture().export(&[&key]).unwrap_err();
        assert!(matches!(err, RegistryError::InvalidKey { .. }), "{key}");
        let mut export = RegistryExport::default();
        export.keys.insert(key, vec![]);
        let mut target = fixture();
        let err = target.replace_subtrees_at(&export, &[], SECS).unwrap_err();
        assert!(matches!(err, RegistryError::InvalidKey { .. }));
        assert_eq!(target.to_string(), FIXTURE);
    }
}

/// `Software\Foo` with its own value and the subkeys `Keep` (holding `Deep` and `Here`), `Swap`
/// and `Here`, and `Software\Other`. Every block has time 1, so a rewritten block shows.
const FOO_TREE: &str = r#"WINE REGISTRY Version 2

[Software\\Foo] 1
"v"="old"

[Software\\Foo\\Here] 1
"h"="old"

[Software\\Foo\\Keep] 1
"k"="old"

[Software\\Foo\\Keep\\Deep] 1
"d"="old"

[Software\\Foo\\Keep\\Here] 1
"h"="old"

[Software\\Foo\\Swap] 1
"s"="old"

[Software\\Other] 1
"o"="old"

"#;

/// An export of `keys` (relative to HKCU), each with the value `x` = `new`.
fn new_values(keys: &[&str]) -> RegistryExport {
    let mut export = RegistryExport::default();
    for key in keys {
        export.keys.insert(full(key), vec![sz("x", "new")]);
    }
    export
}

fn full_keys(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| full(key)).collect()
}

/// The text of the block of `path` (relative to HKCU, spelled exactly), if any.
fn block<'a>(file: &'a RegFile, path: &str) -> Option<&'a str> {
    (file.blocks.iter())
        .find(|b| b.path == path)
        .map(|b| b.raw.as_str())
}

/// Replaces `export` keeping `kept` (both relative to HKCU) in [`FOO_TREE`].
fn replaced(file: &str, export: &RegistryExport, kept: &[&str]) -> RegFile {
    let mut target = RegFile::parse(file).unwrap();
    target
        .replace_subtrees_at(export, &full_keys(kept), SECS)
        .unwrap();
    target
}

/// Asserts that each block of `paths` is in `after` exactly as in `before`.
fn assert_untouched(before: &str, after: &RegFile, paths: &[&str]) {
    let before = RegFile::parse(before).unwrap();
    for path in paths {
        assert_eq!(block(after, path), block(&before, path), "{path}");
        assert!(block(after, path).is_some(), "{path}");
    }
}

/// Whether the block of `path` was written by the replace, holding `x` = `new`.
fn rewritten(file: &RegFile, path: &str) -> bool {
    block(file, path)
        .is_some_and(|raw| raw.contains(&format!("] {SECS}\n")) && raw.contains("\"x\"=\"new\""))
}

#[test]
fn replace_keeps_an_unticked_subkey_with_its_values_and_subkeys() {
    let export = new_values(&[r"Software\Foo"]);

    let target = replaced(FOO_TREE, &export, &[r"Software\Foo\Keep"]);

    let kept = [
        r"Software\Foo\Keep",
        r"Software\Foo\Keep\Deep",
        r"Software\Foo\Keep\Here",
        r"Software\Other",
    ];
    assert_untouched(FOO_TREE, &target, &kept);
    assert!(rewritten(&target, r"Software\Foo"));
    assert_eq!(block(&target, r"Software\Foo\Swap"), None);
}

#[test]
fn replace_leaves_a_kept_last_block_without_a_blank_line_byte_for_byte() {
    let before = FOO_TREE.trim_end_matches('\n');
    for before in [before.to_owned(), format!("{before}\n")] {
        let export = new_values(&[r"Software\Foo"]);

        let target = replaced(&before, &export, &[r"Software\Other"]);

        assert_untouched(&before, &target, &[r"Software\Other"]);
        let text = target.to_string();
        assert!(text.contains("\"o\"=\"old\"\n\n["), "{text}");
        let reparsed = RegFile::parse(&text).unwrap();
        assert!(rewritten(&reparsed, r"Software\Foo"), "{text}");
    }
}

#[test]
fn replace_writes_the_ticked_sibling_of_a_kept_key() {
    let export = new_values(&[r"Software\Foo", r"Software\Foo\Swap"]);

    let target = replaced(FOO_TREE, &export, &[r"Software\Foo\Keep"]);

    assert!(rewritten(&target, r"Software\Foo\Swap"));
    assert!(!target.to_string().contains("\"s\"=\"old\""));
    assert_untouched(FOO_TREE, &target, &[r"Software\Foo\Keep"]);
}

#[test]
fn replace_deletes_keys_only_here_under_a_ticked_key_and_keeps_them_under_a_kept_one() {
    let export = new_values(&[r"Software\Foo", r"Software\Foo\Keep\Deep"]);

    let target = replaced(FOO_TREE, &export, &[r"Software\Foo\Keep"]);

    assert_eq!(block(&target, r"Software\Foo\Here"), None);
    let kept = [r"Software\Foo\Keep", r"Software\Foo\Keep\Here"];
    assert_untouched(FOO_TREE, &target, &kept);
    assert!(rewritten(&target, r"Software\Foo\Keep\Deep"));
}

#[test]
fn replace_writes_a_ticked_key_below_a_kept_one() {
    let export = new_values(&[r"Software\Foo\Keep\Deep"]);

    let target = replaced(FOO_TREE, &export, &[r"Software\Foo"]);

    assert!(rewritten(&target, r"Software\Foo\Keep\Deep"));
    let kept = [
        r"Software\Foo",
        r"Software\Foo\Here",
        r"Software\Foo\Keep",
        r"Software\Foo\Keep\Here",
        r"Software\Foo\Swap",
        r"Software\Other",
    ];
    assert_untouched(FOO_TREE, &target, &kept);
}

#[test]
fn replace_matches_kept_keys_case_insensitively() {
    let export = new_values(&[r"software\FOO"]);

    let target = replaced(FOO_TREE, &export, &[r"SOFTWARE\foo\KEEP"]);

    let kept = [
        r"Software\Foo\Keep",
        r"Software\Foo\Keep\Deep",
        r"Software\Foo\Keep\Here",
    ];
    assert_untouched(FOO_TREE, &target, &kept);
    assert!(rewritten(&target, r"software\FOO"));
    assert_eq!(block(&target, r"Software\Foo"), None);
    assert_eq!(block(&target, r"Software\Foo\Swap"), None);
}

#[test]
fn replace_with_a_kept_key_equal_to_an_export_key_changes_nothing() {
    let export = new_values(&[r"Software\Foo", r"Software\Foo\Keep"]);
    let mut target = RegFile::parse(FOO_TREE).unwrap();

    let kept = full_keys(&[r"software\FOO\keep"]);
    let err = (target.replace_subtrees_at(&export, &kept, SECS)).unwrap_err();

    assert!(matches!(err, RegistryError::KeptAndWritten { .. }), "{err}");
    assert_eq!(target.to_string(), FOO_TREE);
}

#[test]
fn replace_validates_kept_keys_before_changes() {
    let export = new_values(&[r"Software\Foo"]);
    let mut target = RegFile::parse(FOO_TREE).unwrap();

    for (kept, invalid) in [
        (HKCU.to_owned(), true),
        (full(r"Software\\Foo"), true),
        (full(r"Software\Foo\"), true),
        (r"HKEY_LOCAL_MACHINE\Software\Foo".to_owned(), false),
    ] {
        let err = (target.replace_subtrees_at(&export, &[kept], SECS)).unwrap_err();
        if invalid {
            assert!(matches!(err, RegistryError::InvalidKey { .. }), "{err}");
        } else {
            assert!(
                matches!(err, RegistryError::UnsupportedRoot { .. }),
                "{err}"
            );
        }
        assert_eq!(target.to_string(), FOO_TREE);
    }
}

/// `Software\Foo` and `Foo\A\B` with no block of `Foo\A`, as wineserver saves a valueless key
/// with subkeys, plus `Foo\A\Only`, which only this PC has.
const GAPPED: &str = r#"WINE REGISTRY Version 2

[Software\\Foo] 1
"v"="old"

[Software\\Foo\\A\\B] 1
"b"="old"

[Software\\Foo\\A\\Only] 1
"o"="old"

"#;

#[test]
fn replace_of_an_export_with_a_gap_round_trips() {
    let foo = full(r"Software\Foo");
    let mut backed_up = RegFile::parse(GAPPED).unwrap();
    backed_up.blocks.retain(|b| !b.path.ends_with("Only"));
    let export = backed_up.export(&[&foo]).unwrap();
    let tree = export.tree();
    assert_eq!(tree.len(), 3, "Foo, the added Foo\\A and Foo\\A\\B");

    let (write, kept) = export.select(&tree, &vec![true; tree.len()]);
    let mut target = RegFile::parse(GAPPED).unwrap();
    target.replace_subtrees_at(&write, &kept, SECS).unwrap();

    assert_eq!(target.export(&[&foo]).unwrap(), export);
    assert_eq!(block(&target, r"Software\Foo\A"), None);
    assert_eq!(block(&target, r"Software\Foo\A\Only"), None);
}

#[test]
fn replace_writes_no_block_for_a_valueless_key_with_a_block_below() {
    // `Foo` unticked, so the added `Foo\A` is written to own `Foo\A\Only`.
    let mut export = new_values(&[r"Software\Foo\A\B"]);
    export.keys.insert(full(r"Software\Foo\A"), Vec::new());

    let target = replaced(GAPPED, &export, &[r"Software\Foo"]);

    assert_untouched(GAPPED, &target, &[r"Software\Foo"]);
    assert_eq!(block(&target, r"Software\Foo\A\Only"), None);
    assert_eq!(block(&target, r"Software\Foo\A"), None);
    assert!(rewritten(&target, r"Software\Foo\A\B"));

    // A valueless key with nothing below gets an empty block, as wineserver saves one.
    let mut leaf = RegistryExport::default();
    leaf.keys.insert(full(r"Software\Foo\A\B"), Vec::new());
    let target = replaced(GAPPED, &leaf, &[]);
    let empty = format!("[Software\\\\Foo\\\\A\\\\B] {SECS}\n");
    assert!(block(&target, r"Software\Foo\A\B").is_some_and(|raw| raw.starts_with(&empty)));
}
