# ADR-0005: Bases resolve against a target context, which is plain data

**Status:** Accepted
**Date:** 2026-11-16

## Context

ADR-0004 stores `<Base>/<tail>`. On restore each base must become a real directory on this
machine: native Windows folders, a Wine/Proton prefix with its own user (`steamuser` for Proton,
the Unix user for plain Wine; research.md §6), or Linux XDG dirs. `<base>`/`<root>` depend on
where the game is installed, which differs per machine and OS. Ludusavi resolves these through
configured roots (research.md §2).

## Decision

- **Contexts are plain data**, built by detection functions and constructible in tests:
  - Contexts carry `user`, the value of `<osUserName>`: `native()` sets the OS user and
    `WinFolders::for_prefix` the Wine user. `GameVars` does not hold it.
  - `WinFolders { user, home, documents, app_data, local_app_data, local_app_data_low, public,
    program_data, windows }`:
    - `WinFolders::native()` on Windows uses `dirs` (Known Folders, so a OneDrive-redirected
      Documents folder works); `local_app_data_low` = `<home>/AppData/LocalLow`; `public`,
      `program_data` and `windows` come from `%PUBLIC%`, `%PROGRAMDATA%` and `%WINDIR%`.
    - `WinFolders::for_prefix(prefix, user)` joins `drive_c/users/<user>/...`,
      `drive_c/users/Public`, `drive_c/ProgramData` and `drive_c/windows`.
  - `LinuxFolders { user, home, xdg_data, xdg_config }`.
  - `Context::{WindowsNative(WinFolders), WinePrefix { prefix, folders: WinFolders },
    Linux(LinuxFolders)}`. The prefix variant keeps the prefix path, because restore needs it for
    `user.reg` and for case-insensitive matching.
  - `Roots`: configured list of `{ path, kind: Steam | Other | WinePrefix }` plus auto-detected
    Steam roots (ADR-0007). `GameDir` = `<root>/steamapps/common/<installDir>` for Steam roots and
    `<root>/<installDir>` for Other. `<installDir>` comes from the manifest `installDir`, falling
    back to the title. `StoreRoot` = the Steam root.
- **Username swap is implicit.** The Windows user name is never stored. `WinHome` and the others
  resolve to whichever user the target context has. Where a root holds `<osUserName>`, each
  write's tail is the root's rest with `<osUserName>` replaced by the target family's `user`,
  plus the matched remainder, so a mixed restore uses each family's own user (ADR-0004).
- **A snapshot is untrusted input.** Its roots are trusted only when they equal or lie below
  (component-wise, ASCII case ignored for `Win*` bases, no `..`) the classified glob-free prefix
  of one of the game's declared templates: the entry's file templates (manifest merged with the
  custom game), a side of one of its saved pairs, or an engine-rule target of those. "Below"
  keeps a pre-restore snapshot of a mapped root (pair side plus a subdirectory) restorable.
  A prefix from a globbed template (`<base>/*.sav`, `<winDir>/System32/x-s[number]`) trusts
  only files whose tail (or a parent dir) matches that template's glob.
  Only files under a trusted root and registry keys under the keys the entry declares
  are restored; anything else is skipped with a reason, and without an entry nothing is.
  A pre-restore snapshot whose roots came from a per-file override (`Absolute`/`WinAbsolute`)
  is therefore not restorable through the normal flow; its files show as skipped.
