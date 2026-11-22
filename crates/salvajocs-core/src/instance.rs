//! One salvajocs at a time: two instances would share the backup stage mirror in the cache
//! directory, and one could rewrite files while the other's backup reads them.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;

/// The file in the cache directory whose lock marks the running instance.
const LOCK_FILE: &str = "instance.lock";

/// The lock of the running instance, released when this is dropped or the process ends.
#[derive(Debug)]
pub struct InstanceLock {
    file: File,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // Unlocked by hand so a child that inherited the handle cannot keep the lock; the OS
        // releases it at exit anyway, so a failure here needs no handling.
        let _ = self.file.unlock();
    }
}

/// Takes the lock of the instance that uses `cache_dir`, creating the directory if need be.
/// `None` means another instance holds it. The file is never written, renamed or removed.
pub fn lock(cache_dir: &Path) -> io::Result<Option<InstanceLock>> {
    fs::create_dir_all(cache_dir)?;
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(cache_dir.join(LOCK_FILE))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(InstanceLock { file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(err)) => Err(err),
    }
}

#[cfg(test)]
mod tests;
