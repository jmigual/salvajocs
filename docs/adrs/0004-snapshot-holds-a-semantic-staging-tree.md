# ADR-0004: A snapshot holds a staging tree keyed by semantic base, stored at a fixed path

**Status:** Accepted
**Date:** 2026-11-16

## Context

A save backed up from a Proton prefix
(`.../compatdata/<id>/pfx/drive_c/users/steamuser/AppData/Roaming/Foo`) has to restore to
`%APPDATA%\Foo` on Windows, and the reverse. A restic-format snapshot stores the paths it was
given (research.md §3). rustic_core's `BackupOptions.as_path` replaces the one source path with
any path we choose (ADR-0003). Ludusavi stores absolute paths plus a `mapping.yaml` and relies
on manual redirects (research.md §2). Its unreleased `redirectWine` classifies paths into a
semantic base (`WinAppData`, `WinDocuments`, …) plus a tail.

## Decision

Each game is copied into a **staging tree**, which is backed up as one source with
`as_path = /salvajocs/<slug>` (slug: ADR-0010). The snapshot path is therefore the same on every
machine and OS.

```
<cache>/salvajocs/stage/<slug>/   →  stored as /salvajocs/<slug>/
  salvajocs.json                  # SnapshotMeta { format: 1, title, roots: [(Base, rest)] }
  files/<Base>/<tail...>          # one dir per semantic base
  registry.json                   # optional, ADR-0006
  hashes.json                     # since 0.6.0, ADR-0018: SHA-256, size and mtime of each
                                  # file under files/, plus hashes of salvajocs.json and
                                  # registry.json
```

- `Base` is taken from the template's **leading placeholder** together with the **evaluation
  context** (ADR-0005), never by reverse-matching real paths:
  - Windows-like context (native Windows or a Wine prefix): `<home>`→`WinHome`,
    `<winDocuments>`→`WinDocuments`, `<winAppData>`→`WinAppData`,
    `<winLocalAppData>`→`WinLocalAppData`, `<winLocalAppDataLow>`→`WinLocalAppDataLow`,
    `<winPublic>`→`WinPublic`, `<winProgramData>`→`WinProgramData`, `<winDir>`→`WinDir`.
    `<home>/AppData/Roaming`, `<home>/AppData/Local` and `<home>/AppData/LocalLow` are normalized
    to `WinAppData`, `WinLocalAppData` and `WinLocalAppDataLow`. The manifest writes Unity's
    `LocalLow` as `<home>/AppData/LocalLow/...`. One name per folder lets ADR-0011's rules
    match on the base. These normalizations change only the base and tail split, not the path.
  - Linux-native context: `<home>`→`Home`, `<xdgData>`→`XdgData`, `<xdgConfig>`→`XdgConfig`.
    `<home>/.config` and `<home>/.local/share` are normalized to `XdgConfig` and `XdgData`. The
    manifest writes Unity's Linux path as `<home>/.config/unity3d/...` 1 021 times and as
    `<xdgConfig>/...` 628 times (ADR-0011).
  - Any context: `<base>`→`GameDir` (tail relative to the install dir), `<root>`→`StoreRoot`
    (tail relative to the root).
  - A template without a leading placeholder is a custom absolute path and restores to the same
    OS only:
    - Unix → `Absolute`, tail = the path without its leading `/`:
      `/opt/game/save.dat` → `(Absolute, "opt/game/save.dat")`. It resolves on a Unix host only.
    - Windows drive-letter path → `WinAbsolute`, tail = the drive letter without the colon, then
      the path (`C:` is not a valid directory name on Windows, so `files/Absolute/C:/…` is
      impossible): `C:\Games\Foo\save.dat` → `(WinAbsolute, "C/Games/Foo/save.dat")`, restored
      to `C:\Games\Foo\save.dat`. It resolves in a native Windows context only. UNC paths are not
      classified. A Windows path on a Linux host is not absolute, so it stays unclassified.
  This matters because the manifest uses `<home>` for Windows-only entries. Under Proton that is
  `users/<wineuser>`, not the Linux home.
- `SnapshotMeta.roots` lists, for each template that produced files, its glob-free prefix as
  `(Base, rest)`, for example `(XdgConfig, "unity3d/Co/Game")`. A cross-build restore
  (ADR-0011) maps these roots. Everything else only needs `files/`.
- Found files are deduplicated by real path, since two templates can match the same file.
- The tail is the matched real path with the resolved base prefix removed. Tails keep the case
  they had on disk.
- The staging dir is fixed per machine and game and kept between backups as a **mirror**:
  - A file is copied only when its size or mtime differs from the source. Its mtime is then set
    to the source's with std `File::set_modified`.
  - Staged files whose source is gone are removed, except for a game whose scan reported
    unreadable paths: it is staged without deletions, so a read error never removes saves from
    the next snapshot. Its `salvajocs.json` also keeps the roots it already lists, after the
    scan's own, so restore still finds the kept files.
  - `salvajocs.json` and `registry.json` hold no timestamps, are serialized deterministically
    (sorted maps), and are rewritten only when their bytes change.
  An unchanged game therefore keeps identical file metadata, and `skip_if_unchanged`
  (ADR-0010) sees an identical tree. In the ADR-0003 spike, the same untouched source gave the
  same tree id twice and the second snapshot was skipped.
- **Restore** lists `/salvajocs/<slug>` and reads `salvajocs.json`, `registry.json` and
  `hashes.json` to plan; it restores the subtree only when the user restores (ADR-0020).

## Alternatives considered

- **Back up real paths plus a mapping file in the snapshot:** no copy on backup. But the mapping
  file and registry export still need a staging dir, restore still needs translation, and the
  snapshot tree is host-specific and unreadable to a human on another OS.
- **One rustic backup call per real source dir, each with its own `as_path`:** no copy, but one
  snapshot per source dir instead of one per game, and the metadata and registry still need a
  staged source.

## Consequences

- Each save exists twice on the local disk: in its real location and in the staging mirror
  under the cache dir. Saves are usually small.
- Only changed files are copied on each backup.
- The snapshot is self-describing. A user can browse `/salvajocs/<slug>/files/WinAppData/...`
  with rustic or restic.
- `<osUserName>` stays literal in `SnapshotMeta.roots`. On restore it matches exactly one
  path component of the tail, and the write's tail takes the target family's user there
  (ADR-0005), so a Proton save under `steamuser/` lands under the Windows user's name.
  `<storeUserId>` inside a tail is stored as is and not translated in v1.
