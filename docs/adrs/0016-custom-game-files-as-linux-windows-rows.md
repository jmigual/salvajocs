# ADR-0016: Custom game files are rows with a Linux and a Windows path

**Status:** Accepted
**Date:** 2026-11-18

## Context

A custom game (ADR-0008) listed its files as plain templates in `entry.files`, with no `when.os`.
A template therefore applied on every OS where it classified: `<home>/Foo` was scanned as the
Linux home on a native build and as the Wine user's home in a prefix. The cross-build mapping
(ADR-0011) was a second list, `pairs`, typed separately. The user asked for one list in which
each entry has a Linux path and a Windows path, only one of them mandatory: an entry with one
path is backed up and restored on that system only, and an entry with both says the two paths
hold the same file or folder.

## Decision

- **Schema:** `CustomGame.files: Vec<CustomFile>`, `CustomFile { linux: Option<String>,
  windows: Option<String> }`. Registry keys and install folders stay in `entry.registry` and
  `entry.install_dir`. The app no longer writes a custom game's `entry.files`; it still reads
  it as-is.
- **Breaking change in 0.4.0.** Configs from 0.3.x are not converted: custom games must be
  entered again. Nobody has backups that depend on them yet.
- **Backup:** `CustomGame::game_entry()` returns `entry` plus one save-tagged `FileRule` per
  side, with `when.os: linux` for the Linux side (native Linux build) and `when.os: windows` for
  the Windows side (native Windows or a Wine/Proton prefix), the same meaning `when.os` has for
  manifest entries (ADR-0008). A template on both sides, or in several rows, gets both OSes.
  A template already in `entry.files` keeps its rule. `merge_custom` uses `game_entry()`.
- **Validation (`crossbuild::validate_file`)**, textual so every host gives the same answer:
  - at least one side;
  - Linux side: a Linux or install-relative placeholder template (`<home>`, `<xdgData>`,
    `<xdgConfig>`, `<base>`, `<root>`), or an absolute path starting with `/`;
  - Windows side: a Windows or install-relative placeholder template, or a drive path
    (`C:\...` or `C:/...`);
  - wildcards, `<game>`, `<osUserName>` and `<storeUserId>` are allowed; relative paths and
    `.`/`..` components are not.
  - A row with two different sides is also a mapping: the glob-free prefix of each side must be
    a valid pair side (ADR-0011), so not an absolute path (the error says to split the row in
    two), and a prefix on a Linux or Windows base needs a folder below the placeholder.
- **Restore:** each row with two different sides gives a pair of its glob-free prefixes
  (`CustomGame::file_pairs`), e.g. `<xdgData>/g/*.sav` and `<winDocuments>/G/Saves/*.sav` give
  `<xdgData>/g` and `<winDocuments>/G/Saves`. `restore::game_info` lists these row pairs first,
  then the saved `pairs`, so a saved pair wins a tie. A row pair is dropped when a saved skip
  classifies equal to either side, so a skip keeps winning over it.
- A row with one side gets no implicit skip on a cross-build restore: its root goes through the
  usual order of ADR-0011 (saved pair or skip, engine rule, auto-pair, dialog).
- Pairs and skips that the cross-build dialog saves stay in `pairs` and `skips`, as restore-only
  mappings. `apply_choices` does not store a pair that a row already declares.

## Alternatives considered

- **A files list plus a separate list of Linux/Windows pairs:** what 0.3 had. The user rejected
  it: one list says at backup time which two paths are the same file or folder.
- **Convert 0.3 configs:** each old template would need an OS guess (`<home>` is both).
  Without existing users, entering the games again is cheaper than a converter.
- **Skip the root of a one-sided row in a cross-build restore:** rejected by the user. The
  engine rules, the auto-pair and the dialog still help when the other build's path is unknown.

## Consequences

- `<home>/Foo` on the Linux side is no longer scanned inside a prefix.
- Row pairs widen restore trust no further than a typed pair: they are declared roots like the
  sides of `pairs` (ADR-0005), and the folder-below rule keeps `<home>/*.sav` from trusting a
  whole home.
