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
  - A template without a leading placeholder (custom absolute path) → `Absolute`, with the tail
    being the full path. It restores only to the same OS.
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
  - Staged files whose source is gone are removed.
  - `salvajocs.json` and `registry.json` hold no timestamps, are serialized deterministically
    (sorted maps), and are rewritten only when their bytes change.
  An unchanged game therefore keeps identical file metadata, and `skip_if_unchanged`
  (ADR-0010) sees an identical tree. In the ADR-0003 spike, the same untouched source gave the
  same tree id twice and the second snapshot was skipped.
- **Restore** opens the node at `/salvajocs/<slug>` (`node_from_snapshot_and_path`) and restores
  that subtree into `<cache>/salvajocs/restore/<slug>`. Then it reads `salvajocs.json` and
  `files/`. If the Windows CI check in ADR-0003 shows Windows stores the path differently, the
  fallback is to find `salvajocs.json` with `ls`.

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
- `<osUserName>` or `<storeUserId>` inside a tail is stored as is and not translated in v1.
