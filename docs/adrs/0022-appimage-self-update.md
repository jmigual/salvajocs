# ADR-0022: AppImage self-update: replace the file in one rename, restart on request

**Status:** Accepted
**Date:** 2026-11-19

## Context

ADR-0013 checks for updates on Windows only, so the Linux AppImage showed no notice. Gear Lever
and AppImageUpdate can update the AppImage from its embedded update information, but a user who
does not open them never learns that a release exists. A running AppImage reads its own file
through a FUSE mount, so writing into that file in place would break the running app; a rename
replaces the directory entry while the open file stays readable.

## Decision

- **Check (amends ADR-0013):** `Env::check_update` is also set when the app runs as an AppImage:
  `$APPIMAGE` is a file and the running exe lies in `$APPDIR`. An app started from another
  AppImage inherits that one's variables, and the `$APPDIR` test keeps it from replacing the other
  app's file (`update::running_appimage`). Settings shows "Check for updates at start" there too.
- **Notice:** when the release is found, `update::replaceable` probes the AppImage's folder by
  creating and removing a file next to it. A writable folder gets **Update now**; a read-only one
  keeps the release page link, so it never shows a failed download.
- **Update now:** a job downloads `salvajocs-<version>-x86_64.AppImage` and `SHA256SUMS` through
  the installer's code (same URL, HTTPS and size checks) and refuses a hash that is missing or
  differs, before anything is written. Requests time out after 30 s to connect or 60 s without
  data, never on the total, which a slow link would exceed. It then writes `.<name>.update-<pid>`
  next to `$APPIMAGE` with the old file's mode plus the owner's execute bit (set after creating,
  so the umask does not mask it), syncs it and renames it over `$APPIMAGE`, which keeps the
  user's file name. A failed step removes the new file.
- **Restart now:** once no background job runs (`App::busy`, as for the installer), the window
  closes, and `on_exit` starts `$APPIMAGE` with the same arguments (`Env::restart_appimage`) after
  writing the unsaved choices, which the new instance reads at start. Unlike the installer, the
  new AppImage does not wait for the old process to exit. When the close dialog's **Wait** cancels
  that close, the restart goes back to waiting for the jobs, so a later quit does not restart.
- Other Linux builds (tarball, `cargo install`, mise) still do not check.

## Alternatives considered

- **Link only, Gear Lever does the update:** no download code, but the user leaves the app for
  each update.
- **zsync delta through AppImageUpdate's library:** smaller downloads, but no Rust crate and a C
  dependency; the AppImage is small enough to download whole.
- **Write the new file under its release name and delete the old one:** breaks the desktop file
  and launchers that point at the old path.
- **Restart automatically after the download:** would close the window without the user asking.

## Consequences

- The hash protects against a corrupted or truncated download, not a compromised release, as in
  ADR-0013.
- A crash between creating and renaming the temp file can leave a hidden `.update-<pid>` file next
  to the AppImage.
- An AppImage owned by another user in a sticky folder such as `/tmp` passes the probe, and the
  rename then fails with "The update failed"; the old file stays.
- The runtime consumes `--appimage-extract-and-run`, so the restart lacks it and fails without
  FUSE 2; the user starts the app again by hand. `APPIMAGE_EXTRACT_AND_RUN=1` is inherited and works.
- The new app inherits the old runtime's keepalive pipe, so the old FUSE mount and the replaced
  file stay until the new app exits. Closing the descriptor in the child needs `unsafe` code
  (`pre_exec`), which the workspace forbids.
