//! What makes a changed file differ from its backup (ADR-0024): both copies' size, mtime and
//! SHA-256, how many lines end in CRLF and in LF, and for a text file the changed lines.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use std::time::{Duration, SystemTime};

use similar::{ChangeTag, TextDiff};

use super::BackedUp;
use crate::backup::{sha256_bytes, sha256_file};
use crate::layout::FILES_DIR;
use crate::storage::{Storage, StorageError};

/// The largest file, in bytes, that is read to look for text unless the caller lifts the cap
/// (see [`details`]). A copy over it is hashed (this PC's) or listed (the backup's), never
/// downloaded, and gets no diff.
pub const TEXT_CAP: u64 = 10 << 20;

/// Lines of context around each changed hunk.
const CONTEXT: usize = 3;

/// How long the diff may search for a shorter one; a longer search is cut off, and the diff is
/// then correct but less minimal.
const DIFF_TIMEOUT: Duration = Duration::from_secs(1);

/// How many of a text's lines end in each way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct LineEndings {
    /// Lines ending in CRLF, as Windows writes them.
    pub crlf: usize,
    /// Lines ending in a bare LF, as Linux writes them.
    pub lf: usize,
    /// Lines ending in a bare CR, as old Mac systems wrote them.
    pub cr: usize,
}

/// This PC's copy of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LocalFile {
    /// Size in bytes.
    pub size: u64,
    /// The file's mtime, if the system has one.
    pub modified: Option<SystemTime>,
    /// The SHA-256 of its content, in hex.
    pub sha256: String,
    /// Its line endings; `None` when it is not read as text (see [`Content`]).
    pub line_endings: Option<LineEndings>,
    /// It is text that starts with a byte order mark, as some Windows editors write.
    pub bom: bool,
}

/// Where the backup's SHA-256 of a file comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BackupHash {
    /// The backup's hash list, which salvajocs wrote when it made the backup.
    List(String),
    /// Computed from the content read from the repository, as the backup has no hash list.
    Content(String),
    /// Neither: the backup has no hash list and the content was not read.
    Unknown,
}

impl BackupHash {
    /// The hash in hex, if there is one.
    pub fn hex(&self) -> Option<&str> {
        match self {
            BackupHash::List(hex) | BackupHash::Content(hex) => Some(hex),
            BackupHash::Unknown => None,
        }
    }
}

/// The backup's copy of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BackupFile {
    /// Size in bytes.
    pub size: u64,
    /// The mtime the backup stored, if any.
    pub modified: Option<SystemTime>,
    /// The SHA-256 of its content.
    pub sha256: BackupHash,
    /// Its line endings; `None` when it is not read as text (see [`Content`]).
    pub line_endings: Option<LineEndings>,
    /// It is text that starts with a byte order mark, as some Windows editors write.
    pub bom: bool,
}

/// What a line of a [`Hunk`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LineKind {
    /// In both copies, shown around a change.
    Context,
    /// Only in the backup.
    Removed,
    /// Only on this PC.
    Added,
}

/// One line of a [`Hunk`], without its line ending.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiffLine {
    /// Whether it is in both copies or one.
    pub kind: LineKind,
    /// The line's number in the backup's copy, from 1; `None` for an added line.
    pub backup_number: Option<usize>,
    /// The line's number on this PC, from 1; `None` for a removed line.
    pub local_number: Option<usize>,
    /// The line's text.
    pub text: String,
    /// The line is the last of its text and has no line ending, as a unified diff marks it.
    pub missing_newline: bool,
}

/// A run of changed lines with their context.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Hunk {
    /// The lines in order.
    pub lines: Vec<DiffLine>,
}

/// How the content of the two copies compares.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Content {
    /// The same bytes.
    Same,
    /// Text that is the same once every line ending is read as LF and the byte order mark is left
    /// out; each field says which of the two the copies differ in.
    FormatOnly {
        /// The line endings differ.
        line_endings: bool,
        /// Only one copy starts with a byte order mark.
        bom: bool,
    },
    /// Text that differs, as the backup's lines against this PC's, after reading every line
    /// ending as LF and leaving out the byte order mark.
    Diff(Vec<Hunk>),
    /// A copy is not text: not valid UTF-8, or it holds a NUL byte.
    NotText,
    /// A copy is over the cap, so nothing was compared.
    TooLarge,
    /// This PC's copy could not be read.
    Unavailable,
}

/// What [`details`] found out about a file and its backup.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileDetails {
    /// This PC's copy, or why it could not be read.
    pub local: Result<LocalFile, String>,
    /// The backup's copy.
    pub backup: BackupFile,
    /// How the two compare.
    pub content: Content,
}

/// The content of `backed_up` in the repository, to hand to [`details`]. Nothing is downloaded,
/// and `None` is returned, when either copy is over `cap` bytes (`None`: no cap, as [`TEXT_CAP`]
/// is the usual one) or the file at `local` cannot be statted. `slug` is the game's.
///
/// # Errors
/// The repository error when the file cannot be read from it.
pub fn read_backed_up(
    storage: &mut Storage,
    slug: &str,
    backed_up: &BackedUp,
    local: &Path,
    cap: Option<u64>,
) -> Result<Option<Vec<u8>>, StorageError> {
    let fits = |size: u64| cap.is_none_or(|cap| size <= cap);
    let local_fits = fs::metadata(local).is_ok_and(|meta| fits(meta.len()));
    if !fits(backed_up.size) || !local_fits {
        return Ok(None);
    }
    let path = format!("/salvajocs/{slug}/{FILES_DIR}/{}", backed_up.path);
    storage.read_file(&backed_up.snapshot.id, &path).map(Some)
}

