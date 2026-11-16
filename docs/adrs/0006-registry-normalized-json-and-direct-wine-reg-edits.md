# ADR-0006: Registry as normalized JSON; Wine side edits `user.reg` directly

**Status:** Accepted
**Date:** 2026-11-16

## Context

Manifest registry keys look like `HKEY_CURRENT_USER/Software/Foo`; a key means the key and all of
its subkeys (research.md §1). On Windows, the live registry is reached through the `winreg` crate.
In a Wine prefix, `user.reg` holds HKCU and `system.reg` holds HKLM, in Wine's text format
(research.md §5). wineserver keeps the registry in memory and saves it every 30 s and on exit. A
file edited while it runs is overwritten. Ludusavi backs up whole `*.reg` files for Proton, which
cannot restore to Windows.

## Decision

- **Interchange form:** `registry.json` in the staging tree (ADR-0004) holds a sorted map from the
  full key path (`HKEY_CURRENT_USER\Software\Foo\Bar`, backslashes) to its values. A value is
  `{ name ("" = default), kind, data }`, with `kind` one of
  `sz | expand_sz | multi_sz | dword | qword | binary | other(u32)`. Key lookup is
  case-insensitive. JSON because `serde_json` is already needed for the manifest cache.
- **Scope v1:** HKCU only. HKLM manifest keys are skipped with a warning (writing them needs admin
  on Windows).
- **Windows:** `winreg` reads and writes the subtree (`cfg(windows)` module).
- **Wine:** the program parses and writes `user.reg` directly. The parser keeps the header and
  every untouched key block verbatim. On restore each backed-up key's subtree is replaced: the old
  subtree is deleted and the captured keys are written with a fresh timestamp. The file is written
  to a temp file and renamed into place.
- **wineserver guard:** registry writes are refused while **any** `wineserver` process runs for
  the user (`/proc/*/comm`). Reads during backup are allowed; they can be up to 30 s stale.
  Upgrade path: match the specific prefix through the wineserver's server directory (dev/inode of
  the prefix). That mechanism is not in research, so verify it before adopting.
- Escaping of non-ASCII and special characters in `user.reg` is not in research. To verify in
  plan step 4 against a fixture copied from a real prefix.

## Alternatives considered

- **`wine reg export/import`:** no format code needed. Rejected because it needs the right Wine
  binary for the prefix. Running a system wine against a Proton prefix can trigger a prefix update
  to the wrong Wine version. It also starts a wineserver itself.
- **Store Windows `.reg` text (`REGEDIT5`, UTF-16LE) as the interchange form:** a human can
  import it on Windows, but it needs another writer and parser for no v1 caller.
- **Copy whole `user.reg` (ludusavi's Proton approach):** cannot restore to native Windows.

## Consequences

- Restoring registry saves into a prefix requires closing every Wine/Proton game first. The GUI
  states this.
- The Wine parser/writer is pure text logic and can be round-trip tested on fixtures.
