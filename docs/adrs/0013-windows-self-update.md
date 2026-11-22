# ADR-0013: Windows self-update: check GitHub at start, one-click silent reinstall

**Status:** Accepted
**Date:** 2026-11-17
**Amended:** 2026-11-19: the AppImage checks for updates and replaces itself too (ADR-0022).
**Amended:** 2026-11-22: `SHA256SUMS` must carry a minisign signature of the release key.

## Context

On Windows salvajocs ships as a per-user Inno Setup installer (`{localappdata}\Programs\salvajocs`)
and as a zip. Linux users update through their package manager or `cargo install`; the AppImage
updates itself (ADR-0022). A running Windows exe cannot overwrite itself, but the installer can
replace it after the app closes.

## Decision

- **Check:** on Windows builds and in an AppImage (ADR-0022; `Env::check_update` is `None`
  elsewhere), once per start and in a background job, the app asks
  `api.github.com/repos/jmigual/salvajocs/releases/latest` (User-Agent `salvajocs/<version>`) and
  compares the tag `v<version>` with its own version (`semver`; a pre-release or build metadata
  never counts as newer). Any failure, offline or GitHub's rate limit, is only logged to stderr.
  Settings has "Check for updates at start" (`check_updates` in `config.yaml`, default on, missing
  in older files = on).
- **Notice:** a newer release shows "Update available: vX.Y.Z" in the bottom bar. When
  `unins000.exe` sits next to the running exe the installer put it there, and the notice offers
  **Update now**. Otherwise (zip, `cargo install`) it links the release page.
- **Update now:** a job empties `<cache dir>/salvajocs/update`, downloads
  `salvajocs-<version>-x86_64-setup.exe`, `SHA256SUMS` and `SHA256SUMS.minisig` over HTTPS only,
  from `github.com/jmigual/salvajocs/releases/download/v<version>/` only and at most 100 MB each,
  refuses the update when `SHA256SUMS.minisig` is missing (`UpdateError::Unsigned`) or is not the
  release key's signature of `SHA256SUMS` (`UpdateError::BadSignature`), before trusting any hash,
  then refuses the installer when its SHA-256 is missing from the list or differs, and writes it
  there. The failure shows as "The update failed" in the notices.
- **Release key:** the release workflow signs `SHA256SUMS` with a minisign key (secret
  `MINISIGN_SECRET_KEY`); the public key is embedded in the binary
  (`crates/salvajocs-core/src/update/salvajocs.pub`, verified with `minisign-verify`). Setup and
  rotation: `docs/guide/releasing.md`.
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

- The hash comes from the same release as the installer, so on its own it protects against a
  corrupted or truncated download only. The signature adds authenticity against assets swapped
  after the workflow or outside it, such as with a stolen token that edits a release. It does not
  stop a token that can push a `v*` tag, as the workflow then signs what that tag builds; the
  required reviewer of the `release` environment guards builds started by a tag. The signed list
  names each file with its version, so another release's list cannot vouch for this release's
  installer.
- A build with this check refuses a release without `SHA256SUMS.minisig`. Updates only go
  forward and every release from this change on is signed, so it meets one only if an older
  release is the latest, for a build that is not released yet: "Update now" then fails with a
  notice. Builds before this change ignore the extra asset and still check `SHA256SUMS` only, so
  they update to the first signed release as before.
- Losing or replacing the key leaves installed apps refusing new releases until their users
  install one by hand. A leaked key is worse: builds that ship the old public key keep trusting
  it, so replacing the secret does not stop whoever holds the key from signing assets they accept,
  and their users must install a new build by hand.
- The installer is not code-signed, so SmartScreen may warn when it runs. Code signing is out of
  scope.
- Every Windows start makes one unauthenticated GitHub API request (60 per hour per IP).
