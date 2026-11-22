use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// `<cache_dir>/<name>/<process id>`, so two running apps never clear each other's files.
pub(super) fn own_dir(cache_dir: &Path, name: &str) -> PathBuf {
    cache_dir.join(name).join(std::process::id().to_string())
}

/// Removes `dir` and everything below it; a missing dir is fine. A restored dir can be
/// read-only, which would block removing its entries, so every real dir is made writable first.
pub(super) fn remove_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    make_writable(dir)?;
    match fs::remove_dir_all(dir) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// Adds owner write and search to `path` and every dir below it, without following symlinks,
/// so nothing outside `path` changes.
#[cfg(unix)]
fn make_writable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = match fs::symlink_metadata(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        metadata => metadata?,
    };
    if !metadata.is_dir() {
        return Ok(());
    }
    let mode = metadata.permissions().mode() | 0o700;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    for entry in fs::read_dir(path)? {
        make_writable(&entry?.path())?;
    }
    Ok(())
}
