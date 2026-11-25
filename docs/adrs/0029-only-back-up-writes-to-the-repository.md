# ADR-0029: Only Back up writes custom games and the Old backups rules to the repository

**Status:** Accepted
**Date:** 2026-11-21
**Amended:** 2026-11-22: a mirror keeps the local game and offers a different repository copy;
the repository's games this PC lacks are offered, not appended; **Keep the repository's** is an
acceptance (ADR-0030).
**Amended:** 2026-11-25: the Back up dialog also asks about each game changed here and on
another PC since this PC last synced, with **Don't back up** preselected (ADR-0035).

## Context

In 0.8.0 (ADR-0028) the custom games reach the repository as soon as the app opens it: the first
open uploads this PC's games, and each **Save game**, **Remove** and restore mapping choice
writes a settings snapshot at once. The Old backups rules are saved on each tick (ADR-0015).
The user found a custom game synced the moment the app opened, and wants the repository written
only when **Back up** is pressed: writing all the time is a can of worms of sync issues.

The user decided:

- the custom games are read when the repository opens and from a **Read repository** button;
- **Back up** writes every custom game changed here;
- a game changed here and, since then, on another PC is asked about on Back up, with both
  versions shown, and the user picks one;
- the Old backups rules also wait for Back up, and a conflict on them is asked in the same dialog;
- Back up with no game ticked but unsent changes is enabled and sends only those changes;
- Cancel in that dialog cancels the whole Back up, and nothing is written.

What the code allows:

- `custom_games::save(name, based_on, new)` already refuses to write when the repository's
  newest copy is not `based_on` (`CustomGameError::Changed`), so a content base is enough to
  detect a conflict, also for an edit made offline.
- `State::save_config` is the only config writer, and it writes the whole file atomically.
- Opening the app, the pre-restore safety backup (ADR-0005), **Delete backups**, creating a
  repository and **Apply compression** are explicit actions or reads, not syncs of settings.

## Decision

- **Writers.** Only Back up writes custom games and the rules. A read, an edit, a remove, a
  rename, a mapping dialog, a tick of an Old backups rule and a Settings repository switch write
  nothing to the repository. These still write: Back up, **Delete backups**, the pre-restore
  safety backup (it sends no unsent setting), creating a repository and **Apply compression**.
- **Unsent changes in `config.yaml`.** Two optional top-level fields, left out when empty:

  ```yaml
  custom_games_unsent:   # names changed here since the last Back up, in order
    - name: Foo          # edited here: based_on is the copy before the first change,
      based_on: { ... }  #   without wine_prefix
    - name: Bar          # added here: no based_on
    - name: Old name     # not in custom_games: removed here (or renamed away)
      based_on: { ... }
  retention_unsent:      # the rules changed here since the last Back up
    policy: { keep_last: 10 }
    based_on: {}         # the rules this PC had loaded when the change started; {} = off
  ```

  - A removal is an entry whose name is not in `custom_games`, never a flag, so the two cannot
    disagree.
  - The earliest base wins: a second change to a name with an entry keeps its `based_on`. An
    entry stays until a Back up clears it, also when the game is edited back to its base.
  - Both live in `config.yaml`, not in a pending file as ADR-0014's selection does, so the
    local copy and its unsent mark are one atomic write: a crash cannot leave an edit without
    its mark, which the next read would overwrite.
  - `retention_unsent` reads leniently: a value that does not parse reads as none, so a newer
    version's rule never makes the config unreadable.
- **Marker renamed.** The repository key whose custom games `custom_games` mirrors is written as
  `custom_games_read_from`; the old name `custom_games_repository` is still read. A 0.8.0
  downgrade then sees no marker and migrates, so its mirror never silently drops a game added
  here and not sent yet.
- **Reads.** On repository open, after a Settings switch, and from **Read repository** in the
  Custom games tab (disabled without an open repository or while a read runs). The tab no longer
  reads each time it is shown. The job only lists (`load_all`); the GUI thread merges into the
  current config, so an edit made while the read ran is kept. A read that started before a Back
  up ended is dropped and run again.
