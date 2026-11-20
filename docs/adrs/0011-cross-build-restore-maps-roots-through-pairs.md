# ADR-0011: A cross-build restore maps snapshot roots through pairs, with a dialog for the rest

**Status:** Accepted
**Date:** 2026-11-16
**Amended:** 2026-11-20: saved pairs and skips go to the repository with the custom game
(ADR-0028).

## Context

A native Linux build and a Windows build of one game keep saves in different places, e.g.
`<xdgConfig>/unity3d/Co/Game` vs `<home>/AppData/LocalLow/Co/Game`. Restoring one onto the other
is mostly a path problem: the manifest lists both paths but does not say which Linux path matches
which Windows path. Two things remain that paths can't solve:

- Formats can differ. Unity `PlayerPrefs` live in the Windows registry under
  `HKEY_CURRENT_USER\Software\<Company>\<Product>`, but in a file in
  `~/.config/unity3d/<Company>/<Product>` on Linux (Unity 6000.6 `PlayerPrefs` docs).
- Some games may simply write different formats per build.

Measured on the live manifest (2026-11-16, throwaway program parsing it with serde-saphyr).
A "Linux" entry has `when.os: linux`, or no OS and an `<xdg*>` placeholder; "Windows" likewise
with `windows`/`<win*>`.

| Count | Games |
| --- | --- |
| Non-alias games | 52 421 |
| … with save-tagged files | 13 330 |
| … with a Linux save entry | 1 430 |
| … with both Linux and Windows save entries | 1 238 |
| … exactly one of each (unambiguous) | 1 003 (860 with no glob in either; 733 with the same last path component) |
| … of the 1 238, Linux path under `unity3d` | 413 (344 of them one-each) |
| … Linux path under `godot` | 15 |
| … also with a save-tagged registry key | 6 |

Spelling of Unity and Godot paths in the manifest (all file entries, any tag), by leading
placeholder:

- `unity3d`: `<home>/.config/unity3d` 1 021, `<xdgConfig>` 628, a few typos (`<home>/config/...`,
  `<home>/.configs/...`).
- `godot`/`app_userdata`: `<xdgData>` 54, `<home>/.local/share` 43, `<home>/.godot/app_userdata`
  15 (a legacy layout not in current Godot docs).

Rules therefore match on the normalized base (ADR-0004), not on the spelling.

Engine layouts, from primary sources:

- **Unity** `Application.persistentDataPath` (Unity 6000.6 docs):
  - Windows: `%userprofile%\AppData\LocalLow\<company>\<product>`;
  - Linux: `$XDG_CONFIG_HOME/unity3d/<company>/<product>`.
- **Godot** `user://` (Godot stable docs, "Data paths"):
  - Windows: `%APPDATA%\Godot\app_userdata\<project>`;
  - Linux: `~/.local/share/godot/app_userdata/<project>`.
  - With a custom user dir: `%APPDATA%\<name>` and `~/.local/share/<name>`. That form is too
    generic to detect.

## Decision

- **Pair:** `Pair { linux: String, windows: String }`, two glob-free path templates naming the
  same logical directory. A custom game (or an `extend` override of a manifest game, ADR-0008)
  carries `pairs: Vec<Pair>`. There is no general redirect engine. A custom game's file row
  with two placeholder paths also proposes a pair, ranked below the saved ones, that declares
  no roots (ADR-0016).
- **Skip:** a root the user skipped is stored as its placeholder template in
  `skips: Vec<String>` on the same custom game, so the dialog does not return for it. A skip
  has no family: it matches a root of the source family whose base and rest equal its own
  (component boundary, ASCII case ignored for Windows), and the deepest match among pairs and
  skips wins (a pair on a tie). So a `<home>/Foo` skip applies in both directions: it also skips
  the Windows root `<home>/Foo` on a later Windows to Linux restore, and choosing a target for
  that root removes the skip. The editor lists every skip, so the user can remove it. A skip
  passes the same validation as a pair side. Choosing a target for a root removes its
  skip, and skipping a root removes its pair.
- **Mapping a root:** `SnapshotMeta.roots` (ADR-0004) holds `(Base, rest)` per root. A pair
  side classifies the same way. A root matches a pair side when its base is equal and its `rest`
  starts with that side's rest, compared case-insensitively for Windows. Files under the root
  keep their remaining tail under the other side.
