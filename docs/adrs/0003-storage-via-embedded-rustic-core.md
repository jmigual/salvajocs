# ADR-0003: Storage through embedded rustic_core, pinned exactly; any rustic_backend repository

**Status:** Accepted
**Date:** 2026-11-16

## Context

The user wants a restic-format repository, a compression level and an optional password chosen
at first run, with storage embedded rather than an external binary. They also want to choose
where the repository lives: a folder or any backend rustic supports.

restic is Go and has no embedding API (research.md §3). rustic_core reads and writes the restic
repo format (research.md §4). The facts below were checked against the crate sources
(rustic_core 0.13.0, rustic_backend 0.7.0, both 2026-08-16, the latest releases) and a
throwaway program built against them in the scratchpad:

- **API:** the API is synchronous, so we need no async code.
  - `Repository::new(&RepositoryOptions, &backends)`, then `.init(&Credentials, &KeyOptions,
    &ConfigOptions)` or `.open(&Credentials)`.
  - `.to_indexed_ids()` before `backup`; `.to_indexed()` before `ls`/`restore`.
  - `backup(&BackupOptions, &PathList, SnapshotFile)` takes `BackupOptions.as_path` (single
    source).
  - `get_matching_snapshots(filter)`.
  - `node_from_snapshot_and_path` → `ls` → `prepare_restore` → `restore` into a
    `LocalDestination`.
  All of these ran in the spike.
- **Compression:** a zstd level `i32` stored in the repo config. `ConfigOptions::set_compression`
  is used at init, and `Repository::apply_config` changes it later; both ran in the spike.
  - `Some(0)` = none, unset = zstd default; allowed levels are 1..=22 and -1..=-7.
  - It is a repository setting, not per run, so every machine writing to the repo uses it.
- **Optional password:** rustic_core has no separate no-password mode, but it does not reject
  `Credentials::password("")`. Key lookup and key creation just run scrypt on the given bytes.
  In the spike, init and open with `""` worked, and a wrong password gave
  `RusticError::is_incorrect_password() == true`. restic's `--insecure-no-password` is
  documented in restic 0.19.1 source (`internal/global/global.go`) as "use an empty password for
  the repository", and its password reader returns `""`. So both tools derive the key from the
  empty string.
- **Missing repo:** `Repository::config_id()` returns `Ok(None)` before init (spike).
- **Skip if unchanged:** `BackupOptions.parent_opts.skip_if_unchanged` works, but only when the
  index was loaded after the parent snapshot was written. In the spike, a second backup on the
  same in-memory index saved a duplicate; after reopening and reindexing it was skipped. The
  in-memory index is not updated by `backup`.
  - On Windows the scan reads the directory listing's cached metadata, which can lag a
    just-written subdirectory's mtime (Windows CI: 5 of 20 rounds). rustic hashes directory
    mtimes, so the unchanged second backup wrote a snapshot. `backup_dir` therefore waits, up to
    1 s, until each entry's listed mtime (and a file's size) equals its own, before it scans.
    If the timeout passes, it backs up anyway: at worst one extra snapshot, no data loss.
- **Backends (rustic_backend):** a repository string plus a `BTreeMap<String, String>` of
  options (`BackendOptions { repository, options, .. }.to_backends()`).
  - Plain path or `local:` → local filesystem.
  - `rest:<url>` → restic REST server (reqwest, rustls).
  - `rclone:<remote>` → needs the external `rclone` binary on `PATH`.
  - `opendal:<service>`: the service config goes in the options map. Services built in: b2, ftp,
    swift, azblob, azdls, cos, fs, dropbox, gdrive, gcs, ghac, http, ipmfs, memory, obs,
    onedrive, oss, pcloud, s3, webdav, webhdfs, azfile, yandex-disk. **`sftp` only when not on
    Windows** (`[target."cfg(not(windows))".dependencies.opendal]` in rustic_backend's
    `Cargo.toml`).
- **Dependencies:**
  - rustic_core plus rustic_backend with default features off (local only): about 226 crates
    in the spike, with no tokio runtime (only `tokio/time` via `backon`) and C code via
    `zstd-sys`.
  - The default-feature tree (opendal, rest, rclone) is larger and was not measured.
  - The opendal backends add a tokio multi-thread runtime that rustic_backend owns internally.
  - The rest backend adds reqwest (rustls).
  - MSRV of both crates: 1.91.
- rustic_core changes its API almost every release (research.md §4; 0.9 → 0.13 within a year).

## Decision

