# ADR-0009: Config in the OS config dir; password and secret options in the OS keyring or asked per session

**Status:** Accepted
**Date:** 2026-11-16

## Context

At first launch the app must ask for:

- the repository location: a folder or any rustic_backend backend (ADR-0003), where backups go
  and are restored from;
- the compression level;
- an optional password.

The config must also hold roots (ADR-0005) and custom games (ADR-0008). Passwords and backend
secrets (access keys, tokens) must not sit in the config in plain text.

## Decision

- **Location:** `dirs::config_dir()/salvajocs/config.yaml` (`~/.config` on Linux, `%APPDATA%` on
  Windows). Cache and staging go under `dirs::cache_dir()/salvajocs/`. Written with serde-saphyr,
  through a temp file and a rename.
- **Contents:**
  - `repository`: the rustic_backend repository string (a folder path, `rest:<url>`,
    `rclone:<remote>`, `opendal:<service>`);
  - `repository_options`: a list of `{ key, value | secret: true }`;
  - `password: none | keyring | ask`;
  - `roots`;
  - `custom_games`.
  Compression is **not** stored here: it lives in the repository config (ADR-0003).
- **Generic backend UI:**
  - A picker with "Local folder" (default, with a folder picker), "REST server", "rclone" and
    "OpenDAL service" (a dropdown of the services rustic_backend builds for this OS).
  - Then the location string and a key/value list for options. No per-service forms.
  - Any option can be ticked "secret". Its value then goes to the keyring and the config keeps
    only the key.
  - The picker states that `opendal:sftp` is missing on Windows builds, and that `rclone` needs
    the `rclone` binary.
- **First run:** when the config file is missing, the GUI opens a modal with the backend picker,
  a compression field, a password field and a "remember"
  checkbox. Confirming probes the repo (ADR-0003). The compression field is one integer, where
  empty = zstd default, `0` = off, and `1..=22` or `-7..=-1` are levels. Core validates the
  range before init.
  - `Ready` → use it; the compression control is ignored and the repo's own level is shown.
  - `Missing` → ask "Create a new repository at <location>?" and init (with the chosen
    compression) only when the user confirms. A typo must not silently create an empty repo.
  - `WrongPassword` → inline error.
  - `Error` (network, auth, …) → shown as is, never treated as missing.
- **Secrets:** "remember" stores the password in the OS credential store under service
  `salvajocs`, account = repository string. Secret options use account `<repository>#<key>`.
  Otherwise they are asked once per session and held in memory. If the store fails, the app
  falls back to asking. At startup, `config::missing_secrets` lists the password and every
  secret option not found in the keyring. The GUI prompts for exactly those before opening the
  repository.
- **Keyring crate:** its layout changed. `keyring` 4.x exposes stores under a `cli` module, and
  `keyring-core` 1.0 exists separately. Which crate(s) a library should use is to verify in plan
  step 8. On Linux prefer Secret Service: the kernel keyutils default may not survive a reboot,
  also to verify in step 8.
- An empty password means no password (ADR-0003), shown with a warning.

## Alternatives considered

- **Password file next to the config:** simplest, but the password sits in plain text on disk.
- **Ask every launch only:** safe but annoying. Kept as the fallback and as the user's choice.
- **One form per backend service:** nicer for s3/b2, but about 25 services. The generic
  key/value list matches rustic_backend's own options map.
- **TOML config:** an extra dependency when YAML is already needed for the manifest.

## Consequences

- On Linux without a Secret Service daemon the app asks per session.
- Changing the repository in Settings re-runs the same probe as first run.
- Users of remote backends must know the option keys their service needs (opendal's names).
  The picker links to rustic's backend docs.
