//! Repository storage over `rustic_core` (ADR-0003). The only module that names rustic types.

mod rclone;
mod rest_url;

use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::iter;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use rustic_backend::{BackendOptions, choose::SupportedBackend, util::location_to_type_and_path};
use rustic_core::{
    BackupOptions, ConfigOptions, Credentials, DataId, IndexedFullStatus, KeyOptions,
    LocalDestination, LsOptions, PathList, PruneOptions, Repository, RepositoryBackends,
    RepositoryOptions, RestoreOptions, RusticError, SnapshotOptions, TreeId,
    repofile::{IndexFile, Node, SnapshotFile as RusticSnapshot, SnapshotId},
};

use rclone::Rclone;
pub use rest_url::{
    REST_PASSWORD_KEY, join_rest_password, redact_rest_password, split_rest_password,
};

/// The Windows process creation flag that gives a console program no window.
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// zstd's own default level; rustic cannot unset a compression level once it is stored.
pub const ZSTD_DEFAULT_LEVEL: i32 = 3;

/// How long a backup waits for directory listings to catch up with the files (ADR-0003).
const LISTING_SETTLE_TIMEOUT: Duration = Duration::from_secs(1);
const LISTING_SETTLE_INTERVAL: Duration = Duration::from_millis(20);

/// Errors from [`Storage`] and [`probe`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StorageError {
    /// The compression level is not 0, 1..=22 or -7..=-1.
    #[error("compression level {0} is out of range")]
    InvalidCompression(i32),
    /// The password does not unlock the repository.
    #[error("wrong encryption password")]
    WrongPassword,
    /// No repository has been initialized at the location.
    #[error("no repository at this location")]
    Missing,
    /// The storage server refused the login (HTTP 401 or 403).
    #[error("the server refused the login")]
    LoginRefused,
    /// A path cannot be passed to the repository because it is not valid UTF-8.
    #[error("path is not valid UTF-8: {0}")]
    NonUtf8Path(PathBuf),
    /// The repository holds an entry whose path would leave the restore destination.
    #[error("unsafe path in snapshot: {0}")]
    UnsafeEntryPath(PathBuf),
    /// The program of an rclone location (`rclone` or the `rclone-command` one) was not found.
    #[error("the rclone program was not found; is it installed and on the PATH?")]
    RcloneMissing,
    /// rclone is too old, could not be run or exited before serving; its output is attached.
    #[error("rclone failed: {0}")]
    RcloneFailed(String),
    /// rclone did not print the address it serves on in time; what it printed is attached.
    #[error("rclone did not report its address within {0:?}{suffix}", suffix = output_suffix(.1))]
    RcloneTimeout(Duration, String),
    /// A file of a snapshot is larger than the caller allows to read into memory.
    #[error("{path} is larger than {limit} bytes")]
    TooLarge {
        /// The path in the snapshot.
        path: String,
        /// The most bytes the caller accepted.
        limit: u64,
    },
    /// Any other repository or backend failure, as rustic reports it.
    #[error("repository error: {0}")]
    Backend(String),
}

/// A buffer that refuses bytes past `limit`, so a dump of a crafted file stops there.
struct Capped {
    bytes: Vec<u8>,
    limit: u64,
    exceeded: bool,
}

