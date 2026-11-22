# ADR-0028: Custom games stored in the repository, one settings snapshot per game

**Status:** Accepted
**Date:** 2026-11-20
**Amended:** 2026-11-21: custom games are written to the repository only by Back up, read on open
and from a Read repository button; a migration asks instead of the repository winning; offline
editing; no `config-before-custom-games.yaml` (ADR-0029).
**Amended:** 2026-11-22: a save that finds another PC's snapshot written meanwhile forgets its
own and fails with `NotWritten` (see Consequences).
**Amended:** 2026-11-22: a read never adds or replaces a local game; the repository's new or
changed games are offered and used only once accepted on each PC (ADR-0030).

## Context

The user added a custom game on one PC and did not see it on another PC that uses the same
repository. Custom games lived only in each PC's `config.yaml` (ADR-0008, ADR-0009). Every PC
that opens a repository should see the same custom games: one added, edited or removed on one
PC shows on another the next time that PC opens the repository or the Custom games tab.

What the code allows:

- `selection::load_all` (the same in 0.4.0 to 0.7.0) lists every `salvajocs-settings`
  snapshot, skips only the exact tag `settings:retention`, strips `settings:` from any other tag
  and reads `/salvajocs-settings/<rest>/settings.json`. A custom game tagged
  `settings:custom:<slug>` would fail that read, and the error, filed under the snapshot label
  (the game's name), could replace that game's real selection on an old PC.
- A tag that does not start with `settings:` is skipped by that code, and by the restore list,
  the backup baseline and retention, which match the exact tags `salvajocs` and `game:<slug>`
  (rustic's tag match is exact).
- A slug ends in 8 hex digits (ADR-0010), so no slug is `custom-games`.
- `AppConfig` keeps no unknown fields, and `GameEntry` drops unknown fields, so a stored
  `CustomGame` read by an older version would lose a newer version's fields when rewritten.
- The Backup tab scans without an open repository, so the PC needs a local copy to scan.

## Decision

- **Storage:** one settings snapshot per custom game, as ADR-0014 does for selections.

  | What | Value |
  | --- | --- |
  | Tags | `salvajocs-settings` and `custom-game:<slug>`, `<slug>` = `slug(name)` |
  | `as_path` | `/salvajocs-settings/custom-games/<slug>` |
  | File | `custom-game.json` |
  | Label | the game's name |
  | Staged in | `<stage root>/settings/custom-games/<slug>` |

  `custom-game.json` is `{ "format": 1, "name", "integration", "files", "entry", "pairs",
  "skips" }`, empty fields left out. A removed game is a tombstone,
  `{ "format": 1, "name": …, "removed": true }`.
  - A `format` above this version's is an error: the game is neither applied nor rewritten (no
    edit, no remove). The PC keeps scanning with its local copy, if it has one.
  - Unknown top-level fields are kept on rewrite. They come from the newest snapshot the save
    reads, never from the caller, and a rename does not carry them to the new name. A field
    added inside `files`, `entry`, `pairs` or `skips` bumps `format`, since `GameEntry` drops
    unknown fields.
  - The `settings:` tag prefix stays reserved for game selections (ADR-0014).
- **Shared and local fields:** `name`, `integration`, `files`, `entry`, `pairs` and `skips`
  are shared. `wine_prefix` is never stored: it is an absolute host path that holds a home
  folder and means nothing on another PC. It stays in `config.yaml`, matched by name.
- **`config.yaml`:** `custom_games` keeps its schema and becomes this PC's copy of the
  repository's custom games plus each game's `wine_prefix`. A new optional field,
  `custom_games_repository`, holds the repository key (`AppConfig::secret_scope`) whose custom
  games that copy mirrors; it is left out when unset.
- **Core API (`core::custom_games`):** `load_all`, `save`, `save_choices`, the pure `merge` and
  `sync` (load, merge, upload).
  - `load_all` reindexes, lists `salvajocs-settings`, keeps tags starting `custom-game:`, and
    reads the newest snapshot per slug: a game, removed, or an error, by name.
  - `save(name, based_on, new)` reindexes and reads the game's newest snapshot. When its shared
    fields differ from `based_on`, the copy the edit started from, it fails with `Changed`,
    carrying the newest copy, and writes nothing; a name with no snapshot at all (its upload
    failed) never fails so. Otherwise it writes `new` (or a tombstone), nothing when unchanged
    or when removing a name with no snapshot, then forgets the older snapshots it listed before
    writing (as ADR-0014). A failed forget is a warning.
  - `save_choices` applies the restore mapping choices (ADR-0011) on the newest repository copy,
    so they never overwrite another PC's newer rows; on this PC's copy when the repository has
    no snapshot of the game at all. `crossbuild::apply_choices` therefore takes the custom game
    list, not the whole config.
- **When saved:** at once, like the retention policy (ADR-0015): **Save game** and **Remove** in
  the Custom games tab, and a mapping dialog's choices (applied in memory first, as before).
  Rename = write the new name, then a tombstone for the old one; if the second write fails,
  both names exist and the form says so.
- **When read:** when the repository opens, after another repository is connected in Settings,
  and each time the Custom games tab is shown, unless a save runs.
- **Merge into the local copy:** keep the local order; take each game's shared fields from the
  repository and its `wine_prefix` from the local copy; drop removed games; append new games by
  name. An unreadable game keeps its local copy, which is scanned; the tab shows the error and
  disables Edit and Remove for it.
- **Mirror or migrate:** when `custom_games_repository` equals the open repository's key, the
  repository is the source of truth and a local game it does not know is dropped. Otherwise
  (absent after the upgrade, or another repository):
  - a local game whose name has no snapshot at all is uploaded;
  - a local game the repository also has with other contents takes the repository's version
    (**the repository wins**); a notice names each, saying only that this PC's copy differed;
  - a local game with a tombstone is removed here; a notice says how many and names them, so
    it never vanishes silently.
  The marker is set only when every upload succeeded and no local game's repository copy was
  unreadable; otherwise it is left unset, so the next sync migrates again and a failed upload is
  retried, never dropped. Before the first rewrite that replaced or removed a local game,
  `config.yaml` is copied to `config-before-custom-games.yaml` in the config dir, once: an
  existing copy is never overwritten.
- **Offline:** without an open repository the tab lists the games read-only ("Open the
  repository to change custom games"). Backups and restores need the repository anyway.
- **Another repository (Settings):** this PC's custom games go along. The new repository gets
  the names it does not have and keeps its own version of the others; the old repository keeps
  its snapshots.

## Alternatives considered

- **One snapshot for the whole list:** one read, but two PCs editing different games would each
  rewrite the list, and the newer write drops the other's game (rejected in ADR-0014 too).
- **Only in `config.yaml`, with export and import:** a manual step on every PC, and the user
  asked for sharing.
- **Tag `settings:custom:<slug>`:** salvajocs 0.4.0 to 0.7.0 would misread it as a game
  selection (see Context).
- **Edit offline and send later** (`pending-custom-games.json`): a second pending file and replay
  rules for a game changed elsewhere meanwhile, for edits that only matter once the repository
  is back.
- **This PC wins on the first open, or a merge of both versions:** the last PC to upgrade would
  overwrite the others' definitions; a merge is ambiguous when `integration` differs.
- **Only the new repository's games after a switch:** moving to a new repository would lose the
  games.
- **Delete by forget instead of a tombstone:** fails in append-only repositories, and a PC with
  an old copy would upload the game again.

## Consequences

- Loading reads one file per custom game; slower on remote repositories.
- "Newest" is by each PC's clock (as ADR-0014). Two PCs saving the same game at the same moment
  write from the same snapshot. After writing, a save lists the game again and, when a snapshot
  it did not read is there, forgets its own and fails with `NotWritten`; the change stays unsent.
  Usually only the PC that lists later does, and its next Back up gets the conflict dialog; when
  the writes overlap both do, and the PC that retries first just sends again while the other
  then gets the conflict dialog.
- That check can still lose the other PC's edit in a narrow case: a crash, a failed listing or
  a failed forget of its own snapshot between the write and the withdrawal leaves the newest
  snapshot without the other PC's edit, which that PC has dropped as saved. An append-only
  repository always fails the forget (`Storage` error, the snapshot stays). Closing this needs a
  schema change; not done.
- Forgotten snapshots keep their data until a prune; an append-only repository keeps them all,
  which is harmless. The newest tombstone of a name is never forgotten.
- Removing the newest custom game snapshot with restic brings back the previous version, or
  drops the game when it was the only one.
- salvajocs 0.7.0 and older never read these snapshots: they keep their own list, and a game
  added on a newer PC does not show there. They do not misread, list or forget them either.
  When such a PC upgrades, its first open migrates its list. A downgrade that rewrites
  `config.yaml` drops the marker, so the next upgrade migrates again.
- Editing a non-secret option of the repository changes its key (ADR-0009), so the next open
  migrates again: a game another PC changed since the last load differs from the local copy and
  is reported, and the one-time config copy is taken then if it was never taken. No local-only
  game is lost.
- A save needs the repository: without it the tab is read-only.
- The Backup tab may scan before the first load ends, so a game another PC added shows in the
  backup after the next scan.