- Depend on `rustic_core = "=0.13.0"` and `rustic_backend = "=0.7.0"`, pinned exactly in
  `[workspace.dependencies]`. Upgrades are their own commit with the storage tests green.
- All rustic calls live in `core::storage`, the only module that names rustic types. Everything
  else uses our own types (`RepoState`, `SnapshotInfo`, `Compression`). The one exception is
  test-only: `crates/salvajocs-gui/tests/appearance.rs` writes fixture snapshots with rustic to
  set their host and time.
- **Repository location:** a local folder (default), or any backend rustic_backend exposes,
  addressed by repository string plus options map (ADR-0009 stores them). rustic_backend's
  default features (`opendal`, `rest`, `rclone`) stay on.
  - The cross-OS caveat is shown in the backend picker: `opendal:sftp` is unavailable in Windows
    builds, so a repo that must be read from both OSes should not use it.
  - `rclone:` requires `rclone` on `PATH`.
- **Compression:** the first-run dialog's level goes to `ConfigOptions::set_compression` at init.
  Settings shows `repo.config().compression` and changes it with `apply_config`. The app config
  does not store compression.
- **Password:** `Credentials::password(p)`, with `""` meaning no password, shown with a warning.
  This is the same key restic derives with `--insecure-no-password`.
- **Probe:**
  - `config_id() == None` → `Missing`, except for a local repository, where rustic reports
    `None` for any failed `exists()` (such as an unreadable directory). There the probe confirms
    with `fs::exists(<dir>/config)`; an `Err` there is `Error(message)`;
  - `open` failing with `is_incorrect_password()` → `WrongPassword`;
  - any other error → `Error(message)`, never treated as `Missing`.
- **Index freshness:** the repository is reindexed once at the start of each backup run (a
  `backup_games` call, or a pre-restore safety backup). Within one run each game is backed up
  once, and its parent comes from an earlier run, which is already in that index. Reindexing
  per game would cost N index loads on remote backends for no gain.
- **Indexed state:** `core::storage` holds one `Repository<IndexedFullStatus>` (`to_indexed`),
  not the ids-only index used for backup in the spike. `Repository<OpenStatus>` is not `Clone`,
  so one value cannot serve both states, and restore needs the full index. A snapshot written
  in a session is not in that in-memory index, so `restore_path` reindexes first when the
  session has written one. `reindex` reopens the repository (the key is derived again) rather
  than using `drop_index().to_indexed()`: both consume the repository, and a failed `to_indexed`
  would leave `Storage` without one.

## Alternatives considered

- **restic CLI from `PATH`:** no API churn, and users can inspect with plain restic. But it adds
  an external binary that must be present and of the right version, and needs JSON
  output parsing and env-var plumbing. The user explicitly prefers the library.
- **restic CLI bundled in our binary** (`include_bytes!`, extracted at runtime): self-contained,
  but it is still a child process with a fetch script, checksum checks and a third-party
  license to ship. rustic_core removes all of that.
- **rustic_backend local only:** a smaller dependency tree, but the user wants remote backends.

## Consequences

- For `rest:` and `rclone:`, any non-2xx answer to the HEAD on `config` (401, 403, 5xx) reaches
  us as "no config", so `probe` reports `Missing`. The confirm-before-init dialog showing the
  location is the guard against initializing over a repository that merely failed to answer.
- One self-contained binary; no external restic.
- Every rustic_core upgrade is a code change. The exact pin keeps `cargo update` from breaking
  the build.
- The compression level is per repository: all machines share it.
- **restic interop is run-tested** (restic 0.19.1, 2026-11-16):
  - **restic reads rustic's repo.** The rustic spike made an empty-password repo (compression
    19, then 3) with a snapshot at `/salvajocs/test-slug` tagged `salvajocs,game:test-slug`.
    `restic --insecure-no-password snapshots`, `ls latest` and `restore latest` all worked,
    and the restored files are identical (`diff -r`).
  - **rustic reads restic's writes.** restic then backed up into the same repo
    (`--compression max`), and `restic check --read-data` found no errors. rustic reopened the
    repo with the empty password and restored that restic snapshot identically.
  - **restic ignores the stored compression level.** `restic cat config` shows only `version`,
    `id` and `chunker_polynomial`, so restic uses its own per-run `--compression`.
  - A restic backup of a relative dir is stored under that relative name. This matters only if
    a user writes into the repo with restic.
- Whether `as_path = /salvajocs/<slug>` is stored identically by Windows builds is to verify in
  plan step 7 (Windows CI) with `snapshot_path_is_the_same_on_every_os`.