impl io::Write for Capped {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let held = u64::try_from(self.bytes.len()).unwrap_or(u64::MAX);
        let more = u64::try_from(data.len()).unwrap_or(u64::MAX);
        if held.saturating_add(more) > self.limit {
            self.exceeded = true;
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// `: <output>` after an error message, or nothing when the program printed nothing.
fn output_suffix(output: &str) -> String {
    if output.is_empty() {
        String::new()
    } else {
        format!(": {output}")
    }
}

fn backend(err: impl AsRef<RusticError>) -> StorageError {
    StorageError::Backend(err.as_ref().display_log())
}

/// [`backend`] for a failure to reach the repository while opening it, where a 401 or 403 means
/// the login. Later operations keep [`backend`]: there a 403 can be a locked file.
fn open_failure(err: impl AsRef<RusticError>) -> StorageError {
    let err = err.as_ref();
    if login_refused(err) {
        return StorageError::LoginRefused;
    }
    StorageError::Backend(err.display_log())
}

/// Whether an OpenDAL error in the chain of `err` holds a 401 or 403 response.
fn login_refused(err: &RusticError) -> bool {
    iter::successors(Some(err as &(dyn Error + 'static)), |&err| err.source())
        .filter_map(|err| err.downcast_ref::<opendal::Error>())
        .any(|err| refused_status(&err.to_string()))
}

/// Whether an OpenDAL error's text holds a 401 or 403 response. OpenDAL keeps the response only
/// as text in its context, and WebDAV reports a 401 as `Unexpected`. The message (the server's
/// body) follows ` => ` and the source `, source: `, so neither is read.
fn refused_status(text: &str) -> bool {
    let head = text.split(" => ").next().unwrap_or_default();
    let head = head.split(", source: ").next().unwrap_or_default();
    [
        "response: Parts { status: 401,",
        "response: Parts { status: 403,",
    ]
    .iter()
    .any(|status| head.contains(status))
}

/// Where a repository lives: a rustic repository string (a plain path, `local:`, `rest:`,
/// `rclone:`, `opendal:<service>`) and the backend options for it. The password of a `rest:`
/// URL is the option [`REST_PASSWORD_KEY`], not part of the string.
#[derive(Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct RepoLocation {
    /// The repository string.
    pub repository: String,
    /// Backend options, such as the opendal service configuration.
    pub options: BTreeMap<String, String>,
}

/// Debug output lists only the option keys: the values may be secrets. A password in a `rest:`
/// URL is hidden too.
impl fmt::Debug for RepoLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RepoLocation")
            .field("repository", &redact_rest_password(&self.repository))
            .field("option_keys", &self.options.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl RepoLocation {
    /// The location `repository` with backend `options`.
    pub fn new(repository: String, options: BTreeMap<String, String>) -> Self {
        Self {
            repository,
            options,
        }
    }

    /// The directory holding the repository when it is on the local filesystem, by rustic's own
    /// parsing of the repository string.
    fn local_dir(&self) -> Option<PathBuf> {
        match location_to_type_and_path(&self.repository) {
            Ok((SupportedBackend::Local, dir)) => Some(PathBuf::from(dir.as_str())),
            _ => None,
        }
    }

    /// Whether "no config file" from this location proves there is no repository. The local
    /// filesystem is checked directly and OpenDAL reports only "not found" as absent. REST (and
    /// rclone, which serves REST) reads every non-2xx answer, a refused login included, as "no
    /// config", so there it proves nothing.
    pub fn proves_absence(&self) -> bool {
        matches!(
            location_to_type_and_path(&self.repository),
            Ok((SupportedBackend::Local | SupportedBackend::OpenDAL, _))
        )
    }

    /// The backends for rustic. An `rclone:` location is served by an rclone started here, which
    /// is returned too: rustic reaches it as a REST server and it stops when dropped.
    fn backends(&self) -> Result<(RepositoryBackends, Option<Rclone>), StorageError> {
        let mut options = self.options.clone();
        let password = options.remove(REST_PASSWORD_KEY);
        let (repository, rclone) = match (location_to_type_and_path(&self.repository), &password) {
            (Ok((SupportedBackend::Rclone, remote)), _) => {
                let (rclone, url) = Rclone::start(remote.as_str(), &options)?;
                (format!("rest:{url}"), Some(rclone))
            }
            (Ok((SupportedBackend::Rest, _)), Some(password)) => (
                rest_url::join_rest_password(&self.repository, password),
                None,
            ),
            _ => (self.repository.clone(), None),
        };
        let backends = BackendOptions::default()
            .repository(repository)
            .options(options)
            .to_backends()
            .map_err(|err| {
                // rustic quotes the URL, password included, when it cannot parse it.
                let text = AsRef::<RusticError>::as_ref(&err).display_log();
                StorageError::Backend(match &password {
                    Some(password) => rest_url::mask_password(&text, password),
                    None => text,
                })
            })?;
        Ok((backends, rclone))
    }
}

fn repository(backends: &RepositoryBackends) -> Result<Repository<()>, StorageError> {
    Repository::new(&RepositoryOptions::default(), backends).map_err(open_failure)
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
                open_failure(err)
            }
        })
}

/// What [`probe`] found at a [`RepoLocation`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RepoState {
    /// A repository exists and the password unlocks it.
    Ready,
    /// No repository has been initialized there.
    Missing,
    /// A repository exists but the password does not unlock it.
    WrongPassword,
    /// The storage server refused the login (HTTP 401 or 403).
    LoginRefused,
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
#[non_exhaustive]
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
    /// The bytes of the files the snapshot holds, before compression; `None` when it has no
    /// summary, as a snapshot of an older restic.
    pub size: Option<u64>,
}

impl SnapshotInfo {
    /// A snapshot's details from its fields.
    pub fn new(
        id: String,
        time: SystemTime,
        host: String,
        label: String,
        tags: Vec<String>,
    ) -> Self {
        Self {
            id,
            time,
            host,
            label,
            tags,
            size: None,
        }
    }

