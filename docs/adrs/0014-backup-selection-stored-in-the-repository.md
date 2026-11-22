# ADR-0014: Backup selection stored in the repository, one settings snapshot per game

**Status:** Accepted
**Date:** 2026-11-18
**Amended:** 2026-11-20: the `settings:` tag prefix is reserved for game selections and the
retention policy; other settings snapshots, such as custom games, use another prefix (ADR-0028).
**Amended:** 2026-11-22: a save that finds another machine's snapshot written meanwhile forgets
its own and fails with `NotWritten`, so the changes stay pending (see Consequences).

## Context

Like Ludusavi, the user wants to choose which of a game's found files, folders and registry keys
are backed up, by ticking nodes of a tree (recursively), and to turn a whole game off. Every
machine that opens the repository should share these choices. A backup of the same game runs
from several machines, and the pre-restore safety backup (ADR-0005) must keep backing up
everything the restore would overwrite.

## Decision

- **Model** (`core::selection`, pure): per game a `GameSettings { format: 1, skip, paths,
  registry }` as JSON, empty fields left out.
  - `paths` maps a path key to included or not. The key is the staging path (ADR-0004):
    `<Base>`, `<Base>/<dir>` or `<Base>/<tail>`.
  - A path takes the value of its deepest saved ancestor-or-self; none means included. `a` is
    not an ancestor of `ab`. Tails below Windows-family bases compare ignoring ASCII case, as the
    scan does in prefixes; Linux bases, `GameDir` and `StoreRoot` compare exactly.
  - Setting a node removes every entry below it and saves the node only when its value differs
    from the inherited one, so the map stays minimal.
  - `registry` holds the excluded manifest keys, matched ignoring ASCII case.
  - Unknown fields are kept on rewrite, so an older app never drops a newer one's data. A
    higher `format` is an error: those settings are neither applied nor rewritten.
- **Applied outside the backup:** the GUI backup job filters each scan with
  `selection::apply` before `backup::backup_games`. A skipped game, or one with nothing left,
  gets no snapshot. `backup_games` and `stage` stay unfiltered, so the pre-restore safety backup
  covers everything.
- **Storage:** one settings snapshot per game, `as_path = /salvajocs-settings/<slug>` holding
  `settings.json`, tags `salvajocs-settings` and `settings:<slug>`, label = title. Without the
  `salvajocs` or `game:` tags it stays out of the restore list (ADR-0010). Since 0.4.0
  `load_all` reads every `settings:` tag but `settings:retention` as a game, so another kind of
  settings snapshot must not use the `settings:` prefix (ADR-0028).
  - `load_all`: reindex, one snapshot list, the newest per `settings:<slug>`, one file read
    each.
  - `save`: reindex, read the game's newest settings, replay this machine's changes on them,
    write a new snapshot (nothing when the result is unchanged), then forget the older settings
    snapshots of that game that it listed before writing, never one written since. An unchanged
    save still forgets the older ones. A failed forget does not fail the save; it is returned
    beside the result. Settings stage in `<stage root>/settings/<slug>`, which no game's
    `<stage root>/<slug>` can be, since every slug holds a hex hash.
- **When changes are saved:** a checkbox click is not written to the repository. The changes
  (`Change` list per title) wait in `pending-selection.json` in the config dir, one map per
  repository keyed like the keyring (ADR-0009: repository string and non-secret options), so
  switching repositories keeps each one's changes. The backup saves them before it backs up,
  then each saved game's changes leave the file. This makes one settings snapshot per backup,
  not one per click.

## Alternatives considered

- **In the local `config.yaml`:** simplest, but not shared between machines.
- **One settings snapshot for all games:** one read at load. But two machines saving at once
  each replace the whole file, so one can lose the other's changes to unrelated games.
- **Inside each game's backup snapshot:** no extra snapshots. But a skipped game never gets a
  snapshot to hold its setting, and a toggle would only reach the repository with that game's
  next changed backup.
- **Save on every click:** other machines see changes at once, but every click makes a
  snapshot and a reindex.

## Consequences

- Loading reads one file per game with settings, slower on remote repositories.
- Loading and each save reindex so they can read another machine's newer snapshot.
- Forgotten settings snapshots keep their data in the repository until a `rustic prune`. In an
  append-only repository nothing is forgotten; saves still work.
- "Newest" is by snapshot time, which is each machine's clock: a machine whose clock runs
  behind can have its newer settings read as older.
- Two machines saving the same game at once both read the same snapshot, so each write lacks the
  other's changes. After writing, `save` lists the game again; when a snapshot it did not read
  is there, it forgets its own and fails with `NotWritten`, so the machine keeps its pending
  changes and a retry merges them. Usually only the machine that lists later sees the other's
  snapshot; both do when their writes overlap, and both then retry.
- That check can still lose the other machine's changes in a narrow case: a crash, a failed
  listing or a failed forget of its own snapshot between the write and the withdrawal leaves
  the newest snapshot without the other's changes, and the machine that saw nothing wrong has
  dropped them. So can a backend whose listing lags behind writes (some rclone remotes): both
  machines can miss each other's snapshot, and one change is lost. An append-only repository always fails the forget (`Storage` error, the
  snapshot stays). Closing this needs a schema change (a revision in the snapshot to compare
  on write); not done.
- A corrupt or newer-format newest settings snapshot makes every save of that game fail until
  it is replaced or forgotten by hand.
- Changes reach other machines only after a backup on this one.
- Two titles with the same slug share one settings history.
- `apply` keeps the scan's `registry_source`, the first context holding any of the game's
  keys, while any key is left. If the excluded keys were the only ones that context holds, the
  backup exports nothing from it: the snapshot has no `registry.json`, and a game without
  files gets a snapshot with only `salvajocs.json`. The scan does not record which keys each
  context holds, so `apply` cannot tell.
- A game whose scan reported unreadable paths is staged without deletions (ADR-0004), so files
  excluded since keep being backed up until a scan reads everything.
