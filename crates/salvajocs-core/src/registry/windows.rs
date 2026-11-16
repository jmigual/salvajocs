//! The live Windows registry (HKCU only) as the normalized model of [`super`] (ADR-0006).
//!
//! Same contract as the Wine side: an export of key `K` holds `K` itself plus every key below
//! it, and replacing deletes the subtree of each exported key before writing it. Value data
//! that is not in canonical form (unterminated or odd-length strings, a `REG_DWORD` of 8 bytes)
//! is kept as [`RegData::Other`] so it writes back byte for byte.

use std::io;

use windows_registry::{CURRENT_USER, Key, Type, Value};

use super::wine::hkcu_relative;
use super::{HKCU, RegData, RegValue, RegistryError, RegistryExport};

/// `HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)`, what a missing key reports.
const NOT_FOUND: i32 = 0x8007_0002_u32 as i32;

/// Errors from reading or writing the live registry.
#[derive(Debug, thiserror::Error)]
pub enum WindowsRegistryError {
    /// A key path was refused before touching the registry.
    #[error(transparent)]
    Key(#[from] RegistryError),
    /// A Windows registry call failed for `key`.
    #[error("registry access failed for {key}")]
    Os {
        key: String,
        #[source]
        source: io::Error,
    },
}

/// Collects each of `keys` (`HKEY_CURRENT_USER\...`) and everything below it.
///
/// Absent keys add nothing. Every key is spelled as the registry stores it. The HKCU root itself
/// and paths with empty elements are refused.
pub fn export<S: AsRef<str>>(keys: &[S]) -> Result<RegistryExport, WindowsRegistryError> {
    let mut export = RegistryExport::default();
    for key in keys {
        let relative = hkcu_relative(key.as_ref())?;
        if let Some((path, root)) = open_stored(relative)? {
            collect(&root, &path, &mut export)?;
        }
    }
    Ok(export)
}

/// Deletes the subtree of every key in `export`, then writes the exported keys and values.
///
/// All key paths are validated before anything is deleted. An empty export changes nothing. Not
/// atomic: a failure after the deletes leaves the keys deleted or partly written, so callers
/// back up the current state first.
pub fn replace_subtrees(export: &RegistryExport) -> Result<(), WindowsRegistryError> {
    let keys = export
        .keys
        .keys()
        .map(|key| hkcu_relative(key).map(|relative| (key.as_str(), relative)))
        .collect::<Result<Vec<_>, _>>()?;
    // Delete every subtree first: a later delete would erase keys already written below it.
    for (key, relative) in &keys {
        found(key, CURRENT_USER.remove_tree(relative))?;
    }
    for ((key, relative), values) in keys.iter().zip(export.keys.values()) {
        let target = os(key, CURRENT_USER.options().write().create().open(relative))?;
        for value in values {
            let (ty, bytes) = to_raw(&value.data);
            os(key, target.set_bytes(&value.name, ty, &bytes))?;
        }
    }
    Ok(())
}

/// Adds `key` (at `path`) and every key below it to `export`.
fn collect(key: &Key, path: &str, export: &mut RegistryExport) -> Result<(), WindowsRegistryError> {
    let mut values: Vec<RegValue> = os(path, key.values())?
        .map(|(name, value)| RegValue {
            name,
            data: from_raw(&value),
        })
        .collect();
    // The registry lists values in creation order; sorted keeps exports deterministic.
    values.sort_by(|a, b| a.name.cmp(&b.name));
    export.keys.insert(path.to_owned(), values);
    for name in os(path, key.keys())? {
        // A listed name that cannot be reopened (e.g. lossy UTF-16) must fail, not be skipped:
        // a later replace would delete the subtree this export silently left out.
        let child = os(path, key.open(&name))?;
        collect(&child, &format!("{path}\\{name}"), export)?;
    }
    Ok(())
}

/// Opens `relative` below HKCU, returning its full path spelled as the registry stores each
/// element, or `None` if the key is absent.
fn open_stored(relative: &str) -> Result<Option<(String, Key)>, WindowsRegistryError> {
    let mut path = HKCU.to_owned();
    let mut current: Option<Key> = None;
    for element in relative.split('\\') {
        let parent = current.as_ref().unwrap_or(CURRENT_USER);
        let wanted = element.to_lowercase();
        let name = os(&path, parent.keys())?
            .find(|name| name.to_lowercase() == wanted)
            .unwrap_or_else(|| element.to_owned());
        path = format!("{path}\\{name}");
        match found(&path, parent.open(&name))? {
            Some(key) => current = Some(key),
            None => return Ok(None),
        }
    }
    Ok(current.map(|key| (path, key)))
}

fn os<T>(key: &str, result: windows_registry::Result<T>) -> Result<T, WindowsRegistryError> {
    result.map_err(|e| WindowsRegistryError::Os {
        key: key.to_owned(),
        source: e.into(),
    })
}

/// Like [`os`], with a missing key as `None`.
fn found<T>(
    key: &str,
    result: windows_registry::Result<T>,
) -> Result<Option<T>, WindowsRegistryError> {
    match result {
        Err(e) if e.code().0 == NOT_FOUND => Ok(None),
        result => os(key, result).map(Some),
    }
}

/// Types a registry value; anything that would not write back identically stays `Other`.
fn from_raw(value: &Value) -> RegData {
    let ty = value.ty();
    let bytes: &[u8] = value;
    let typed = match ty {
        Type::U32 => bytes
            .try_into()
            .ok()
            .map(|b| RegData::Dword(u32::from_le_bytes(b))),
        Type::U64 => bytes
            .try_into()
            .ok()
            .map(|b| RegData::Qword(u64::from_le_bytes(b))),
        Type::String => decode_string(bytes).map(RegData::Sz),
        Type::ExpandString => decode_string(bytes).map(RegData::ExpandSz),
        Type::MultiString => decode_multi(bytes).map(RegData::MultiSz),
        Type::Bytes => Some(RegData::Binary(bytes.to_vec())),
        Type::Other(_) => None,
    };
    typed.unwrap_or_else(|| RegData::Other {
        code: ty.into(),
        bytes: bytes.to_vec(),
    })
}

/// `Other` is written as `Type::Other` even for codes 1, 2 and 7: the crate's typed string paths
/// debug-assert terminated data, which `Other` (e.g. an empty `REG_SZ`) need not have.
fn to_raw(data: &RegData) -> (Type, Vec<u8>) {
    match data {
        RegData::Sz(text) => (Type::String, string_bytes(text)),
        RegData::ExpandSz(text) => (Type::ExpandString, string_bytes(text)),
        RegData::MultiSz(list) => (Type::MultiString, multi_bytes(list)),
        RegData::Dword(n) => (Type::U32, n.to_le_bytes().to_vec()),
        RegData::Qword(n) => (Type::U64, n.to_le_bytes().to_vec()),
        RegData::Binary(bytes) => (Type::Bytes, bytes.clone()),
        RegData::Other { code, bytes } => (Type::Other(*code), bytes.clone()),
    }
}

/// UTF-16LE with a terminating NUL, as `REG_SZ` stores it.
fn string_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// Each string NUL-terminated, then one more NUL, as `REG_MULTI_SZ` stores it.
fn multi_bytes(list: &[String]) -> Vec<u8> {
    list.iter()
        .flat_map(|s| s.encode_utf16().chain([0]))
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn wide(bytes: &[u8]) -> Option<String> {
    let (chunks, rest) = bytes.as_chunks::<2>();
    let units = rest.is_empty().then(|| {
        chunks
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect::<Vec<_>>()
    })?;
    String::from_utf16(&units).ok()
}

/// The text of a canonical `REG_SZ`/`REG_EXPAND_SZ` (one trailing NUL, no other).
fn decode_string(bytes: &[u8]) -> Option<String> {
    let text = wide(bytes)?.strip_suffix('\0')?.to_owned();
    (string_bytes(&text) == bytes && !text.contains('\0')).then_some(text)
}

/// The strings of a canonical `REG_MULTI_SZ`; data after the first empty string is not.
fn decode_multi(bytes: &[u8]) -> Option<Vec<String>> {
    let text = wide(bytes)?;
    let list: Vec<String> = text
        .split('\0')
        .take_while(|s| !s.is_empty())
        .map(String::from)
        .collect();
    (multi_bytes(&list) == bytes).then_some(list)
}

#[cfg(test)]
mod tests;