    /// The snapshot with its files' total `size` (see [`SnapshotInfo::size`]).
    pub fn with_size(self, size: Option<u64>) -> Self {
        Self { size, ..self }
    }
}

fn snapshot_info(snap: &RusticSnapshot) -> SnapshotInfo {
    SnapshotInfo {
        id: snap.id.to_hex().to_string(),
        time: snap.time.timestamp().into(),
        host: snap.hostname.clone(),
        label: snap.label.clone(),
        tags: snap.tags.iter().cloned().collect(),
        size: (snap.summary.as_ref()).map(|summary| summary.total_bytes_processed),
    }
}

/// What a [`SnapshotFile`] is in its snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A symlink or a special file.
    Other,
}

/// An entry of a snapshot, as [`Storage::snapshot_files`] and [`Storage::snapshot_entries`] list
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SnapshotFile {
    /// The path below the listed directory, `/`-separated.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// The mtime the snapshot stored, if any.
    pub modified: Option<SystemTime>,
    /// Whether it is a file, a directory or something else.
    pub kind: EntryKind,
}

/// `path` with `/` between its components; a component that is not UTF-8 is an error.
fn slash_path(path: &Path) -> Result<String, StorageError> {
    let parts = path
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| StorageError::NonUtf8Path(path.to_path_buf()))?;
    Ok(parts.join("/"))
}

/// Whether the backend lists a repository config at `location`; an unreadable local location is
/// an error, never "no". A "no" proves absence only where [`RepoLocation::proves_absence`].
fn has_config(location: &RepoLocation, repo: &Repository<()>) -> Result<bool, StorageError> {
    match repo.config_id() {
        // rustic's local backend reports "no config" for any `exists()` failure, such as an
        // unreadable directory, so ask the filesystem before calling the repository missing.
        Ok(None) => match location
            .local_dir()
            .map(|dir| fs::exists(dir.join("config")))
        {
            Some(Ok(false)) | None => Ok(false),
            Some(Ok(true)) => Ok(true),
            Some(Err(err)) => Err(StorageError::Backend(err.to_string())),
        },
        Ok(Some(_)) => Ok(true),
        Err(err) => Err(open_failure(err)),
    }
}

/// Inspects `location` with `password` (empty means no password). Never treats an unreadable
/// location as [`RepoState::Missing`].
pub fn probe(location: &RepoLocation, password: &str) -> RepoState {
    let (backends, _rclone) = match location.backends() {
        Ok(backends) => backends,
        Err(err) => return probe_failed(err),
    };
    let repo = match repository(&backends) {
        Ok(repo) => repo,
        Err(err) => return probe_failed(err),
    };
    match has_config(location, &repo) {
        Ok(false) => return RepoState::Missing,
        Ok(true) => {}
        Err(err) => return probe_failed(err),
    }
    match repo.open(&Credentials::password(password)) {
        Ok(_) => RepoState::Ready,
        Err(err) if err.is_incorrect_password() => RepoState::WrongPassword,
        Err(err) => probe_failed(open_failure(err)),
    }
}

