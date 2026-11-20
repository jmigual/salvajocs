# ADR-0023: A backup remembers its Wine prefix, and a restore finds it again

**Status:** Proposed
**Date:** 2026-11-20

## Context

Tower Wizard (Godot) was backed up from the Bottles bottle `Generic-Game`. Bottles prefixes are
shared (ADR-0017), so no game owns that bottle. The snapshot holds
`WinAppData/Godot/app_userdata/Tower Wizard`.

The restore default (`restore::default_targets`, ADR-0005, ADR-0017) only looks for the custom
`wine_prefix`, Proton `compatdata/<steam.id>` and the game's own Heroic or Lutris prefix. None
exists for this game, so the default was native Linux only. The Godot engine rule (ADR-0011) then
mapped the root onto `XdgData/godot/app_userdata/Tower Wizard`, with the cross-build warning.

A second report: the batch confirm of Dave the Diver said "Into native Linux and the prefix
/home/…/compatdata/1868140/pfx", although every write and the registry went into the prefix. The
line named every target of the default, not the ones the plan writes into, and printed the full
path, home folder included.

## Decision

- **The snapshot records the prefix by description, never by path.** `SnapshotMeta` gains
  `prefix: Option<PrefixOrigin>`, the last field, omitted when `None` (ADR-0004):
  - `Proton(u32)`: the app id of `steamapps/compatdata/<id>/pfx`.
  - `Heroic(String)`: the Heroic app name (the `GamesConfig/<app>.json` stem).
  - `Lutris(String)`: the Lutris game slug (`game_slug`, else the file stem without its time).
  - `Bottles(String)`: the bottle's directory name in `bottles/bottles`, also for a bottle kept
    elsewhere.
  - `Home(String)`: a prefix below the home folder, as `<home>/<rest>`, `/`-separated, plain
    components only. A home of `/` records nothing, since every path would be below it.
  - Anything else (outside home, or native Windows) records nothing. No absolute path and no
    user name is ever stored.

  JSON looks like `"prefix": {"bottles": "Generic-Game"}`. The field is read leniently: an
  unknown kind or a malformed value reads as `None`, and the roots survive. `format` stays `1`.
- **Launcher names.** `LauncherPrefix` gains `name`, the launcher's own key for the prefix (the
  Heroic app name, the Lutris slug, the bottle's directory name), filled by `launcher::detect`.
- **What is recorded.** `scan::describe_prefix` describes the scan's prefix
  (`GameScan.prefixes.first()`, as `changes::Mapper` uses it) into `GameScan.prefix_origin`. It
  tries, in order: a launcher entry with that path mapped to this game (so Heroic's shared
  default prefix gives this game's app name), a shared launcher entry with that path, the Proton
  shape `steamapps/compatdata/<id>/pfx`, then a path below the native home folder. Ignored
  launcher prefixes are not launcher entries. `backup::stage` writes the description; when the
  scan had unreadable paths and found no prefix, it keeps the staged one, as it keeps the roots.
  The pre-restore safety backup records none.
- **Default targets on Linux**, in this order; each step yields a prefix only when its Wine user
  can be read, else the next step runs:
  1. the custom game's `wine_prefix` on this PC;
  2. the game's own prefix by id: Proton by Steam app id, then the Heroic or Lutris prefix mapped
     to the game;
  3. the prefix the snapshot names, resolved through this PC's launchers: `Proton(id)` under the
     detected and configured Steam roots, a launcher name against the detected prefixes not
     ignored, `Home` below this PC's home. When it resolves to several prefixes (the native and
     Flatpak installs of a launcher, two Lutris files with one slug, two Steam libraries), the
     one that holds the saves is chosen. When several hold them, none is chosen, the restore
     names them, and the search does not run. When none does, the search runs, and if it finds
     nothing either, the restore names the prefixes the name fits;
  4. a search, only for a snapshot of the Windows build (a root in the Windows folders, other
     than `WinAbsolute`): every custom `wine_prefix`, every `compatdata/*/pfx` of every Steam
     root, the launcher prefixes not ignored, and the `WinePrefix` roots. A prefix holds the
     saves when the folder of one of the snapshot's files in the Windows folders, resolved in it
     as a restore resolves the file (through its root, case-insensitive, with the prefix's
     user), is a non-empty folder. The files' folders are checked rather than the roots, since a
     generic root such as `My Games` holds other games' folders too. Files directly in a base
     folder are not checked, nor more than 32 folders per prefix. Exactly one hit is the
     default. Several hits choose none, and the restore names them so the user picks. A
     registry-only snapshot does not search: reading each prefix's `user.reg` costs megabytes,
     and such a backup records its prefix since this decision;
  5. native Linux only. Steps 1 and 2 are today's default, so nothing else remains.

  `native Linux` stays ticked beside the prefix, as before. `default_targets` takes the staged
  snapshot (`None` behaves as before: no stored prefix, no search) and returns
  `DefaultTargets`: the targets, why the prefix was chosen (`CustomGame`, `OwnPrefix`,
  `Backup`, `Saves`), the chosen prefix's description, the prefixes that all hold the saves
  when none was chosen, the prefixes the stored name fits when none of them holds the saves and
  the search found none, and whether the snapshot names a prefix this PC does not have or does not
  use (ignored, or without a Wine user). Windows hosts are unchanged.
