//! Repository storage over `rustic_core` (ADR-0003). The only module that names rustic types.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use rustic_backend::{BackendOptions, choose::SupportedBackend, util::location_to_type_and_path};
use rustic_core::{
    BackupOptions, ConfigOptions, Credentials, IndexedFullStatus, KeyOptions, LocalDestination,
    LsOptions, PathList, Repository, RepositoryBackends, RepositoryOptions, RestoreOptions,
    RusticError, SnapshotOptions, repofile::SnapshotFile,
};

/// zstd's own default level; rustic cannot unset a compression level once it is stored.
const ZSTD_DEFAULT_LEVEL: i32 = 3;

/// How long a backup waits for directory listings to catch up with the files (ADR-0003).
const LISTING_SETTLE_POLLS: u32 = 50;
const LISTING_SETTLE_INTERVAL: Duration = Duration::from_millis(20);

/// Errors from [`Storage`] and [`probe`].
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The compression level is not 0, 1..=22 or -7..=-1.
    #[error("compression level {0} is out of range")]
    InvalidCompression(i32),
    /// The password does not unlock the repository.
    #[error("wrong repository password")]
    WrongPassword,
    /// A path cannot be passed to the repository because it is not valid UTF-8.
    #[error("path is not valid UTF-8: {0}")]
    NonUtf8Path(PathBuf),
    /// The repository holds an entry whose path would leave the restore destination.
    #[error("unsafe path in snapshot: {0}")]
    UnsafeEntryPath(PathBuf),
    /// Any other repository or backend failure, as rustic reports it.
    #[error("repository error: {0}")]
    Backend(String),
}

fn backend(err: impl AsRef<RusticError>) -> StorageError {
    StorageError::Backend(err.as_ref().display_log())
}

/// Where a repository lives: a rustic repository string (a plain path, `local:`, `rest:`,
/// `rclone:`, `opendal:<service>`) and the backend options for it.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct RepoLocation {
    /// The repository string.
    pub repository: String,
    /// Backend options, such as the opendal service configuration.
    pub options: BTreeMap<String, String>,
}

/// Debug output lists only the option keys: the values may be secrets.
impl fmt::Debug for RepoLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RepoLocation")
            .field("repository", &self.repository)
            .field("option_keys", &self.options.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl RepoLocation {
    /// The directory holding the repository when it is on the local filesystem, by rustic's own
    /// parsing of the repository string.
    fn local_dir(&self) -> Option<PathBuf> {
        match location_to_type_and_path(&self.repository) {
            Ok((SupportedBackend::Local, dir)) => Some(PathBuf::from(dir.as_str())),
            _ => None,
        }
    }

    fn backends(&self) -> Result<RepositoryBackends, StorageError> {
        BackendOptions::default()
            .repository(&self.repository)
            .options(self.options.clone())
            .to_backends()
            .map_err(backend)
    }
}

fn repository(backends: &RepositoryBackends) -> Result<Repository<()>, StorageError> {
    Repository::new(&RepositoryOptions::default(), backends).map_err(backend)
}

fn open_indexed(
    backends: &RepositoryBackends,
    password: &str,
) -> Result<Repository<IndexedFullStatus>, StorageError> {
    repository(backends)?
        .open(&Credentials::password(password))
        .and_then(Repository::to_indexed)
        .map_err(|err| {
            if err.is_incorrect_password() {
                StorageError::WrongPassword
            } else {
                backend(err)
            }
        })
}

/// What [`probe`] found at a [`RepoLocation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoState {
    /// A repository exists and the password unlocks it.
    Ready,
    /// No repository has been initialized there.
    Missing,
    /// A repository exists but the password does not unlock it.
    WrongPassword,
    /// The location could not be inspected; the rustic message is attached.
    Error(String),
}

/// A repository's zstd compression setting. It belongs to the repository, so every machine
/// writing to it shares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// No compression.
    Off,
    /// zstd's default level.
    Default,
    /// An explicit zstd level: 1..=22 or -7..=-1.
    Level(i32),
}

impl Compression {
    /// Reads the first-run integer: `None` is the default, `0` is off, `1..=22` and `-7..=-1`
    /// are levels. Anything else is [`StorageError::InvalidCompression`].
    pub fn from_field(field: Option<i32>) -> Result<Self, StorageError> {
        let compression = match field {
            None => Self::Default,
            Some(0) => Self::Off,
            Some(level) => Self::Level(level),
        };
        compression.config_level()?;
        Ok(compression)
    }

    /// The value for rustic's `set_compression`, `None` meaning "leave unset".
    fn config_level(self) -> Result<Option<i32>, StorageError> {
        match self {
            Self::Off => Ok(Some(0)),
            Self::Default => Ok(None),
            Self::Level(level) if (1..=22).contains(&level) || (-7..=-1).contains(&level) => {
                Ok(Some(level))
            }
            Self::Level(level) => Err(StorageError::InvalidCompression(level)),
        }
    }

