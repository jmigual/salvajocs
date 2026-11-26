# ADR-0036: A restore can delete the files its backup lacks

**Status:** Accepted (the user's OK, 2026-11-25)
**Date:** 2026-11-25
**Approved:** 2026-11-25 by the user, as required for a change to what is deleted (CLAUDE.md):
the deletion is optional and remembered per game in `config.yaml`; its rows start ticked; only
the folders in the backup are mirrored; the unticks are recorded per backup in `hashes.json`;
both schema additions are approved; it ships in the same PR as the restore fixes. Also on
2026-11-25: a game unticked as a whole ("Delete as usual") still deletes, and the behaviour must
stay predictable and simple: do what is expected, no heuristics, the safety backup is the undo.
Also on 2026-11-25: the batch confirm has one Select all button that turns the box on for every
game in it, remembered per game like the box, and a Deselect all button that turns them all off
(the user's OK: "Keep it").
**Amends:** ADR-0005 (a restore can delete), ADR-0014 (a backup records its rules), ADR-0018
(a `hashes.json` field), ADR-0019 and ADR-0025 (deletion rows and ticks), ADR-0035 (the effect
on the base).

## Context

A restore only adds and overwrites (ADR-0005). Games that rotate 4 or 5 auto-saves gain extra
files on every round trip between PCs: PC A restores B's backup, keeps its own older auto-saves
next to B's, and its next Back up stores the mix as Ahead (ADR-0035), which spreads them to the
other PCs. The user asked: "upon restoring we can choose to delete everything that's not in the
backup", with a checkbox per file, and "If I unticked a file in a backup then don't delete it
from another pc when restoring".

## Decision

### What is offered for deletion

With the box "Delete files the backup does not have" on, the plan offers a file on this PC for
deletion when all of these hold:

1. It is a regular file (not a folder, a symlink or a special file) inside a folder the backup
   restores to: the target folder of one of the backup's roots that the plan restores, mapped
   or not. Folders the backup has no root for are left alone.
2. That folder is not a whole base such as the home folder, `AppData` or a bare drive (`C:`).
   The game's install folder (`<base>`, a `GameDir` root with an empty tail) is one: it also
   holds the game's own files, so a root that is the install folder never deletes.
3. The game's manifest entry or custom game covers the file, as it would cover a file of the
   backup at that place: a `*.sav` template never deletes `notes.txt`. Restoring a backup that
   held the file would write it to exactly this path.
4. The backup does not hold the file, and no write of the plan targets it. Both comparisons
   ignore ASCII case where paths do (Windows folders, and any case-insensitive file system).
5. Neither the backup's recorded selection nor today's selection excludes it (below).

`restore::deletions` computes this list in core, once, from the snapshot and the plan as
`plan` made it. Unticking a row, a per-file override or a folder override changes only what is
deleted from that list, never what is on it:

- an unticked row, or a row below an unticked folder, is not deleted;
- a row below a folder override is dropped (the override sends that folder's writes elsewhere);
- a file that a ticked write targets after an override (per file or per folder) is not
  deleted: the plan marks its row as kept and leaves it out of the count, and `restore::run`
  refuses it anyway, so a restore never deletes what it writes.

If a folder cannot be listed, nothing in it is offered and the plan says so: a read error never
removes saves (ADR-0004).

### Protection by the selection

A file is kept, and not listed, when either selection excludes it, through
`GameSettings::path_included`:

- the backup's recorded selection (`Hashes.selection`), checked on the file's path as the
  backup would hold it. An older backup has none.
- today's selection (the repository settings with this PC's pending changes), checked on the
  file's path as the backup would hold it and as this PC holds it, since the shared selection
  holds both builds' paths.

If today's settings cannot be read, nothing is deleted. The restore reads them again just before
it runs, for each game of a batch afresh, and deletes only the ticked files `restore::deletions`
still offers, so a file unticked on the Backup tab after the plan was made is kept, with a
result line saying so. Restore files waits while the Backup tab still writes its pending
changes. If they cannot be read then, or the Backup tab's last write of them failed, the restore
deletes nothing and a failed result line says so; like a failed write, it records no base.
Unticking the whole game
(`GameSettings::skip`) only stops its backups: a restore still offers its deletions, and the user
unticks rows to keep them (user decision 2026-11-25).

### `Hashes.selection` in `hashes.json`

- The game's `GameSettings.paths` rules as the backup used them, as `{ "<path key>": bool }`
  with the same meaning (the deepest saved folder or file wins). It holds rules, not a list of
  excluded files, so an unticked folder also protects files the backing-up PC did not have.
- Written by Back up (`selection::apply` carries it in `GameScan.selection`, staging writes it).
  Left out when empty, so the bytes of a game without unticks do not change.
- `HASHES_FORMAT` stays 1: an older version reads the list and ignores the field.
- Outside `Hashes::same_content` and outside `salvajocs.json`, so the meta hash, the skip rule
  and the sync states (ADR-0035) do not change. A change of the rules alone makes no new
  snapshot.
- The `pre-restore` safety backup records none.
- The cross-build `Mapper` copies it unchanged; the restore checks it on the backup's own
  paths, so a mapped restore needs no translation.

### `AppConfig.restore_deletes` in `config.yaml`

The titles of the games whose box was last left on. Off for every game by default. A malformed
value reads as empty; left out when empty. A per-PC preference keyed by title, written only
through `State::save_config`.

### UI

- A box per game, in the single-game plan above the tree and in each game's block of the batch
  confirm, read from and saved to `restore_deletes`. It is disabled, with the reason on hover,
  for a `pre-restore` snapshot, while the settings cannot be read, and when nothing would be
  deleted.
- Above the games of the batch confirm, **Select all** turns the box on for every planned game
  and **Deselect all** turns it off, each saved to `restore_deletes` like a click on each box.
- The files to delete are rows in the same tree after the writes, with a "will be deleted" badge
  (`Badge::Deletes`), ticked at first, shown only while the box is on. A folder tick covers both
  writes and deletions below it. A folder that holds only deletions has no Override button.
- While a deletion is ticked, a line above Restore files says how many files will be deleted and
  that the safety backup keeps them.
- Each deletion has a result line, "deleted" or "failed: …". A failed deletion blocks the base
  record like a failed write.

### What `restore::run` does

1. It backs up each file to delete in the safety backup, with the files the restore overwrites.
   No safety backup, no deletion. A file to delete that cannot be read blocks the whole restore,
   as an unreadable write target does.
2. It writes the files and the registry.
3. It deletes each file that was in the safety backup and is unchanged since: the same size and
   modification time as before the safety backup. A file changed or created after the safety
   backup is kept (it has no copy). Folders are never removed. The registry restore is unchanged
   (it already replaces whole subtrees, ADR-0006 and ADR-0026).

### Refused

- A `pre-restore` snapshot: it holds only what a restore touched, not the whole folder.
- Unreadable settings.
- A whole-base, undeclared, too-wide, skipped or unmapped root, and a folder that cannot be
  listed.
- A file the backup holds, a file a write targets (also after an override), and anything below
  a folder override.
- A file changed or created after the safety backup, a folder, a symlink or a special file.

## Invariants

- **I1:** every deleted file is in the safety backup made by the same restore, as it was
  deleted.
- **I2:** nothing outside the offered list is touched: ticks, template globs, both selections
  and overrides all hold.
- **I3:** restoring the safety backup with everything ticked brings each deleted file back byte
  for byte.

## Alternatives considered

- **Today's selection only:** an untick made before the backup and re-ticked since would not be
  seen, so the other PC would delete a file the user excluded at the time.
- **The rules in `salvajocs.json`:** they would change the meta hash and so the sync states.
- **Bumping `HASHES_FORMAT`:** older versions would read the list as missing and lose the skip
  rule and the base comparisons.
- **A true mirror of every template folder:** it would delete in folders the backup never had a
  file of, which the user did not ask for.
- **An option that is not remembered:** the user restores the same games the same way each time.
- **An unticked game deletes nothing:** a special case the user rejected; the rows already let
  them keep any file.
- **Mirror-restoring a `pre-restore` snapshot to undo the files a restore added:** the safety
  backup holds only what the restore overwrote or deleted, not the whole folder, so it would
  delete saves.

## Consequences

- After a deleting restore with nothing kept, this PC's saves equal the restored snapshot, so
  the base it records reads Up to date instead of Ahead, and the extra auto-saves stop
  spreading. Kept files read Ahead: the next Back up stores the mix (ADR-0035).
- A snapshot made by an older version, or re-staged by one, is protected by today's selection
  only.
- A change of the rules alone makes no new snapshot; today's selection still protects the files.
  A re-tick with no content change makes no snapshot either, so the newest backup's recorded
  untick keeps those files out of deletion until the next backup with a content change. This
  errs toward keeping.
- An older version that rewrites `config.yaml` drops `restore_deletes`; the box shows off again.
- A file changed between the safety backup and its deletion is kept and reported as failed.
- Empty folders remain. Files a restore added are not removed by restoring its safety backup.
