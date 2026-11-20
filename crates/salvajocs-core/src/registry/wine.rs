//! Wine's registry text format (`user.reg`, "WINE REGISTRY Version 2") and its normalized model.
//!
//! Escaping, as written by wineserver (`server/unicode.c` `dump_strW`, read back by
//! `parse_strW`), verified against a real Proton `user.reg`:
//! - Key paths are relative to HKCU with `\\` between elements; `\`, `[` and `]` are escaped
//!   in key names, `\` and `"` in value names and string data.
//! - Every UTF-16 code unit above 127 is written as `\x<hex>`, never as UTF-8. Surrogate pairs
//!   are two escapes (`\xd83c\xdf0e`). The hex is padded to 4 digits only when the next
//!   character is an ASCII hex digit. The reader takes up to 4 digits and also accepts raw UTF-8.
//! - Control characters below 32 are `\a \b \t \n \v \f \r \e` or octal (padded to 3 digits only
//!   before an octal digit). `\0` separates the strings of `str(7):`.
//! - `str(2):` and `str(7):` hold text; other types are `hex(<type>):` bytes, wrapped after
//!   column 76 with a trailing `\` and a two-space indent; `REG_QWORD` is `hex(b):`.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    HKCU, Owner, Owners, RegData, RegValue, RegistryError, RegistryExport, is_under, lowered_below,
};

#[cfg(test)]
mod tests;

const MAGIC: &str = "WINE REGISTRY Version 2";
/// Seconds between the Windows epoch (1601) and the Unix epoch.
const SECS_1601_TO_1970: u64 = 11_644_473_600;

/// A parsed `user.reg`. Everything is kept as text so untouched keys write back verbatim.
#[derive(Debug, Clone)]
pub struct RegFile {
    /// Text before the first key block (magic line, relative-to comment, `#arch`).
    header: String,
    blocks: Vec<Block>,
}

/// One `[key]` line with the lines below it, up to the next key.
#[derive(Debug, Clone)]
struct Block {
    /// Decoded path relative to HKCU, `\`-separated.
    path: String,
    /// 1-based line of the `[key]` line; 0 for blocks written by [`RegFile::replace_subtrees`].
    line: usize,
    raw: String,
}

impl RegFile {
    /// Splits `text` into the header and key blocks; values are only read by [`Self::export`].
    pub fn parse(text: &str) -> Result<RegFile, RegistryError> {
        if text.lines().next() != Some(MAGIC) {
            return Err(RegistryError::NotRegistryFile);
        }
        let mut file = RegFile {
            header: String::new(),
            blocks: Vec::new(),
        };
        for (n, line) in text.split_inclusive('\n').enumerate() {
            if let Some(rest) = line.trim_start().strip_prefix('[') {
                let malformed = || RegistryError::MalformedKey { line: n + 1 };
                let (units, _) = unescape(rest, b']').ok_or_else(malformed)?;
                let path = String::from_utf16(&units).map_err(|_| malformed())?;
                file.blocks.push(Block {
                    path,
                    line: n + 1,
                    raw: String::new(),
                });
            }
            match file.blocks.last_mut() {
                Some(block) => block.raw.push_str(line),
                None => file.header.push_str(line),
            }
        }
        Ok(file)
    }

    /// Collects each of `keys` (`HKEY_CURRENT_USER\...`) and everything below it.
    ///
    /// Matching is case-insensitive on whole path elements. Absent keys add nothing. The HKCU
    /// root itself and paths with empty elements are refused.
    pub fn export<S: AsRef<str>>(&self, keys: &[S]) -> Result<RegistryExport, RegistryError> {
        let mut export = RegistryExport::default();
        for key in keys {
            let root = hkcu_relative(key.as_ref())?;
            let mut found = self.blocks.iter().filter(|b| is_under(&b.path, root));
            let Some(first) = found.next() else { continue };
            // The root may be a valueless key that has no block of its own.
            let depth = root.split('\\').count();
            let root_path = first.path.split('\\').take(depth).collect::<Vec<_>>();
            export
                .keys
                .entry(full_key(&root_path.join("\\")))
                .or_default();
            for block in std::iter::once(first).chain(found) {
                export.keys.insert(full_key(&block.path), block.values()?);
            }
        }
        Ok(export)
    }