    fn from_config(level: Option<i32>) -> Self {
        match level {
            None => Self::Default,
            Some(0) => Self::Off,
            Some(level) => Self::Level(level),
        }
    }
}

/// A snapshot as the rest of the app sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotInfo {
    /// The full hex snapshot id.
    pub id: String,
    /// When the snapshot was taken.
    pub time: SystemTime,
    /// The host that made the snapshot.
    pub host: String,
    /// The snapshot label (the game title).
    pub label: String,
    /// The snapshot tags.
    pub tags: Vec<String>,
}

fn snapshot_info(snap: &SnapshotFile) -> SnapshotInfo {
    SnapshotInfo {
        id: snap.id.to_hex().to_string(),
        time: snap.time.timestamp().into(),
        host: snap.hostname.clone(),
        label: snap.label.clone(),
        tags: snap.tags.iter().cloned().collect(),
    }
}

/// Inspects `location` with `password` (empty means no password). Never treats an unreadable
/// location as [`RepoState::Missing`].
pub fn probe(location: &RepoLocation, password: &str) -> RepoState {
    let repo = match location
        .backends()
        .and_then(|backends| repository(&backends))
    {
        Ok(repo) => repo,
        Err(err) => return RepoState::Error(err.to_string()),
    };
    match repo.config_id() {
        // rustic's local backend reports "no config" for any `exists()` failure, such as an
        // unreadable directory, so ask the filesystem before calling the repository missing.
        Ok(None) => match location
            .local_dir()
            .map(|dir| fs::exists(dir.join("config")))
        {
            Some(Ok(false)) | None => return RepoState::Missing,
            Some(Ok(true)) => {}
            Some(Err(err)) => return RepoState::Error(err.to_string()),
        },
        Ok(Some(_)) => {}
        Err(err) => return RepoState::Error(backend(err).to_string()),
    }
    match repo.open(&Credentials::password(password)) {
        Ok(_) => RepoState::Ready,
        Err(err) if err.is_incorrect_password() => RepoState::WrongPassword,
        Err(err) => RepoState::Error(backend(err).to_string()),
    }
}

/// An open repository with its index loaded. [`Storage::reindex`] refreshes the index.
pub struct Storage {
    backends: RepositoryBackends,
    password: String,
    repo: Repository<IndexedFullStatus>,
    /// A snapshot was written since the index was loaded, so restoring it needs a reindex.
    stale: bool,
}

impl Storage {
    /// Creates a new repository at `location` with `compression` and opens it.
    pub fn init(
        location: &RepoLocation,
        password: &str,
        compression: Compression,
    ) -> Result<Self, StorageError> {
        let mut config = ConfigOptions::default();
        if let Some(level) = compression.config_level()? {
            config = config.set_compression(level);
        }
        let backends = location.backends()?;
        let repo = repository(&backends)?
            .init(
                &Credentials::password(password),
                &KeyOptions::default(),
                &config,
            )
            .and_then(Repository::to_indexed)
            .map_err(backend)?;
        Ok(Self::new(backends, password, repo))
    }

    /// Opens the existing repository at `location`. Fails with
    /// [`StorageError::WrongPassword`] if `password` does not unlock it.
    pub fn open(location: &RepoLocation, password: &str) -> Result<Self, StorageError> {
        let backends = location.backends()?;
        let repo = open_indexed(&backends, password)?;
        Ok(Self::new(backends, password, repo))
    }

    fn new(
        backends: RepositoryBackends,
        password: &str,
        repo: Repository<IndexedFullStatus>,
    ) -> Self {
        Self {
            backends,
            password: password.to_string(),
            repo,
            stale: false,
        }
    }

    /// The compression stored in the repository config.
    pub fn compression(&self) -> Compression {
        Compression::from_config(self.repo.config().compression)
    }

    /// Changes the repository's compression. [`Compression::Default`] stores zstd's default
    /// level 3, because rustic cannot unset a stored level.
    pub fn set_compression(&mut self, compression: Compression) -> Result<(), StorageError> {
        let level = compression.config_level()?.unwrap_or(ZSTD_DEFAULT_LEVEL);
        self.repo
            .apply_config(&ConfigOptions::default().set_compression(level))
            .map_err(backend)?;
        Ok(())
    }

    /// Reopens the repository from its backend and reloads the index. Call once at the start of
    /// each backup run so parents written by earlier runs are found (ADR-0003).
    pub fn reindex(&mut self) -> Result<(), StorageError> {
        self.repo = open_indexed(&self.backends, &self.password)?;
        self.stale = false;
        Ok(())
    }

