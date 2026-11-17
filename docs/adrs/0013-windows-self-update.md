# ADR-0013: Windows self-update: check GitHub at start, one-click silent reinstall

**Status:** Accepted
**Date:** 2026-11-17

## Context

On Windows salvajocs ships as a per-user Inno Setup installer (`{localappdata}\Programs\salvajocs`)
and as a zip. Linux users update through their package manager, the AppImage or
`cargo install`. A running Windows exe cannot overwrite itself, but the installer can replace it
after the app closes.

## Decision

- **Check:** on Windows builds only (`Env::check_update` is `None` elsewhere), once per start and in
  a background job, the app asks `api.github.com/repos/jmigual/salvajocs/releases/latest` (User-Agent
  `salvajocs/<version>`) and compares the tag `v<version>` with its own version (`semver`; a
  pre-release or build metadata never counts as newer). Any failure, offline or GitHub's rate
  limit, is only logged to stderr. Settings has "Check for
  updates at start" (`check_updates` in `config.yaml`, default on, missing in older files = on).
- **Notice:** a newer release shows "Update available: vX.Y.Z" in the top bar. When `unins000.exe`
  sits next to the running exe the installer put it there, and the notice offers **Update now**.
  Otherwise (zip, `cargo install`) it links the release page.
- **Update now:** a job empties `<cache dir>/salvajocs/update`, downloads
  `salvajocs-<version>-x86_64-setup.exe` and `SHA256SUMS` over HTTPS only, from
  `github.com/jmigual/salvajocs/releases/download/v<version>/` only and at most 100 MB each,
  refuses the installer when its SHA-256 is missing from the list or differs, and writes it there.
- The UI thread starts it with `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /RELAUNCH` and closes the
  window only on a frame where no background job runs (`App::busy`: backup, restore, repository
  or compression change, config save, keyring write, startup), so no job is cut off mid-write.
  The installer closes any remaining instance and starts the new version.
- All logic except the notice lives in `core::update`, testable on every OS; `sha2` and `semver`
  were already in the lock file through rustic.

## Alternatives considered

- **`self_update` / `self-replace` crates:** replace the exe in place, but skip the installer's
  shortcuts and uninstall entry, and add dependencies.
- **Notify only, no one-click:** simpler, but every update would be a manual download.
- **Background download without asking:** surprises users on metered links.

## Consequences

- The hash comes from the same release as the installer. It protects against a corrupted or
  truncated download, not against a compromised GitHub account or release.
- The installer is not code-signed, so SmartScreen may warn when it runs. Code signing is out of
  scope.
- Every Windows start makes one unauthenticated GitHub API request (60 per hour per IP).