    /// Deletes the subtree of every key in `export`, except what a key of `kept` owns, then
    /// writes the exported keys with the current time (ADR-0026). A key belongs to the deepest
    /// key of `export` or `kept` it is at or below; a block owned by `kept`, or by neither, stays
    /// byte for byte. An exported key without values gets no block when a block lies below it,
    /// as wineserver saves it. An empty export changes nothing.
    ///
    /// # Errors
    /// Every key is checked before anything changes: [`RegistryError::UnsupportedRoot`] and
    /// [`RegistryError::InvalidKey`] as for [`Self::export`], and
    /// [`RegistryError::KeptAndWritten`] for a key in both `export` and `kept`.
    pub fn replace_subtrees(
        &mut self,
        export: &RegistryExport,
        kept: &[String],
    ) -> Result<(), RegistryError> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH);
        self.replace_subtrees_at(export, kept, now.map_or(0, |d| d.as_secs()))
    }

    fn replace_subtrees_at(
        &mut self,
        export: &RegistryExport,
        kept: &[String],
        unix_secs: u64,
    ) -> Result<(), RegistryError> {
        let roots = export
            .keys
            .keys()
            .map(|key| hkcu_relative(key))
            .collect::<Result<Vec<_>, _>>()?;
        let kept = (kept.iter())
            .map(|key| hkcu_relative(key))
            .collect::<Result<Vec<_>, _>>()?;
        let owners = Owners::new(&roots, &kept)?;
        self.blocks
            .retain(|b| owners.owner(&b.path) != Owner::Write);
        // Lowercased once: the save rule checks them for each valueless key.
        let after: Vec<String> = (self.blocks.iter().map(|b| b.path.as_str()))
            .chain(roots.iter().copied())
            .map(str::to_lowercase)
            .collect();
        let mut appended: Vec<Block> = (roots.iter().zip(export.keys.values()))
            .filter(|(root, values)| {
                // A key exists through its subkeys' blocks; an empty block would add it to the
                // next export when the backup's export left it out.
                let root = root.to_lowercase();
                !values.is_empty() || !after.iter().any(|path| lowered_below(path, &root))
            })
            .map(|(root, values)| Block {
                path: (*root).to_owned(),
                line: 0,
                raw: key_text(root, values, unix_secs),
            })
            .collect();
        let Some(first) = appended.first_mut() else {
            return Ok(());
        };
        // Wine separates key blocks with a blank line. It goes into the new block, so a kept
        // last block stays byte for byte.
        let tail = (self.blocks.last()).map_or(self.header.as_str(), |b| b.raw.as_str());
        if !tail.ends_with("\n\n") {
            let separator = if tail.ends_with('\n') { "\n" } else { "\n\n" };
            first.raw.insert_str(0, separator);
        }
        // Appended, not sorted in: wineserver re-sorts on its next save.
        self.blocks.extend(appended);
        Ok(())
    }
}

impl fmt::Display for RegFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.header)?;
        self.blocks.iter().try_for_each(|b| f.write_str(&b.raw))
    }
}

impl Block {
    /// Reads the values below the `[key]` line. Option and comment lines are skipped.
    fn values(&self) -> Result<Vec<RegValue>, RegistryError> {
        let mut lines = self.raw.lines().skip(1);
        let mut values = Vec::new();
        while let Some(line) = lines.next() {
            let line = line.trim_start();
            if line.starts_with(['@', '"']) {
                let value = parse_value(line, &mut lines)
                    .ok_or(RegistryError::MalformedValue { line: self.line })?;
                values.push(value);
            }
        }
        Ok(values)
    }
}

/// `HKEY_CURRENT_USER\a\b` gives `a\b`. The root alone and empty elements are refused: a
/// stray `\` would make restore miss the subtree it must delete.
pub(crate) fn hkcu_relative(key: &str) -> Result<&str, RegistryError> {
    let (root, rest) = key.split_once('\\').unwrap_or((key, ""));
    if !root.eq_ignore_ascii_case(HKCU) {
        return Err(RegistryError::UnsupportedRoot { key: key.into() });
    }
    if rest.split('\\').any(str::is_empty) {
        return Err(RegistryError::InvalidKey { key: key.into() });
    }
    Ok(rest)
}

fn full_key(relative: &str) -> String {
    format!("{HKCU}\\{relative}")
}

