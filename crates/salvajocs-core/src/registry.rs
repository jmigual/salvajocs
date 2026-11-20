//! Registry keys as normalized data (ADR-0006), shared by the Wine and Windows backends.

#[cfg(windows)]
pub mod windows;
pub mod wine;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Root every key path starts with; only HKCU is supported in v1.
pub const HKCU: &str = "HKEY_CURRENT_USER";

/// Errors from reading, exporting or replacing registry data.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RegistryError {
    /// The text does not start with Wine's `WINE REGISTRY Version 2` line.
    #[error("not a Wine registry file")]
    NotRegistryFile,
    /// A `[key]` line is unterminated or its name is not valid UTF-16.
    #[error("malformed key header at line {line}")]
    MalformedKey { line: usize },
    /// A value line in the key block starting at `line` cannot be parsed.
    #[error("malformed value in the key block at line {line}")]
    MalformedValue { line: usize },
    /// A key path is not under `HKEY_CURRENT_USER` (backslash separated).
    #[error("key is not under HKEY_CURRENT_USER: {key}")]
    UnsupportedRoot { key: String },
    /// A key path is the HKCU root itself or has an empty element (doubled or trailing `\`).
    #[error("invalid key path: {key}")]
    InvalidKey { key: String },
}

/// A registry value's data, tagged by Windows value type.
///
/// Types without a typed form (e.g. a `REG_DWORD` holding 8 bytes, as Unity writes) are kept as
/// [`RegData::Other`] with their raw bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RegData {
    Sz(String),
    ExpandSz(String),
    /// Strings of a `REG_MULTI_SZ`, without the terminating empty string.
    MultiSz(Vec<String>),
    Dword(u32),
    Qword(u64),
    Binary(Vec<u8>),
    /// Any other Windows value type `code`, as raw little-endian bytes.
    Other {
        code: u32,
        bytes: Vec<u8>,
    },
}

/// One named value of a key; the empty name is the key's default value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RegValue {
    pub name: String,
    #[serde(flatten)]
    pub data: RegData,
}

impl RegValue {
    /// The value `name` holding `data`.
    pub fn new(name: String, data: RegData) -> Self {
        Self { name, data }
    }
}

/// Values of the exported keys, keyed by full path (`HKEY_CURRENT_USER\Software\Foo`).
///
/// An export of key `K` holds `K` itself (possibly without values) plus every existing key
/// below it, or nothing when `K` is absent. Restoring replaces the subtree of each key present.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
#[non_exhaustive]
pub struct RegistryExport {
    pub keys: BTreeMap<String, Vec<RegValue>>,
}

impl RegistryExport {
    /// The keys with no ancestor key in the export, in map order: the declared keys as backed
    /// up. Every key lies under exactly one root; of keys equal but for case, the first is it.
    ///
    /// Roots are disjoint, so [`Self::remove_subtree`] of one leaves the others whole, and a
    /// restore of the rest replaces nothing below the removed root.
    pub fn roots(&self) -> Vec<&str> {
        let keys: Vec<&str> = self.keys.keys().map(String::as_str).collect();
        let below_another = |at: usize, key: &str| {
            (keys.iter().enumerate()).any(|(other_at, other)| {
                other_at != at && is_under(key, other) && (other_at < at || !is_under(other, key))
            })
        };
        (keys.iter().enumerate())
            .filter(|&(at, key)| !below_another(at, key))
            .map(|(_, key)| *key)
            .collect()
    }

    /// Removes `root` and every key below it, matching whole `\` elements with case ignored, as
    /// the restore's subtree replace does.
    pub fn remove_subtree(&mut self, root: &str) {
        self.keys.retain(|key, _| !is_under(key, root));
    }
}

/// Whether the `\`-separated key `path` is `root` or below it, comparing whole elements
/// case-insensitively.
pub(crate) fn is_under(path: &str, root: &str) -> bool {
    // Unicode lowercase approximates Wine's uppercase table; exotic names may differ.
    let (path, root) = (path.to_lowercase(), root.to_lowercase());
    path.strip_prefix(&root)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('\\'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trips_every_kind() {
        let value = |name: &str, data| RegValue {
            name: name.into(),
            data,
        };
        let mut export = RegistryExport::default();
        export.keys.insert(
            format!("{HKCU}\\Software\\Foo"),
            vec![
                value("", RegData::Sz("a".into())),
                value("e", RegData::ExpandSz("%TEMP%".into())),
                value("m", RegData::MultiSz(vec!["x".into(), "y".into()])),
                value("d", RegData::Dword(7)),
                value("q", RegData::Qword(u64::MAX)),
                value("b", RegData::Binary(vec![1, 2])),
                value(
                    "o",
                    RegData::Other {
                        code: 4,
                        bytes: vec![0; 8],
                    },
                ),
            ],
        );
        let json = serde_json::to_string(&export).unwrap();
        assert_eq!(
            serde_json::from_str::<RegistryExport>(&json).unwrap(),
            export
        );
        assert!(json.contains(r#""kind":"multi_sz""#));
    }

    /// An export of the given keys, each without values.
    fn export_of(keys: &[&str]) -> RegistryExport {
        let mut export = RegistryExport::default();
        for key in keys {
            export.keys.insert(format!("{HKCU}\\{key}"), Vec::new());
        }
        export
    }

    /// Nested keys, siblings sharing a text prefix and case variants, in and out of map order.
    const KEYS: [&str; 7] = [
        r"Software\Co",
        r"Software\Co\Game",
        r"Software\Com",
        r"Software\co",
        r"Software\Other\Deep",
        r"software\other",
        r"Software\Other\Deep\Er",
    ];

    #[test]
    fn roots_are_the_keys_without_an_ancestor_in_the_export() {
        let export = export_of(&KEYS);

        let roots = export.roots();
        let want = [r"Software\Co", r"Software\Com", r"software\other"];
        let want: Vec<String> = want.iter().map(|key| format!("{HKCU}\\{key}")).collect();
        assert_eq!(roots, want);
        // Every key lies under exactly one root.
        for key in export.keys.keys() {
            let under = roots.iter().filter(|root| is_under(key, root)).count();
            assert_eq!(under, 1, "{key}");
        }
    }

    #[test]
    fn remove_subtree_removes_a_root_and_everything_below_only() {
        let mut export = export_of(&KEYS);

        export.remove_subtree(&format!("{HKCU}\\Software\\Co"));

        let left: Vec<&str> = export.keys.keys().map(String::as_str).collect();
        let want = [
            r"Software\Com",
            r"Software\Other\Deep",
            r"Software\Other\Deep\Er",
            r"software\other",
        ];
        let want: Vec<String> = want.iter().map(|key| format!("{HKCU}\\{key}")).collect();
        assert_eq!(left, want);
        // A case variant of a root removes the same subtree.
        export.remove_subtree(&format!("{HKCU}\\SOFTWARE\\OTHER"));
        export.remove_subtree(&format!("{HKCU}\\Software\\Com"));
        assert!(export.keys.is_empty(), "{export:?}");
    }
}