/// What [`probe`] reports for a failure: a refused login keeps its own state.
fn probe_failed(err: StorageError) -> RepoState {
    match err {
        StorageError::LoginRefused => RepoState::LoginRefused,
        err => RepoState::Error(err.to_string()),
    }
}

/// An open repository with its index loaded. [`Storage::reindex`] refreshes the index.
pub struct Storage {
    backends: RepositoryBackends,
    password: String,
    repo: Repository<IndexedFullStatus>,
    /// A snapshot was written or a prune ran since the index was loaded, so restoring needs a
    /// reindex.
    stale: bool,
    /// The host new snapshots are stored with; `None`, outside tests, is rustic's default.
    host: Option<String>,
    /// The rclone serving an `rclone:` location. Last, so it stops after the repository is gone.
    _rclone: Option<Rclone>,
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
        let (backends, rclone) = location.backends()?;
        let repo = repository(&backends)?
            .init(
                &Credentials::password(password),
                &KeyOptions::default(),
                &config,
            )
            .and_then(Repository::to_indexed)
            .map_err(backend)?;
        Ok(Self::new(backends, password, repo, rclone))
    }

    /// Opens the existing repository at `location`. Fails with [`StorageError::Missing`] if there
    /// is none, and with [`StorageError::WrongPassword`] if `password` does not unlock it. Where
    /// a missing config proves nothing ([`RepoLocation::proves_absence`]) the failure is
    /// [`StorageError::Backend`] instead, so a refused login is not taken for a missing repository.
    /// A login the server refuses with HTTP 401 or 403 is [`StorageError::LoginRefused`].
    pub fn open(location: &RepoLocation, password: &str) -> Result<Self, StorageError> {
        let (backends, rclone) = location.backends()?;
        if !has_config(location, &repository(&backends)?)? {
            return Err(if location.proves_absence() {
                StorageError::Missing
            } else {
                StorageError::Backend(
                    "no repository config was found; the server may have refused the request"
                        .to_string(),
                )
            });
        }
        let repo = open_indexed(&backends, password)?;
        Ok(Self::new(backends, password, repo, rclone))
    }

    fn new(
        backends: RepositoryBackends,
        password: &str,
        repo: Repository<IndexedFullStatus>,
        rclone: Option<Rclone>,
    ) -> Self {
        Self {
            backends,
            password: password.to_string(),
            repo,
            stale: false,
            host: None,
            _rclone: rclone,
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
            .host(self.host.clone())
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

    /// Deletes the snapshots `ids` (full hex ids). Their data stays in the repository until a
    /// `rustic prune`. May fail if one of them is already gone, depending on the backend.
    pub fn forget(&self, ids: &[&str]) -> Result<(), StorageError> {
        let ids = ids
            .iter()
            .map(|id| id.parse::<SnapshotId>().map_err(backend))
            .collect::<Result<Vec<_>, _>>()?;
        self.repo.delete_snapshots(&ids).map_err(backend)
    }

    /// Removes data no snapshot needs, with rustic's default options: an unused pack is only
    /// marked, and a later prune deletes it once `keep_delete` (23 h) has passed (ADR-0015).
    /// Reindexes after a prune that succeeded, so marked packs no longer count as live.
    pub fn prune(&mut self) -> Result<(), StorageError> {
        let options = PruneOptions::default();
        let plan = self.repo.prune_plan(&options).map_err(backend)?;
        let pruned = self.repo.prune(&options, plan);
        // Prune rewrites the index files, even when it fails partway.
        self.stale = true;
        pruned.map_err(backend)?;
        self.reindex()
    }

    /// Whether any index file of the repository, as stored now, lists packs a prune marked for
    /// deletion. Reads every index file, but no tree or pack.
    pub fn has_marked_packs(&self) -> Result<bool, StorageError> {
        for index in self.repo.stream_files::<IndexFile>().map_err(backend)? {
            if !index.map_err(backend)?.1.packs_to_delete.is_empty() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether the repository is append-only, so [`Storage::forget`] and [`Storage::prune`] fail.
    pub fn append_only(&self) -> bool {
        self.repo.config().append_only == Some(true)
    }

    /// Makes the repository append-only, so [`Storage::forget`] fails.
    #[cfg(test)]
    pub(crate) fn set_append_only(&mut self) {
        self.repo
            .apply_config(&ConfigOptions::default().set_append_only(true))
            .unwrap();
    }

    /// The host name new snapshots are stored with, as rustic determines it.
    pub fn hostname(&self) -> Result<String, StorageError> {
        let snapshot = SnapshotOptions::default()
            .host(self.host.clone())
            .to_snapshot()
            .map_err(backend)?;
        Ok(snapshot.hostname)
    }

    /// The snapshot `snapshot_id`, read after a reindex when the open index lacks its tree:
    /// another session, such as another machine, added it after this one loaded the index.
    fn snapshot(&mut self, snapshot_id: &str) -> Result<RusticSnapshot, StorageError> {
        let reindexed = self.stale;
        if reindexed {
            self.reindex()?;
        }
        let snap = (self.repo)
            .get_snapshot_from_str(snapshot_id, |_| true)
            .map_err(backend)?;
        // An index loaded just now already holds every tree there is.
        if !reindexed && self.repo.get_index_entry(&snap.tree).is_err() {
            self.reindex()?;
        }
        Ok(snap)
    }

    /// The bytes the snapshots `snapshot_ids` take in the repository: the packed size of each
    /// distinct tree and data blob they reference, counted once however many of them share it
    /// (restic's `stats --mode raw-data`). Reads trees, through rustic's cache, but no data.
    ///
    /// # Errors
    /// An unknown snapshot id, or a blob missing from the index even after a reindex.
    pub fn space_used(&mut self, snapshot_ids: &[&str]) -> Result<u64, StorageError> {
        let mut pending: Vec<TreeId> = Vec::new();
        for id in snapshot_ids {
            pending.push(self.snapshot(id)?.tree);
        }
        let repo = &self.repo;
        let (mut trees, mut data) = (HashSet::new(), HashSet::<DataId>::new());
        let mut total = 0;
        while let Some(tree) = pending.pop() {
            if !trees.insert(tree) {
                continue;
            }
            let entry = repo.get_index_entry(&tree).map_err(backend)?;
            total += u64::from(entry.location.length);
            for node in repo.get_tree(&tree).map_err(backend)?.nodes {
                pending.extend(node.subtree);
                for blob in node.content.into_iter().flatten() {
                    if data.insert(blob) {
                        let entry = repo.get_index_entry(&blob).map_err(backend)?;
                        total += u64::from(entry.location.length);
                    }
                }
            }
        }
        Ok(total)
    }

    /// Every node below the directory `path_in_snapshot` of snapshot `snapshot_id`, with its
    /// path relative to it.
    fn snapshot_nodes(
        &mut self,
        snapshot_id: &str,
        path_in_snapshot: &str,
    ) -> Result<Vec<(PathBuf, Node)>, StorageError> {
        let snap = self.snapshot(snapshot_id)?;
        let repo = &self.repo;
        let node = repo
            .node_from_snapshot_and_path(&snap, path_in_snapshot)
            .map_err(backend)?;
        let nodes = repo.ls(&node, &LsOptions::default()).map_err(backend)?;
        nodes.map(|entry| entry.map_err(backend)).collect()
    }

    /// Every regular file below the directory `path_in_snapshot` of snapshot `snapshot_id`,
    /// with its path relative to it.
    pub fn snapshot_files(
        &mut self,
        snapshot_id: &str,
        path_in_snapshot: &str,
    ) -> Result<Vec<SnapshotFile>, StorageError> {
        let mut files = Vec::new();
        for (path, node) in self.snapshot_nodes(snapshot_id, path_in_snapshot)? {
            if node.is_file() {
                files.push(SnapshotFile {
                    path: slash_path(&path)?,
                    size: node.meta.size,
                    modified: node.meta.mtime.map(SystemTime::from),
                    kind: EntryKind::File,
                });
            }
        }
        Ok(files)
    }

    /// Every entry below the directory `path_in_snapshot` of snapshot `snapshot_id`, with its
    /// kind and its path relative to it. A name that is not UTF-8 is converted lossily, as a
    /// restored dir's names are read.
    ///
    /// # Errors
    /// [`StorageError::UnsafeEntryPath`] for a path [`Storage::restore_path`] refuses, and the
    /// repository error otherwise.
    pub fn snapshot_entries(
        &mut self,
        snapshot_id: &str,
        path_in_snapshot: &str,
    ) -> Result<Vec<SnapshotFile>, StorageError> {
        let nodes = self.snapshot_nodes(snapshot_id, path_in_snapshot)?;
        (nodes.into_iter())
            .map(|(path, node)| {
                check_entry_path(&path)?;
                let kind = if node.is_file() {
                    EntryKind::File
                } else if node.is_dir() {
                    EntryKind::Dir
                } else {
                    EntryKind::Other
                };
                let parts: Vec<_> = (path.components())
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect();
                Ok(SnapshotFile {
                    path: parts.join("/"),
                    size: node.meta.size,
                    modified: node.meta.mtime.map(SystemTime::from),
                    kind,
                })
            })
            .collect()
    }

    /// The contents of the file `path_in_snapshot` in snapshot `snapshot_id`, `None` when the
    /// snapshot has nothing at that path. The bytes are counted as they are read, since a
    /// crafted snapshot can declare any size.
    ///
    /// # Errors
    /// [`StorageError::TooLarge`] when the file holds more than `limit` bytes.
    pub fn read_file_if_present(
        &mut self,
        snapshot_id: &str,
        path_in_snapshot: &str,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        let snap = self.snapshot(snapshot_id)?;
        let repo = &self.repo;
        let found = repo
            .find_nodes_from_path([snap.tree], Path::new(path_in_snapshot))
            .map_err(backend)?;
        let Some(&Some(at)) = found.matches.first() else {
            return Ok(None);
        };
        let mut capped = Capped {
            bytes: Vec::new(),
            limit,
            exceeded: false,
        };
        let dumped = repo.dump(&found.nodes[at], &mut capped);
        if capped.exceeded {
            return Err(StorageError::TooLarge {
                path: path_in_snapshot.to_owned(),
                limit,
            });
        }
        dumped.map_err(backend)?;
        Ok(Some(capped.bytes))
    }

    /// Stores new snapshots as made by `host`, as another machine would.
    #[cfg(test)]
    pub(crate) fn set_host(&mut self, host: &str) {
        self.host = Some(host.to_owned());
    }

    /// The contents of the file `path_in_snapshot` in snapshot `snapshot_id`.
    pub fn read_file(
        &mut self,
        snapshot_id: &str,
        path_in_snapshot: &str,
    ) -> Result<Vec<u8>, StorageError> {
        let snap = self.snapshot(snapshot_id)?;
        let repo = &self.repo;
        let node = repo
            .node_from_snapshot_and_path(&snap, path_in_snapshot)
            .map_err(backend)?;
        let mut bytes = Vec::new();
        repo.dump(&node, &mut bytes).map_err(backend)?;
        Ok(bytes)
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
        let snap = self.snapshot(snapshot_id)?;
        let repo = &self.repo;
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

/// Waits, up to a timeout, until the mtime and size a directory listing reports match the
/// entries' own. On
/// Windows the scan reads the listing's cached metadata, which can lag a just-written directory;
/// rustic hashes directory mtimes, so a lagging read makes an unchanged tree look changed.
fn wait_for_listing_to_settle(dir: &Path) {
    let start = Instant::now();
    while !listing_is_current(dir) && start.elapsed() < LISTING_SETTLE_TIMEOUT {
        std::thread::sleep(LISTING_SETTLE_INTERVAL);
    }
}

/// Whether every entry under `dir` has the same mtime and (for files) size in its parent's
/// listing as on its own. atime is not compared: rustic stores mtime in its place.
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
            && (actual.is_dir() || listed.len() == actual.len())
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