- **The restore reads the snapshot before it picks targets.** The single-game detection job reads
  the listing (kept for the plan, so nothing is read twice) and drops a result for a snapshot no
  longer chosen; a failed read falls back to the default without a snapshot. The batch reads,
  then picks. When the user switches snapshots, the default is worked out again unless the user
  changed the pick.
- **The "Into …" line names only the targets the plan writes into** (`restore::used_targets`,
  from each write's target-side base and the registry), in the batch confirm and in the
  single-game plan. A prefix is named by its description, for example "the Steam Proton prefix
  of app 1868140", with the path on hover; only a prefix without a description shows its path.
  While the planned prefix is the default, the reason follows in brackets. The single-game plan
  also says when this machine does not have or does not use the snapshot's prefix, and names the
  prefixes of an ambiguous choice.

## Alternatives considered

- **Store the absolute path:** machine-specific, and it holds the user name.
- **Search only, without storing:** finds nothing before the game has run in the prefix here, and
  cannot tell two prefixes apart.
- **Store the prefix in a separate file:** one more read per restore, and `hashes.json` would
  need a new key.
- **Pick the prefix with the newest file when several hold the saves:** right when a game moved
  from one launcher to another, silently wrong otherwise.
- **Search only the game's own Proton app id:** step 2 already covers it; every `compatdata`
  also covers non-Steam shortcuts.
- **The stored prefix before the custom `wine_prefix` and the game's own prefix:** an explicit
  setting on this PC and an id match on where the game is installed are stronger than another
  machine's layout.
- **Leave the prefix out of skip rule (B):** needs a `Hashes` format change or a second read of
  the newest snapshot's `salvajocs.json`, and would keep this machine's state in a snapshot that
  names the other machine's prefix.

## Consequences

- No released version (0.1.0 to 0.6.0) has `deny_unknown_fields` on `SnapshotMeta` or compares
  `format`, so older versions read new snapshots and ignore the prefix. An older version that
  restages such a game writes `salvajocs.json` back without it.
- The first backup after the update rewrites `salvajocs.json` and `hashes.json` of each game
  found in a prefix: one new snapshot per such game per machine. After that, an unchanged game is
  skipped again. Games not in a prefix keep byte-identical metadata.
- Skip rule (B) across machines holds only for the same description: two machines with the same
  saves in different prefixes each store their own snapshot once per change. A machine still on
  an older version writes no prefix, so (B) does not hold between it and an updated one.
- The backup preview does not read `salvajocs.json` (ADR-0018), so the first backup after the
  update can show a game as unchanged and still back it up.
- A stored prefix costs a few `stat`s and a `read_dir`. The search runs only for a Windows-build
  snapshot with no custom, own or stored prefix here: one `read_dir` per Steam library, one per
  candidate for its user, and a `stat` and a `read_dir` per folder of the snapshot's files (at
  most 32). In a batch it runs per game.
- `restore::default_targets` takes the staged snapshot and returns `DefaultTargets`, and
  `LauncherPrefix`, `SnapshotMeta` and `GameScan` gain fields: a minor-version break of
  `salvajocs-core`.
