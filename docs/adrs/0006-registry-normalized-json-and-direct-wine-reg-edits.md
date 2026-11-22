# ADR-0006: Registry as normalized JSON; Wine side edits `user.reg` directly

**Status:** Accepted
**Date:** 2026-11-16

## Context

Manifest registry keys look like `HKEY_CURRENT_USER/Software/Foo`; a key means the key and all of
its subkeys (research.md §1). On Windows, the live registry is reached through the
`windows-registry` crate. In a Wine prefix, `user.reg` holds HKCU and `system.reg` holds HKLM, in
Wine's text format (research.md §5). wineserver keeps the registry in memory and saves it every
30 s and on exit. A file edited while it runs is overwritten. Ludusavi backs up whole `*.reg` files
for Proton, which cannot restore to Windows.

## Decision

- **Interchange form:** `registry.json` in the staging tree (ADR-0004) holds a sorted map from the
  full key path (`HKEY_CURRENT_USER\Software\Foo\Bar`, backslashes) to its values. A value is
  `{ name ("" = default), kind, data }`, with `kind` one of
  `sz | expand_sz | multi_sz | dword | qword | binary | other`; `other` is
  `{"kind":"other","data":{"code":<u32>,"bytes":[..]}}` and keeps types without a typed form (for
  example Unity's 8-byte `hex(4):` doubles). Key lookup is case-insensitive, on whole path
  elements. An export of key `K` holds `K` itself (possibly without values) and every existing
  key below it; restore deletes the subtree of each key in the export, except what a kept key
  owns (ADR-0026), then writes them. The HKCU root itself and paths with empty elements (`A\\B`,
  trailing `\`) are refused with a typed error: there is no whole-HKCU restore in v1. JSON
  because `serde_json` is already needed for the manifest cache.
- **Scope v1:** HKCU only. HKLM manifest keys are skipped with a warning (writing them needs admin
  on Windows).
- **Windows:** `windows-registry` 0.6.1 reads and writes the subtree (`cfg(windows)` module).
  The exported root key keeps the caller's spelling and keys below it the registry's (Wine and
  Windows JSON may differ in root case; replace matches case-insensitively). Replace deletes
  every subtree before writing, so it is not atomic; with a kept key below an exported key, it
  clears that key and removes only the subkeys the kept keys do not own (ADR-0026). Restore
  backs up the current state first (plan step 15). Windows may append a NUL when writing an
  unterminated `REG_SZ`; the string content is unchanged but the bytes are not exact.
  `winreg` 0.56 was rejected: its `RegType` is a closed enum and `get_raw_value` fails on type
  codes above 11, so `RegData::Other` could not round-trip. `windows-registry` 0.100 needs Rust
  1.95, above the workspace's 1.91, hence 0.6.1. Its `set_bytes` debug-asserts terminated data for
  `String`/`ExpandString`/`MultiString`, so `Other` values are written as `Type::Other(code)`: an
  empty `REG_SZ` from Wine's `hex(1):` is valid data.
- **Wine:** the program parses and writes `user.reg` directly. The parser keeps the header and
  every untouched key block verbatim. On restore each backed-up key's subtree is replaced: the old
  subtree is deleted and the captured keys are written with a fresh timestamp. The file is written
  to a temp file and renamed into place.
- **wineserver guard:** registry writes are refused while **any** `wineserver` process runs for
  the user (`/proc/*/comm`). Reads during backup are allowed; they can be up to 30 s stale.
  `restore::run` asks again after the safety backup and before the first write, since a game can
  start meanwhile; it then refuses with nothing written but that backup.
  Upgrade path: match the specific prefix through the wineserver's server directory (dev/inode of
  the prefix). That mechanism is not in research, so verify it before adopting.
- **Escaping (verified in plan step 4):** wineserver writes every UTF-16 unit above 127 as
  `\x<hex>` (surrogate pairs as two escapes), never raw UTF-8; see the `registry::wine` module
  doc for the full rules. The key-name case is verified against a real Proton prefix (an emoji
  key written by wineserver). Non-ASCII *value strings* go through the same `dump_strW` in
  wineserver's source, but no real prefix here has one, so they are verified from source only.

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