/// Mirror of wineserver's `parse_strW`: reads up to the unescaped `end` byte and returns the
/// UTF-16 units with the number of bytes consumed. `None` when `end` is never found.
fn unescape(src: &str, end: u8) -> Option<(Vec<u16>, usize)> {
    let bytes = src.as_bytes();
    let digits = |from: usize, max: usize, accept: fn(&u8) -> bool| {
        bytes[from..]
            .iter()
            .take(max)
            .take_while(|b| accept(b))
            .count()
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == end {
            return Some((out, i + 1));
        }
        if bytes[i] == b'\\' {
            i += 1;
            let escaped = *bytes.get(i)?;
            let control = match escaped {
                b'a' => Some(7),
                b'b' => Some(8),
                b't' => Some(9),
                b'n' => Some(10),
                b'v' => Some(11),
                b'f' => Some(12),
                b'r' => Some(13),
                b'e' => Some(27),
                _ => None,
            };
            if let Some(unit) = control {
                out.push(unit);
                i += 1;
                continue;
            }
            if escaped == b'x' {
                i += 1;
                let n = digits(i, 4, u8::is_ascii_hexdigit);
                if n == 0 {
                    out.push(u16::from(b'x'));
                } else {
                    out.push(u16::from_str_radix(&src[i..i + n], 16).ok()?);
                    i += n;
                }
                continue;
            }
            if (b'0'..=b'7').contains(&escaped) {
                let n = digits(i, 3, |b| (b'0'..=b'7').contains(b));
                out.push(u16::from_str_radix(&src[i..i + n], 8).ok()?);
                i += n;
                continue;
            }
            // Any other escaped character stands for itself.
        }
        let ch = src[i..].chars().next()?;
        let mut buf = [0; 2];
        out.extend_from_slice(ch.encode_utf16(&mut buf));
        i += ch.len_utf8();
    }
    None
}

/// Mirror of wineserver's `dump_strW`. `quote` holds the characters to escape besides `\`.
fn escape(text: &str, quote: [char; 2]) -> String {
    const CONTROL: &[u8; 32] = b".......abtnvfr.............e....";
    let units = text.encode_utf16().collect::<Vec<_>>();
    let mut out = String::new();
    for (i, &unit) in units.iter().enumerate() {
        let next = units.get(i + 1).copied();
        if unit > 127 {
            let padded = next.is_some_and(|n| n < 128 && (n as u8).is_ascii_hexdigit());
            out.push_str(&if padded {
                format!("\\x{unit:04x}")
            } else {
                format!("\\x{unit:x}")
            });
        } else if unit < 32 {
            let letter = CONTROL[usize::from(unit)];
            if letter != b'.' {
                out.push('\\');
                out.push(char::from(letter));
            } else if next.is_some_and(|n| (u16::from(b'0')..=u16::from(b'7')).contains(&n)) {
                out.push_str(&format!("\\{unit:03o}"));
            } else {
                out.push_str(&format!("\\{unit:o}"));
            }
        } else {
            let ch = char::from(unit as u8);
            if ch == '\\' || quote.contains(&ch) {
                out.push('\\');
            }
            out.push(ch);
        }
    }
    out
}

/// Parses `"name"=data` or `@=data`, pulling continuation lines from `lines` for `hex` data.
fn parse_value<'a>(line: &'a str, lines: &mut impl Iterator<Item = &'a str>) -> Option<RegValue> {
    let (name, rest) = match line.strip_prefix('@') {
        Some(rest) => (String::new(), rest),
        None => {
            let (units, used) = unescape(&line[1..], b'"')?;
            (String::from_utf16(&units).ok()?, &line[1 + used..])
        }
    };
    let data = parse_data(rest.trim_start().strip_prefix('=')?.trim_start(), lines)?;
    Some(RegValue { name, data })
}

fn parse_data<'a>(text: &'a str, lines: &mut impl Iterator<Item = &'a str>) -> Option<RegData> {
    for (tag, code) in [("\"", 1), ("str:\"", 1), ("str(2):\"", 2), ("str(7):\"", 7)] {
        if let Some(rest) = text.strip_prefix(tag) {
            return Some(string_data(code, unescape(rest, b'"')?.0));
        }
    }
    if let Some(rest) = text.strip_prefix("dword:") {
        return u32::from_str_radix(rest.trim(), 16)
            .ok()
            .map(RegData::Dword);
    }
    let (code, rest) = match text.strip_prefix("hex:") {
        Some(rest) => (3, rest),
        None => {
            let (code, rest) = text.strip_prefix("hex(")?.split_once("):")?;
            (u32::from_str_radix(code, 16).ok()?, rest)
        }
    };
    let bytes = parse_hex(rest, lines)?;
    if code == 4
        && let Ok(raw) = <[u8; 4]>::try_from(bytes.as_slice())
    {
        return Some(RegData::Dword(u32::from_le_bytes(raw)));
    }
    if code == 11
        && let Ok(raw) = <[u8; 8]>::try_from(bytes.as_slice())
    {
        return Some(RegData::Qword(u64::from_le_bytes(raw)));
    }
    Some(match code {
        3 => RegData::Binary(bytes),
        _ => RegData::Other { code, bytes },
    })
}