    /// Backs up the directory `src` as one snapshot stored under `as_path`, with `tags` (which
    /// must not contain commas) and `label`. Returns `None` when `src` is unchanged since the
    /// parent snapshot and nothing was written.
    pub fn backup_dir(
        &mut self,
        src: &Path,
        as_path: &str,
        tags: &[&str],
        label: &str,
    ) -> Result<Option<SnapshotInfo>, StorageError> {
        let src = src
            .to_str()
            .ok_or_else(|| StorageError::NonUtf8Path(src.to_path_buf()))?;
        wait_for_listing_to_settle(Path::new(src));
        let source = PathList::from_string(src)
            .map_err(backend)?
            .sanitize()
            .map_err(|err| StorageError::Backend(err.to_string()))?;
        debug_assert!(
            tags.iter().all(|tag| !tag.contains(',')),
            "rustic splits tags on commas"
        );
        let snapshot = SnapshotOptions::default()
            .label(label.to_string())
            .add_tags(&tags.join(","))
            .and_then(|opts| opts.to_snapshot())
            .map_err(backend)?;
        let mut options = BackupOptions::default().as_path(PathBuf::from(as_path));
        options.parent_opts = options.parent_opts.skip_if_unchanged(true);
        let snap = self
            .repo
            .backup(&options, &source, snapshot)
            .map_err(backend)?;
        // A skipped snapshot is never saved, so it keeps the null id.
        let written = !snap.id.is_null();
        self.stale |= written;
        Ok(written.then(|| snapshot_info(&snap)))
    }

    /// All snapshots carrying `tag`, newest first.
    pub fn snapshots(&self, tag: &str) -> Result<Vec<SnapshotInfo>, StorageError> {
        let mut found = self
            .repo
            .get_matching_snapshots(|snap| snap.tags.contains(tag))
            .map_err(backend)?;
        found.sort_by(|a, b| b.time.cmp(&a.time));
        Ok(found.iter().map(snapshot_info).collect())
    }

    /// Restores `path_in_snapshot` of snapshot `snapshot_id` into `dest_dir`: a directory's
    /// contents land in `dest_dir`, a file lands inside it under its own name.
    pub fn restore_path(
        &mut self,
        snapshot_id: &str,
        path_in_snapshot: &str,
        dest_dir: &Path,
    ) -> Result<(), StorageError> {
        let dest_dir = dest_dir
            .to_str()
            .ok_or_else(|| StorageError::NonUtf8Path(dest_dir.to_path_buf()))?;
        if self.stale {
            self.reindex()?;
        }
        let repo = &self.repo;
        let snap = repo
            .get_snapshot_from_str(snapshot_id, |_| true)
            .map_err(backend)?;
        let node = repo
            .node_from_snapshot_and_path(&snap, path_in_snapshot)
            .map_err(backend)?;
        let nodes = repo.ls(&node, &LsOptions::default()).map_err(backend)?;
        for entry in nodes.clone() {
            check_entry_path(&entry.map_err(backend)?.0)?;
        }
        let dest = if node.is_dir() {
            LocalDestination::new(dest_dir, true, false)
        } else {
            let name = node.name();
            check_entry_path(Path::new(&name))?;
            let file = Path::new(dest_dir).join(name);
            let file = file
                .to_str()
                .ok_or_else(|| StorageError::NonUtf8Path(file.clone()))?;
            LocalDestination::new(file, true, true)
        }
        .map_err(backend)?;
        let options = RestoreOptions::default();
        let plan = repo
            .prepare_restore(&options, nodes.clone(), &dest, false)
            .map_err(backend)?;
        repo.restore(plan, &options, nodes, &dest).map_err(backend)
    }
}

/// Waits, bounded, until the mtimes a directory listing reports match the entries' own. On
/// Windows the scan reads the listing's cached metadata, which can lag a just-written directory;
/// rustic hashes directory mtimes, so a lagging read makes an unchanged tree look changed.
fn wait_for_listing_to_settle(dir: &Path) {
    for _ in 0..LISTING_SETTLE_POLLS {
        if listing_is_current(dir) {
            return;
        }
        std::thread::sleep(LISTING_SETTLE_INTERVAL);
    }
}

/// Whether every entry under `dir` has the same mtime in its parent's listing as on its own.
/// Entries that cannot be read count as current: the backup reports them itself.
fn listing_is_current(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return true;
    };
    entries.flatten().all(|entry| {
        let path = entry.path();
        let (Ok(listed), Ok(actual)) = (entry.metadata(), fs::symlink_metadata(&path)) else {
            return true;
        };
        listed.modified().ok() == actual.modified().ok()
            && (!actual.is_dir() || listing_is_current(&path))
    })
}

/// Rejects an entry path that is not purely relative and descending, so a crafted `..` or
/// absolute node name cannot make a restore write outside its destination.
fn check_entry_path(path: &Path) -> Result<(), StorageError> {
    if path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        Ok(())
    } else {
        Err(StorageError::UnsafeEntryPath(path.to_path_buf()))
    }
}

#[cfg(test)]
mod tests;