- **Merge (`custom_games::merge`).** A name with an unsent entry stays as this PC has it, a
  removal included. Every other local game:
  - **mirror** (the marker is the open repository's key): takes the repository's shared fields
    and keeps its `wine_prefix` (amended by ADR-0030: keeps the local game and offers a different
    repository copy); a tombstone drops it; an unreadable copy keeps the local one. A
    name the repository has no snapshot of stays, with an entry without a base, so a copy wrongly
    marked as mirroring (0.8.0 left the marker after a failed upload) loses nothing.
  - **migrate** (no marker, or another repository's): a copy equal to the repository's changes
    nothing. No snapshot, another copy or a tombstone keeps the local game and adds an entry
    based on this PC's own copy, so Back up uploads it or asks. An unreadable copy keeps the
    local game and leaves the marker unset.
  - Repository games this PC lacks and has no entry for are appended by name (amended by
    ADR-0030: offered, used only once accepted). The marker is set
    once a read compared every local game.
- **Back up.** When anything is unsent, a check job reads the newest copy of each unsent name and
  the stored rules, writing nothing. An entry conflicts when the repository has a snapshot of
  the name and its newest copy is neither `based_on` nor what this PC sends; a name with no
  snapshot never does, and neither does a game added and removed here (no base, not local),
  which Back up clears without a write. The rules conflict when the stored ones are neither
  `based_on` nor this PC's. Then:
  - with a conflict, a dialog "Changed on another PC" shows each version side by side, **This
    PC** and **The repository**, with **Keep this PC's** and **Keep the repository's** per
    block. **Back up** applies the choices to the config and starts the backup; **Cancel** ends
    the Back up and writes nothing. Keeping this PC's rebases the entry on the repository's copy;
    keeping the repository's puts that copy here (with this PC's `wine_prefix`), or drops the
    game for a tombstone, and clears the entry.
  - the backup starts, like a Back up without unsent changes, only once no write of
    `pending-selection.json` runs. It sends the custom games (`custom_games::send`), then the
    rules, then saves the selection, scans, backs up and cleans up as before. The rules are
    checked again just before they are written: rules another PC changed since the check, or
    rules of a newer format, are not overwritten and stay unsent. The next Back up asks again
    about the former and reports the latter again; the Old backups section can discard this
    PC's unsent rules. A game write that fails
    keeps its entry and is reported; the game backups still run. Removals are written after the
    games, and are held back when a game write of the same send failed for a reason that may
    pass (repository, I/O, not written), so a rename never leaves the game missing on other PCs.
  - when the rules could not be written, the cleanup forgets nothing, since the stored rules may
    be stricter than this PC's; marked packs are still pruned.
  - with no game ticked, Back up sends only the custom games and the rules.
  - when the job ends and the session is still on the same repository, each sent entry is
    cleared, or rebased on what was sent when the game changed during the backup.
- **Offline editing.** Edits no longer need the repository: they change `custom_games` and add
  an entry. Only **Read repository** needs it.
- **Another repository (Settings):** the custom games and the unsent entries go along. The first
  read of the new repository migrates; Back up then uploads or asks.
- **`config-before-custom-games.yaml` is no longer written**: a read never replaces or removes a
  game changed here, and a migration keeps every local game. A copy 0.8.0 left stays on disk.

### Amends ADR-0028

- *When saved:* on Back up, not at once. Rename = write the new name, then the old name's
  tombstone, both on Back up; the tombstone waits when the new name's write failed.
- *When read:* on open, after a switch and from **Read repository**; not on each show of the tab.
- *Mirror or migrate:* a migration keeps every local game and asks at Back up, instead of "the
  repository wins" with a notice.
- *Offline:* the tab is editable; changes wait for a Back up.
- *Another repository:* the new repository is asked about at Back up instead of keeping its own
  version silently.
- The `config-before-custom-games.yaml` copy is dropped.
- The rejected alternative "Edit offline and send later" is now the decision, kept in
  `config.yaml` rather than in a second pending file.

### Amends ADR-0015

- *Stored in the repository:* the policy is saved on the next Back up, not when edited.
- *The last save wins* becomes a question: rules changed here and, since then, on another PC are
  asked about at Back up.
- The cleanup forgets nothing when the Back up could not write this PC's rules.

## Alternatives considered

- **Keep syncing on open and on each edit (0.8.0):** what the user asked to stop.
- **A pending file like `pending-selection.json` (ADR-0014):** a crash between the config write
  and the pending file write could leave an edit with no unsent mark, which the next read
  overwrites.
- **A snapshot id as the base instead of the content:** needs a per-game record of the last
  read, and does not cover an edit made offline; the content base reuses `save`'s check.
- **Cancel backs up everything but the conflicting games:** the version chosen can change what
  the scan finds, and the user is right there to choose.
- **This PC's rules win silently (ADR-0015's last save wins):** one more block in the same dialog
  keeps "nothing changes without your choice" for every setting.
- **Keep Back up disabled with no game ticked:** a PC with nothing to back up could never share a
  custom game it just added.
- **Reuse the `custom_games_repository` name:** the smallest schema, but a 0.8.0 downgrade would
  mirror and drop every unsent added game.

## Consequences

- Other PCs see a change made here only after a Back up on this PC.
- PCs still on 0.8.0 that share the repository keep writing on every edit; Back up's check covers
  those writes like any other PC's.
- A Back up with unsent changes reads the repository once more before it starts.
- salvajocs 0.8.0 and 0.7 drop the two new fields on their next rewrite of `config.yaml`, losing
  the unsent marks (not the games). 0.8.0 then migrates on its next open: its "repository wins"
  may replace a game edited here, and it brings back a game removed here and not yet sent.
- An edit of a game whose newest copy is unreadable (newer format or corrupt) is not possible
  from the tab, as before; offline nothing is known, so such an edit fails at Back up and stays
  unsent.
