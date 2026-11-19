# ADR-0009: Config in the OS config dir; password and secret options in the OS keyring or asked per session

**Status:** Accepted
**Date:** 2026-11-16
**Amended:** 2026-11-17: named backend fields and rclone remotes replace the bare option
list; compression is a dropdown. 2026-11-19: a login the server refuses has its own probe
result, `LoginRefused`.

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
  - `custom_games`;
  - `ignored_prefixes`: launcher prefixes left out of scans and restores (ADR-0017).
  Compression is **not** stored here: it lives in the repository config (ADR-0003).
- **Generic backend UI:**
  - A picker with "Local folder" (default, with a folder picker), "REST server", "rclone remote"
    and one entry per opendal service that rustic_backend builds for this OS, minus the four
    below, by a friendly name ("WebDAV", "Amazon S3 (or compatible)"). The stored repository
    string stays `opendal:<service>`. The services are sorted by that name; the three kinds
    above them stay first.
  - Hidden services, because each is a trap in a backup picker (`config::tests` compares the
    list with opendal's registry):
    - `memory`: data vanishes when the process exits;
    - `http`: read-only;
    - `ghac`: works only inside GitHub Actions;
    - `fs`: duplicates "Local folder".
  - Local, REST and rclone show a location. rclone also lists its remotes
    (`rclone listremotes`) in a dropdown that fills the start of the location.
  - An opendal service shows named fields from `config::opendal_fields`: label, placeholder,
    hint, required and secret per key. Required fields gate Continue/Apply. A test checks every
    key against opendal's config structs, which ignore unknown keys.
  - "Advanced options" holds a key/value list for any other option, with a link to the service's
    opendal config page. Any option can be ticked "secret". Its value then goes to the keyring
    and the config keeps only the key.
  - Named fields use the same `repository_options` list as Advanced rows, so the config format
    did not change. On load an option goes to a field only when its key and secret flag both
    match, so the flag and the keyring scope never change. A key set twice blocks the form.
  - The picker states that `opendal:sftp` is missing on Windows builds, and that `rclone` needs
    the `rclone` binary.
- **First run:** when the config file is missing, the GUI opens a modal with the backend picker,
  a compression dropdown, an encryption password field and a "remember"
  checkbox. Confirming probes the repo (ADR-0003). The dropdown offers Off, Fast (1), Default
  (3), Smaller (9), Smallest (19) and Custom level, a number field where `0` = off and
  `1..=22` or `-7..=-1` are levels. A stored level without a preset selects Custom level. Core
  validates the range before init.
  - `Ready` → use it; the compression control is ignored and the repo's own level is shown.
  - `Missing` → ask "Create a new repository at <location>?" and init (with the chosen
    compression) only when the user confirms. A typo must not silently create an empty repo.
  - `WrongPassword` → inline error.
  - `LoginRefused` (an OpenDAL service answered HTTP 401 or 403) → inline "The server refused
    the login", also at startup. REST and rclone cannot tell it apart (ADR-0003).
  - `Error` (network, auth, …) → shown as is, never treated as missing.
- **Secrets:** "remember" stores the password in the OS credential store under service
  `salvajocs`. The account is the scope plus, for a secret option, `#<key>`. The scope
  (`AppConfig::secret_scope`) is JSON of the repository string and its sorted non-secret
  options, so two buckets on one service (`opendal:s3`) keep separate passwords and a `#` in a
  path cannot collide. Editing a non-secret option changes the scope, so the old entries are
  orphaned; Settings offers to delete them.
  Otherwise they are asked once per session and held in memory. If the store fails, the app
  falls back to asking. The Windows store rejects values over 2560 bytes; `SecretError::TooLong`
  marks that case and the caller treats the secret as "ask every session". At startup,
  `config::missing_secrets` lists the password and every secret option not found in the keyring. The GUI prompts for exactly those before opening the
  repository.
- **Keyring crates (verified in step 8):** `keyring-core` 1.x plus one store crate per OS, each
  behind a target `cfg`. The `keyring` crate is not used: its `v1` feature pulls the zbus
  Secret Service store, and its `cli` feature pulls every store.
  - Windows: `windows-native-keyring-store` (Credential Manager), 15 packages in total.
  - Linux: `dbus-secret-service-keyring-store` with features `vendored` (builds libdbus, no system
    `libdbus-1-dev`) and `crypto-rust` (the store does not compile without a crypto feature),
    49 packages in total. The zbus Secret Service store needs more than 100 packages and an async
    runtime feature.
  - `linux-keyutils-keyring-store` is smaller (8 packages) but the kernel keyring is in memory and
    is cleared by a reboot, so it cannot hold a remembered password.
  - The store is installed on first use in `core::secret`. A missing store or Secret Service
    daemon surfaces as `SecretError` and the app falls back to asking.
  - `keyring_set_get_delete` passed on Linux against a Secret Service daemon (KDE).
- An empty password means no password (ADR-0003), shown with a warning.

## Alternatives considered

- **Password file next to the config:** simplest, but the password sits in plain text on disk.
- **Ask every launch only:** safe but annoying. Kept as the fallback and as the user's choice.
- **One hand-written form per backend service:** about 25 services. A data table of fields per
  opendal service gives the same form without the code, and a test keeps it honest.
- **Only the generic key/value list** (the first version): users could not tell which keys a
  service needs.
- **TOML config:** an extra dependency when YAML is already needed for the manifest.

## Consequences

- On Linux without a Secret Service daemon the app asks per session.
- Changing the repository in Settings re-runs the same probe as first run.
- An opendal upgrade can rename keys; `named_field_keys_are_opendal_config_keys` fails then.
- Options without a field are set under Advanced, where the user needs opendal's key names; the
  form links to the service's option page.
