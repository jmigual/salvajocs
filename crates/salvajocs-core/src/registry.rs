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
pub struct RegValue {
    pub name: String,
    #[serde(flatten)]
    pub data: RegData,
}

/// Values of the exported keys, keyed by full path (`HKEY_CURRENT_USER\Software\Foo`).
///
/// An export of key `K` holds `K` itself (possibly without values) plus every existing key
/// below it, or nothing when `K` is absent. Restoring replaces the subtree of each key present.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RegistryExport {
    pub keys: BTreeMap<String, Vec<RegValue>>,
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
}
