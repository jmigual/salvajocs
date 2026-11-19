//! What the app reads from its surroundings: directories, the credential store, the manifest
//! download and the update check. Tests substitute their own so nothing touches the real keyring
//! or the network.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

use salvajocs_core::config;
use salvajocs_core::manifest::fetch::{self, FetchError, Freshness};
use salvajocs_core::prefix;
use salvajocs_core::scan::Environment;
use salvajocs_core::secret::{self, SecretError, SecretKey};
use salvajocs_core::update::{self, Release, UpdateError};

/// The credential store of one repository scope (`AppConfig::secret_scope`).
pub trait SecretStore: Send + Sync {
    /// Reads a secret; `None` when there is none.
    fn get(&self, scope: &str, key: &SecretKey) -> Result<Option<String>, SecretError>;
    /// Stores a secret, replacing any previous value.
    fn set(&self, scope: &str, key: &SecretKey, value: &str) -> Result<(), SecretError>;
    /// Removes a secret; removing a missing one succeeds.
    fn delete(&self, scope: &str, key: &SecretKey) -> Result<(), SecretError>;
}

/// The OS credential store, through `salvajocs_core::secret`.
pub struct SystemSecrets;

impl SecretStore for SystemSecrets {
    fn get(&self, scope: &str, key: &SecretKey) -> Result<Option<String>, SecretError> {
        secret::get(scope, key)
    }

    fn set(&self, scope: &str, key: &SecretKey, value: &str) -> Result<(), SecretError> {
        secret::set(scope, key, value)
    }

    fn delete(&self, scope: &str, key: &SecretKey) -> Result<(), SecretError> {
        secret::delete(scope, key)
    }
}

/// A store that keeps secrets in memory. [`MemorySecrets::full`] refuses every write the way the
/// Windows store refuses an over-long value.
#[cfg(test)]
#[derive(Default)]
pub struct MemorySecrets {
    entries: Mutex<Vec<(String, SecretKey, String)>>,
    full: bool,
}

#[cfg(test)]
impl MemorySecrets {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// A store whose `set` always fails with [`SecretError::TooLong`].
    pub fn full() -> Self {
        Self {
            full: true,
            ..Self::default()
        }
    }
}

#[cfg(test)]
impl SecretStore for MemorySecrets {
    fn get(&self, scope: &str, key: &SecretKey) -> Result<Option<String>, SecretError> {
        let entries = self
            .entries
            .lock()
            .expect("no panic while the lock is held");
        Ok(entries
            .iter()
            .find(|(s, k, _)| s == scope && k == key)
            .map(|(_, _, value)| value.clone()))
    }

    fn set(&self, scope: &str, key: &SecretKey, value: &str) -> Result<(), SecretError> {
        if self.full {
            return Err(SecretError::TooLong);
        }
        self.delete(scope, key)?;
        let mut entries = self
            .entries
            .lock()
            .expect("no panic while the lock is held");
        entries.push((scope.to_string(), key.clone(), value.to_string()));
        Ok(())
    }

    fn delete(&self, scope: &str, key: &SecretKey) -> Result<(), SecretError> {
        let mut entries = self
            .entries
            .lock()
            .expect("no panic while the lock is held");
        entries.retain(|(s, k, _)| !(s == scope && k == key));
        Ok(())
    }
}

/// Downloads the manifest into a cache directory.
pub type RefreshManifest = fn(&Path) -> Result<Freshness, FetchError>;

/// Asks for a release newer than the given version (`update::check`).
pub type CheckUpdate = fn(&str) -> Result<Option<Release>, UpdateError>;

/// Shows a folder in the file manager; returns once the opener is spawned.
pub type OpenFolder = Arc<dyn Fn(&Path) -> io::Result<()> + Send + Sync>;

/// Everything a worker thread needs from the environment; cheap to clone.
#[derive(Clone)]
pub struct Env {
    /// Holds `config.yaml`.
    pub config_dir: PathBuf,
    /// Holds the manifest cache.
    pub cache_dir: PathBuf,
    /// Where passwords and secret options are kept.
    pub secrets: Arc<dyn SecretStore>,
    /// Revalidates the manifest cache.
    pub refresh_manifest: RefreshManifest,
    /// The machine scans and restores look at; `None` detects it (`Environment::detect`) in
    /// the job that needs it.
    pub environment: Option<Environment>,
    /// Whether a wineserver runs (`prefix::wineserver_running`).
    pub wineserver_running: fn() -> bool,
    /// Looks for a newer release at start; `None` skips it, as everywhere but on Windows and in an
    /// AppImage (ADR-0013, ADR-0022).
    pub check_update: Option<CheckUpdate>,
    /// Whether the Windows installer installed the running app, so it can update itself.
    pub installed_by_installer: fn() -> bool,
    /// Starts a downloaded installer (`update::run_installer`).
    pub run_installer: fn(&Path) -> Result<(), UpdateError>,
    /// The AppImage the app runs from (`update::running_appimage`), which an update replaces.
    pub appimage: Option<PathBuf>,
    /// Starts the replaced AppImage (`update::restart_appimage`).
    pub restart_appimage: fn(&Path) -> Result<(), UpdateError>,
    /// Lists the rclone remotes (`config::rclone_remotes`).
    pub rclone_remotes: fn() -> io::Result<Vec<String>>,
    /// Asks the user for a folder; `None` when they cancel. Blocks, so call it from a worker.
    pub pick_folder: Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>,
    /// Shows a folder in the file manager.
    pub open_folder: OpenFolder,
}

impl Env {
    /// The real environment, or `None` when the OS has no config or cache directory.
    pub fn system() -> Option<Self> {
        let appimage = running_appimage();
        Some(Self {
            config_dir: config::default_dir()?,
            cache_dir: dirs::cache_dir()?.join("salvajocs"),
            secrets: Arc::new(SystemSecrets),
            refresh_manifest: fetch::refresh,
            environment: None,
            wineserver_running: prefix::wineserver_running,
            check_update: (cfg!(windows) || appimage.is_some())
                .then_some(update::check as CheckUpdate),
            installed_by_installer,
            run_installer: update::run_installer,
            appimage,
            restart_appimage: update::restart_appimage,
            rclone_remotes: config::rclone_remotes,
            pick_folder: Arc::new(|| rfd::FileDialog::new().pick_folder()),
            open_folder: Arc::new(open_folder),
        })
    }
}

fn open_folder(path: &Path) -> io::Result<()> {
    if !path.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "not a folder"));
    }
    // Explorer misreads `/`, which glob output and the drive pattern mix in with `\`.
    #[cfg(windows)]
    let path = &path.components().collect::<PathBuf>();
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = Command::new(program)
        .arg(path)
        .spawn()
        .map_err(|err| io::Error::new(err.kind(), format!("{program}: {err}")))?;
    // Reaped on a thread so the click does not wait, and no zombie is left behind.
    std::thread::spawn(move || child.wait());
    Ok(())
}

fn installed_by_installer() -> bool {
    std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(Path::parent)
        .is_some_and(update::installed_by_installer)
}

fn running_appimage() -> Option<PathBuf> {
    update::running_appimage(
        &std::env::current_exe().ok()?,
        std::env::var_os("APPIMAGE")?.into(),
        Path::new(&std::env::var_os("APPDIR")?),
    )
}
