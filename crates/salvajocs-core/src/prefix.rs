//! Wine prefix validation, Wine user choice and Steam root discovery (ADR-0007).

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Directories under `drive_c/users` that are never the game user. A prefix run as root has a
/// `root` directory.
const SYSTEM_USERS: &[&str] = &["Public", "root"];

/// The user Proton creates in every prefix.
const PROTON_USER: &str = "steamuser";

/// Why a path is not usable as a Wine prefix.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PrefixError {
    /// The path lacks `drive_c/users`, or all of `system.reg`, `user.reg` and `dosdevices`.
    #[error("not a Wine prefix: {}", .0.display())]
    NotAPrefix(PathBuf),
    /// `drive_c/users` has no user directory: the game has never run in this prefix.
    #[error("no user directory in prefix {}", .0.display())]
    NoUser(PathBuf),
    /// Several non-system users and none is `steamuser`.
    #[error("several users in prefix {}: {}", prefix.display(), users.join(", "))]
    Ambiguous { prefix: PathBuf, users: Vec<String> },
    /// Reading the prefix failed.
    #[error("cannot read {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Checks that `path` looks like a Wine prefix: `drive_c/users` exists and one of
/// `system.reg`, `user.reg` or `dosdevices` does too. The user is not examined.
pub fn validate_prefix(path: &Path) -> Result<(), PrefixError> {
    let mut has_marker = false;
    for marker in ["system.reg", "user.reg", "dosdevices"] {
        has_marker |= exists(&path.join(marker))?;
    }
    if has_marker && exists(&path.join("drive_c/users"))? {
        Ok(())
    } else {
        Err(PrefixError::NotAPrefix(path.to_path_buf()))
    }
}

/// The first ancestor of `path` (itself included) that is a valid Wine prefix.
pub fn enclosing_prefix(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|dir| validate_prefix(dir).is_ok())
        .map(Path::to_path_buf)
}

/// Like `Path::try_exists`, but a missing path or a non-directory parent is `false`, and any
/// other error (such as permission denied) is `PrefixError::Io`.
fn exists(path: &Path) -> Result<bool, PrefixError> {
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(false)
        }
        Err(source) => Err(PrefixError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Returns the Wine user of a prefix: the only non-system user directory, else `steamuser`.
/// The returned name has the case of the directory on disk.
pub fn wine_user(prefix: &Path) -> Result<String, PrefixError> {
    validate_prefix(prefix)?;
    let users_dir = prefix.join("drive_c/users");
    let io_err = |source| PrefixError::Io {
        path: users_dir.clone(),
        source,
    };
    let mut users = Vec::new();
    for entry in fs::read_dir(&users_dir).map_err(io_err)? {
        let entry = entry.map_err(io_err)?;
        // A name that is not UTF-8 cannot be used in a path template, so it cannot be the user.
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let is_system = SYSTEM_USERS.iter().any(|s| s.eq_ignore_ascii_case(&name));
        if !is_system && entry.path().is_dir() {
            users.push(name);
        }
    }
    users.sort();
    match users.len() {
        0 => Err(PrefixError::NoUser(prefix.to_path_buf())),
        1 => Ok(users.remove(0)),
        _ => match users
            .iter()
            .position(|u| u.eq_ignore_ascii_case(PROTON_USER))
        {
            Some(i) => Ok(users.swap_remove(i)),
            None => Err(PrefixError::Ambiguous {
                prefix: prefix.to_path_buf(),
                users,
            }),
        },
    }
}

/// Existing Steam roots among the default install locations, without duplicates (on Linux
/// `~/.steam/steam` is a link to `~/.local/share/Steam`).
pub fn steam_roots() -> Vec<PathBuf> {
    existing_steam_roots(default_steam_candidates())
}

#[cfg(windows)]
fn default_steam_candidates() -> Vec<PathBuf> {
    let program_files = std::env::var_os("ProgramFiles(x86)")
        .map_or_else(|| PathBuf::from(r"C:\Program Files (x86)"), PathBuf::from);
    vec![program_files.join("Steam")]
}

#[cfg(not(windows))]
fn default_steam_candidates() -> Vec<PathBuf> {
    dirs::home_dir().map_or_else(Vec::new, |home| {
        vec![
            home.join(".steam/steam"),
            home.join(".local/share/Steam"),
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        ]
    })
}

/// Keeps the candidates that are Steam roots (they contain `steamapps`), dropping any that
/// resolve to a directory already kept.
fn existing_steam_roots(candidates: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = Vec::new();
    let mut roots = Vec::new();
    for candidate in candidates {
        if !candidate.join("steamapps").is_dir() {
            continue;
        }
        let real = fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if !seen.contains(&real) {
            seen.push(real);
            roots.push(candidate);
        }
    }
    roots
}

/// The Proton prefix of a Steam game, `<steam_root>/steamapps/compatdata/<app_id>/pfx`, when it
/// exists and is a valid prefix.
pub fn proton_prefix(steam_root: &Path, app_id: u64) -> Option<PathBuf> {
    let pfx = steam_root
        .join("steamapps/compatdata")
        .join(app_id.to_string())
        .join("pfx");
    validate_prefix(&pfx).ok().map(|()| pfx)
}

/// The app ids with an entry in `<steam_root>/steamapps/compatdata`; unreadable gives none.
pub(crate) fn compat_app_ids(steam_root: &Path) -> HashSet<u64> {
    fs::read_dir(steam_root.join("steamapps/compatdata")).map_or_else(
        |_| HashSet::new(),
        |entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
                .collect()
        },
    )
}

/// Whether any process is named `wineserver*` (Debian/Ubuntu use `wineserver64`). Reads `/proc/*/comm`, so it is `false` where
/// there is no procfs (Windows).
pub fn wineserver_running() -> bool {
    wineserver_in(Path::new("/proc"))
}

fn wineserver_in(proc_dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(proc_dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        fs::read_to_string(entry.path().join("comm"))
            .is_ok_and(|comm| comm.starts_with("wineserver"))
    })
}

#[cfg(test)]
mod tests;