- **Target choice on restore:** one restore has up to two targets,
  `Targets { linux: Option<LinuxFolders>, win: Option<Context> }`, where `win` is
  `WindowsNative` or `WinePrefix`. Each base resolves against the target that has it, so a
  snapshot holding both base families restores both.
  - Windows host: `win` = native `WinFolders`, `linux` = `None`.
  - Linux host: `linux` = `LinuxFolders`. `win` = the game's own prefix when it exists: the
    custom game's `wine_prefix`, else Proton `compatdata/<steam.id>`, else its launcher
    prefix (ADR-0017).
  - On a Linux host the user always picks the build: native Linux (`linux` set, `win` unset)
    or Windows in a prefix (`win` set, `linux` unset), whatever bases the snapshot has; a
    mixed snapshot may keep both. `default_targets` supplies the default. A leftover Proton
    prefix must not force the Windows path, and a Linux-only snapshot can go into a Proton
    build.
  - When a base family has no target on this machine, the restore is a **cross-build
    restore** (ADR-0011). Examples: Linux bases on a Windows host, or `Win*` bases on Linux with
    no prefix found. Those roots are mapped onto the other family. On Linux with `Win*` bases
    and no prefix, the dialog also offers picking a prefix instead.
  - This choice is `restore::default_targets` in core, not GUI code (ADR-0002).
  - `GameDir`/`StoreRoot`: resolved through this machine's roots and the game's `installDir`
    candidates (`GameVars`, the same input the scan uses). If the game is not found, those
    files are reported as skipped.
- **Restore is plan then run.** `restore::plan` maps every staged file to a target path, or to a
  skip reason. The GUI shows the plan before anything is written.
- **A safety backup is built from the plan.** Before writing, the files that exist at the plan's
  write targets, plus the registry subtrees the restore will replace in that target, are staged as
  `<Base>/<tail>` and backed up with an extra `pre-restore` tag. Its `SnapshotMeta` gets `roots`
  for the target roots it covers, so a pre-restore snapshot can itself be restored cross-build.
  This covers a prefix the user picked by hand, which a fresh default scan would miss.
  Restoring it undoes what the restore overwrote, not what it added: files and registry keys
  the restore newly created are in no snapshot of the old state and stay behind.
- **Registry without a Windows target:** in a cross-build restore with no `win` target,
  `registry.json` is skipped with the reason "registry: no Windows target". It is never
  dropped silently.
- **Per-file override:** the plan preview lets the user change one write's target to another
  absolute path, for this restore only. `RestorePlan::override_target` validates it. Pairs
  (ADR-0011) stay per root. The safety backup stages an existing file at an overridden path
  under `Absolute`/`WinAbsolute` with its full path. Nothing the restore overwrites goes unbacked.
- **No safety backup, no restore.** Restore runs as one entry point, `restore::run`, which does
  the safety backup and then applies. If the safety backup fails, nothing is written. "Nothing
  existed at the targets" counts as success.
- **Case-insensitive into Windows-like targets.** When the target is a prefix, each path
  component matches an existing directory case-insensitively before a new one is created.
  Otherwise `My Games/` and `my games/` would both exist on a case-sensitive filesystem. Scanning
  inside a prefix is case-insensitive for the same reason (ADR-0008).

Support matrix for v1:

| Backed up from | Restored to | Supported |
| --- | --- | --- |
| Windows build, native Windows | same build in Wine/Proton prefix | yes |
| Windows build, Wine/Proton prefix | native Windows / another prefix | yes |
| Native Linux build | Linux | yes |
| Native Linux build | Windows build (or reverse) | yes, through a cross-build mapping (ADR-0011): saved pair, engine rule or unambiguous manifest pair, otherwise the user maps each root in a dialog. Same path pair does not mean same file format; the restore warns |
| `Absolute`/`WinAbsolute` custom paths | same OS only | yes, same path |
| macOS | — | not in v1 |

## Alternatives considered

- **Store the original Windows user and rewrite it:** this is what an absolute-path design
  needs. A semantic base removes the need.
- **Contexts as traits with live OS calls:** the Proton→Windows round trip could then only be
  tested on Windows. Plain data lets a Linux test build a synthetic Windows context in a tempdir.

## Consequences

- The core feature is covered by ordinary Linux tests (plan steps 14 and 15). Native Windows
  folders are exercised by the Windows CI job (plan step 7).
- A game installed outside every configured root loses its `GameDir` files on restore until the
  user adds the root.