/// Comma-separated hex bytes; a trailing `\` continues on the next line.
fn parse_hex<'a>(mut text: &'a str, lines: &mut impl Iterator<Item = &'a str>) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        let (body, more) = match text.trim_end().strip_suffix('\\') {
            Some(body) => (body, true),
            None => (text, false),
        };
        for token in body.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            bytes.push(u8::from_str_radix(token, 16).ok()?);
        }
        if !more {
            return Some(bytes);
        }
        text = lines.next()?.trim_start();
    }
}

/// Builds the typed data of a text string. Text that is not valid UTF-16, or a `str(7)` that
/// does not end in `\0`, stays [`RegData::Other`] so nothing is lost.
fn string_data(code: u32, units: Vec<u16>) -> RegData {
    if let Ok(text) = String::from_utf16(&units) {
        match code {
            1 => return RegData::Sz(text),
            2 => return RegData::ExpandSz(text),
            _ if text.is_empty() => return RegData::MultiSz(Vec::new()),
            _ => {
                if let Some(body) = text.strip_suffix('\0') {
                    return RegData::MultiSz(body.split('\0').map(String::from).collect());
                }
            }
        }
    }
    let mut bytes = units
        .iter()
        .flat_map(|u| u.to_le_bytes())
        .collect::<Vec<_>>();
    bytes.extend([0, 0]);
    RegData::Other { code, bytes }
}

/// Mirror of wineserver's `dump_value`, including the line wrapping of `hex` data.
fn value_text(value: &RegValue) -> String {
    let (mut out, mut column) = if value.name.is_empty() {
        ("@=".to_owned(), 2)
    } else {
        let name = escape(&value.name, ['"', '"']);
        (format!("\"{name}\"="), name.len() + 3)
    };
    let (tag, bytes) = match &value.data {
        RegData::Sz(text) => return format!("{out}\"{}\"\n", escape(text, ['"', '"'])),
        RegData::ExpandSz(text) => {
            return format!("{out}str(2):\"{}\"\n", escape(text, ['"', '"']));
        }
        RegData::MultiSz(list) => {
            let text = list
                .iter()
                .flat_map(|s| [s.as_str(), "\0"])
                .collect::<String>();
            return format!("{out}str(7):\"{}\"\n", escape(&text, ['"', '"']));
        }
        RegData::Dword(dword) => return format!("{out}dword:{dword:08x}\n"),
        RegData::Qword(qword) => ("hex(b):".to_owned(), qword.to_le_bytes().to_vec()),
        RegData::Binary(bytes) => ("hex:".to_owned(), bytes.clone()),
        RegData::Other { code: 3, bytes } => ("hex:".to_owned(), bytes.clone()),
        RegData::Other { code, bytes } => (format!("hex({code:x}):"), bytes.clone()),
    };
    out.push_str(&tag);
    column += tag.len();
    for (i, byte) in bytes.iter().enumerate() {
        out.push_str(&format!("{byte:02x}"));
        column += 2;
        if i + 1 < bytes.len() {
            out.push(',');
            column += 1;
            if column > 76 {
                out.push_str("\\\n  ");
                column = 2;
            }
        }
    }
    out.push('\n');
    out
}

/// Text of a key block: header line, `#time`, values and the blank separator line.
fn key_text(relative: &str, values: &[RegValue], unix_secs: u64) -> String {
    let path = relative
        .split('\\')
        .map(|element| escape(element, ['[', ']']))
        .collect::<Vec<_>>()
        .join("\\\\");
    let ticks = (unix_secs + SECS_1601_TO_1970) * 10_000_000;
    let mut out = format!(
        "[{path}] {unix_secs}\n#time={:x}{:08x}\n",
        ticks >> 32,
        ticks & 0xffff_ffff
    );
    values.iter().for_each(|v| out.push_str(&value_text(v)));
    out.push('\n');
    out
}
