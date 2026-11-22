//! File helpers shared by the modules that keep small state files.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;

/// Writes `contents` to `path` via a synced sibling `<path>.<pid>.tmp` and a rename, so a crash
/// never leaves a truncated file. The process id keeps two running instances from writing the
/// same temp file. The temp file is removed when any step fails.
pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let written = File::create(&tmp).and_then(|mut file| {
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    });
    if written.is_err() {
        // Best effort: the write's own error is the one to report.
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Removes the file `path`; one that is not there is no error.
pub(crate) fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Reads all of `body`, or `None` when its declared length `declared` or its actual length
/// exceeds `limit`: the declared one is refused unread, the actual one after `limit + 1` bytes.
pub(crate) fn read_capped(
    declared: Option<u64>,
    body: impl Read,
    limit: u64,
) -> io::Result<Option<Vec<u8>>> {
    if declared.is_some_and(|len| len > limit) {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    body.take(limit + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= limit).then_some(bytes))
}

#[cfg(test)]
mod tests;