- **Where a mapping comes from, in order:**
  1. **Saved:** the game's `pairs`, or a skip (`Saved` with no target: the root is not
     restored, no dialog).
  2. **Engine rule:** a small table in `core::crossbuild`, matched on the root alone, so it works
     for custom games too.
     - Unity: `<xdgConfig>/unity3d/{c}/{p}` ↔ `<winLocalAppDataLow>/{c}/{p}`.
     - Godot: `<xdgData>/godot/app_userdata/{p}` ↔ `<winAppData>/Godot/app_userdata/{p}`.
       The docs say `~/.local/share` and do not mention `$XDG_DATA_HOME`. That only affects
       where files land, which the manifest's own `<xdgData>` already decides. The legacy
       `~/.godot/app_userdata` layout gets no rule (no current primary source).
     Other engines are added only with a primary source.
  3. **Auto-pair:** the game's manifest entry has exactly one save-tagged Linux entry and exactly
     one Windows entry. Their glob-free prefixes form the pair, when the text after both
     prefixes is the same (none, or the same glob); otherwise one side would map into a
     shallower folder than the other, so the root is unknown (ADR-0016).
  4. **Unknown:** no default. The dialog lists the manifest's save templates for the target OS
     as candidates; the user picks one, types a path, or skips the root.
- **Who computes what:** `restore::prepare(staged, targets, vars, entry, pairs, skips, choices)` is
  the core entry point. It calls `crossbuild::propose`, then `crossbuild::resolve`
  (`MappingEntry`s + user `Choice`s → `ResolvedMapping`s; a choice for a root replaces its
  proposal, whatever the confidence), then `restore::plan`. Missing choices
  return `NeedsMapping(Vec<MappingEntry>)`. The GUI only shows those entries and calls
  `prepare` again with the choices (ADR-0002).
- **What a user may type:** placeholder templates only (e.g. `<winLocalAppDataLow>/Co/Game`),
  validated with `layout::classify`; a template that does not classify gets an inline error.
  This keeps ADR-0004's "no reverse-matching of real paths". Candidate templates are offered in
  a dropdown, so typing is rarely needed.
- **Proposal and dialog:** `crossbuild::propose` runs in core. It returns one entry per root:
  source root, file count, proposed target, `Confidence::{Saved, EngineRule, AutoPair,
  Unknown}`, candidate targets, and notes.
  - Unity note: if the Windows side has the `HKEY_CURRENT_USER/Software/{c}/{p}` key in the
    manifest or in `registry.json`, a prefilled note says "settings (PlayerPrefs) are stored
    differently and are not converted". Notes always reach the preview (`RestorePlan.notes`),
    whether the root was mapped automatically or chosen.
  - `propose` returns `Saved`, `EngineRule` and `AutoPair` entries already resolved. They are
    applied automatically, with no dialog. The plan preview shows where every file goes, plus
    the cross-build warning, and the per-file override is available there.
  - On a Linux host the target control always offers both "Windows build in prefix X" and
    "native Linux build (cross-build mapping)", whatever bases the snapshot has. It defaults from
    `default_targets`, so a stale Proton prefix doesn't force the Windows-build path.
  - A file-level or globbed template pairs through its glob-free parent directory. The tail
    under it may then nest differently in the other build, so the preview shows every
    resulting path.
  - **The dialog appears only when at least one root is `Unknown`.** `prepare` then returns
    `NeedsMapping` with every root of the game. The known roots are shown prefilled and
    editable; the unknown ones must be chosen or skipped.
  - On confirm, all of that game's roots are written to its `pairs`, or to its `skips` for a
    skipped root (creating an `extend` custom game if needed), and saved to the repository on
    the game's newest copy (ADR-0028). The next restore of that game needs no dialog. Pairs and
    skips stay editable in the Custom games tab.
  - Rows are per root. The user asked "where to restore each file"; a single file can still be
    redirected in the plan preview (per-file override, ADR-0005). That override is not saved as
    a pair.
- Every cross-build restore shows the warning "the other build may use a different save
  format". The pre-restore safety backup (ADR-0005) makes it reversible.

## Alternatives considered

- **No cross-build restore:** that was the earlier v1 position. The manifest numbers show 1 003
  games are unambiguous and 344 of those follow Unity's documented layout. Refusing is too
  conservative.
- **Pair on equal last path component:** 733 of 1 003 one-each games share a last component,
  but games with several entries would need guessing. Rejected in favour of the strict
  one-each rule plus the dialog.
- **Convert Unity PlayerPrefs (registry ↔ prefs file):** the Linux file format is not
  documented by Unity. Out of scope for v1; a possible follow-up.

## Consequences

- Cross-build restores of about 1 000 manifest games are mapped automatically, with the preview
  as the check. The rest need one dialog the first time, then never again.
- Since the auto-pair requires equal tails (ADR-0016), manifest games whose two save templates
  end differently ask once instead of being auto-mapped: 78 of about 1 003 one-each entries
  (measured on the live manifest, 2026-11-18), about 36 of which engine rules still map. Pairing
  them anyway would put one side into a shallower folder, and a wrong mapping writes the saves
  where the other build does not read them.
- A wrong pair writes files where the other build doesn't read them. The safety backup and the
  warning cover this; nothing is deleted.