/// How this PC's file at `local` differs from `backed_up`, whose content `backup` is as
/// [`read_backed_up`] gave it: `None` when it was not read. Reads and hashes `local`, keeping its
/// bytes only within `cap`, the cap `backup` was read with.
pub fn details(
    local: &Path,
    backed_up: &BackedUp,
    backup: Option<&[u8]>,
    cap: Option<u64>,
) -> FileDetails {
    let backup_text = backup.and_then(text);
    let backup_hash = match (&backed_up.sha256, backup) {
        (Some(list), _) => BackupHash::List(list.clone()),
        (None, Some(bytes)) => BackupHash::Content(sha256_bytes(bytes)),
        (None, None) => BackupHash::Unknown,
    };
    let backup_file = BackupFile {
        size: backed_up.size,
        modified: backed_up.modified,
        sha256: backup_hash,
        line_endings: backup_text.map(line_endings),
        bom: backup_text.is_some_and(has_bom),
    };
    let (mut file, local_bytes) = match read_local(local, cap) {
        Ok(read) => read,
        Err(err) => {
            return FileDetails {
                local: Err(err.to_string()),
                backup: backup_file,
                content: Content::Unavailable,
            };
        }
    };
    let local_text = local_bytes.as_deref().and_then(text);
    file.line_endings = local_text.map(line_endings);
    file.bom = local_text.is_some_and(has_bom);
    let content = if backup_file.sha256.hex() == Some(file.sha256.as_str()) {
        Content::Same
    } else if backup.is_some() && local_bytes.is_some() {
        compare_text(backup_text, local_text)
    } else {
        Content::TooLarge
    };
    FileDetails {
        local: Ok(file),
        backup: backup_file,
        content,
    }
}

/// The file at `path` as this PC has it, without line endings, and its bytes when they are
/// within `cap`.
fn read_local(path: &Path, cap: Option<u64>) -> io::Result<(LocalFile, Option<Vec<u8>>)> {
    let meta = fs::metadata(path)?;
    let modified = meta.modified().ok();
    let mut bytes = Vec::new();
    // One byte past the cap tells a file over it from one that fills it.
    let limit = cap.map_or(u64::MAX, |cap| cap.saturating_add(1));
    File::open(path)?.take(limit).read_to_end(&mut bytes)?;
    let (size, sha256, bytes) = if cap.is_some_and(|cap| bytes.len() as u64 > cap) {
        (meta.len(), sha256_file(path)?, None)
    } else {
        (bytes.len() as u64, sha256_bytes(&bytes), Some(bytes))
    };
    let file = LocalFile {
        size,
        modified,
        sha256,
        line_endings: None,
        bom: false,
    };
    Ok((file, bytes))
}

/// `bytes` as text: valid UTF-8 without a NUL byte.
fn text(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes)
        .ok()
        .filter(|text| !text.contains('\0'))
}

/// How many lines of `text` end in CRLF and in a bare LF.
fn line_endings(text: &str) -> LineEndings {
    let crlf = text.matches("\r\n").count();
    LineEndings {
        crlf,
        lf: text.matches('\n').count() - crlf,
        cr: text.matches('\r').count() - crlf,
    }
}

/// The byte order mark some Windows editors write at the start of a UTF-8 file.
const BOM: char = '\u{feff}';

/// Whether `text` starts with a byte order mark.
fn has_bom(text: &str) -> bool {
    text.starts_with(BOM)
}

/// `text` without its byte order mark.
fn without_bom(text: &str) -> &str {
    text.strip_prefix(BOM).unwrap_or(text)
}

/// `text` with CRLF and a bare CR read as LF, so each line ends in at most one `\n`.
fn normalized(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The comparison of the backup's `old` text and this PC's `new`; a copy that is `None` is not
/// text.
fn compare_text(old: Option<&str>, new: Option<&str>) -> Content {
    let (Some(old), Some(new)) = (old, new) else {
        return Content::NotText;
    };
    let bom = has_bom(old) != has_bom(new);
    let (old, new) = (without_bom(old), without_bom(new));
    let (old_lf, new_lf) = (normalized(old), normalized(new));
    if old_lf == new_lf {
        return Content::FormatOnly {
            line_endings: old != new,
            bom,
        };
    }
    let diff = TextDiff::configure()
        .timeout(DIFF_TIMEOUT)
        .diff_lines(&old_lf, &new_lf);
    let hunks = (diff.grouped_ops(CONTEXT).iter())
        .map(|group| Hunk {
            lines: (group.iter().flat_map(|op| diff.iter_changes(op)))
                .map(|change| DiffLine {
                    kind: match change.tag() {
                        ChangeTag::Equal => LineKind::Context,
                        ChangeTag::Delete => LineKind::Removed,
                        ChangeTag::Insert => LineKind::Added,
                    },
                    backup_number: change.old_index().map(|at| at + 1),
                    local_number: change.new_index().map(|at| at + 1),
                    text: (change.value().strip_suffix('\n'))
                        .unwrap_or(change.value())
                        .to_owned(),
                    missing_newline: change.missing_newline(),
                })
                .collect(),
        })
        .collect();
    Content::Diff(hunks)
}

#[cfg(test)]
mod tests;
