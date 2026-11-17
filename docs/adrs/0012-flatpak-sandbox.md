# ADR-0012: Flatpak sandbox: host XDG folders for games, user-confirmed wineserver guard

**Status:** Accepted
**Date:** 2026-11-17

## Context

salvajocs is published on Flathub as `io.github.jmigual.salvajocs`. Inside the sandbox
(detected by `/.flatpak-info`), `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `XDG_CACHE_HOME` point to
`~/.var/app/<id>/...`; Flatpak passes the host's values as `HOST_XDG_*`. `$HOME` is the real home.
The sandbox has its own PID namespace, so `/proc` shows none of the host's processes and the
wineserver guard (ADR-0006) would always report that no wineserver runs.

## Decision

- **Game folders come from the host.** In the sandbox `LinuxFolders::native` takes `xdg_data`
  and `xdg_config` from `HOST_XDG_DATA_HOME`/`HOST_XDG_CONFIG_HOME`, else `$HOME/.local/share`
  and `$HOME/.config`; relative values are ignored, as `dirs` does. This is the one place scan
  and restore get the native Linux folders. Steam roots derive from `$HOME` and need no change.
  `dirs::document_dir` is used only on Windows. No base uses the cache dir.
- **The app's own files stay in the sandbox:** the config (ADR-0009) and the manifest cache keep
  `dirs::config_dir`/`dirs::cache_dir`.
- **wineserver guard:** `prefix::wineserver_running` returns `Option<bool>`, `None` in the
  sandbox. `restore::run` keeps its `bool`. When the answer is `None` and the plan writes
  registry keys into a Wine prefix, the GUI asks the user to close every Wine and Proton program
  and confirm; Confirm runs the restore as if none runs, Cancel writes nothing.

## Alternatives considered

- **`flatpak-spawn --host` to read the host's processes:** needs the
  `org.freedesktop.Flatpak` talk permission, a full sandbox escape that Flathub's linter flags
  and grants only by exception.
- **Refuse registry restores into prefixes in the sandbox:** safe, but leaves Proton registry
  saves unrestorable in the Flathub build.
- **Reimplement XDG lookup for every Unix host:** would change the macOS paths `dirs` returns;
  only the sandbox needs other folders.

## Consequences

- A registry restore into a prefix from the Flatpak build relies on the user closing Wine; if a
  wineserver still runs, it overwrites the change on exit. The safety backup still holds the old
  keys.
- The manifest must grant filesystem access to the host folders salvajocs scans and restores
  into (home, Steam roots, prefixes).
- Tests inject the sandbox state (`LinuxFolders::native_with`, `Env::wineserver_running`), so
  no test needs a real sandbox.
