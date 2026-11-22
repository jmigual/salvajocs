# ADR-0019: Restore several games with their defaults, one safety backup each

**Status:** Accepted
**Date:** 2026-11-19

## Context

The Restore tab restored one game at a time: choose the game, its snapshot and targets, preview,
restore. A new machine, or a reinstalled one, needs every game back, and that took the same
steps for each game. The user wants to tick several games, check one plan and press Restore once,
like ludusavi, whose plan lists each game's files labelled new or changed.

The rules a single restore follows still hold for each game of a batch: a plan is shown before
anything is written, and nothing is written for a game without its safety backup (ADR-0005); one
snapshot per game (ADR-0010) and `pre-restore` snapshots as their own retention sequence
(ADR-0015); unknown folders of the other OS's build go through the mapping dialog (ADR-0011).

## Decision

- **Defaults only.** Each ticked game is restored from its default snapshot (the newest one that
  is not `pre-restore`, ADR-0010) into `restore::default_targets`. A game with only `pre-restore`
  snapshots cannot be ticked. The batch has no snapshot, target or per-file override choices;
  restoring the game on its own stays the way to make them.
- **One planning job.** It detects the environment once, then per ticked game, in list order,
  reads the snapshot's listing, plans it with `restore::prepare` and compares it (ADR-0020); it
  writes nothing to disk. The run downloads every planned game into `restore-batch/<pid>/<index>`
  under the cache dir before the first restore, so no download waits for a reindex after a
  safety backup (ADR-0003), and removes them after the last one. Before downloading, the run
  also removes the dirs of other process ids untouched for a day; the live batch of another
  running salvajocs is kept. Cancel, or Settings replacing the repository, has nothing on disk
  to remove.
- **Mapping dialogs one after another.** A game that needs mapping gets the existing dialog,
  without the prefix option (choosing a prefix needs the single-game view). Confirmed choices are
  saved with the game as in a single restore, and stay saved if the batch is cancelled later.
  **Skip this game** leaves the game out and saves nothing, unlike a skipped folder, which is
  saved as a skip. **Cancel** drops the whole batch; nothing is written.
- **Compare by content, at planning time.** `restore::compare_writes` labels each write `New`
  (nothing at the target), `Changed` or `Unchanged`, comparing the staged source with the target
  through `core::changes::same_content` (sizes, then bytes; both files are local). A write the
  restore would refuse (symlinked target, a directory there, a source that is not a file) carries
  that error. An existing target file that cannot be opened returns `UnreadableTarget`: the
  safety backup could not copy it, so `restore::run` would write nothing for that game. The
  labels are a snapshot in time; `run` does not use them, and unchanged files are still written.
  ADR-0020 replaces the byte compare with the hash list, since the source is no longer on disk.
- **The confirm.** Per game: title and snapshot, targets, `N new, M changed, K unchanged`, each
  write that will fail with its reason, the cross-build warning and notes, the registry line and
  the skipped items, as the single preview has them. Below the counts and the writes that will
  fail, a read-only file tree (the Backup tab's, ADR-0018) of the files the game writes, keyed by
  where each lands (`restore::staged_target`, the key the safety backup stages it under: the
  target side, so a cross-build restore shows this build's folders), with the badges `new`,
  `changed` and `will fail`; unchanged files have none. A folder's open button shows only when
  the folder exists. A game that will not be restored is listed with its reason: no regular
  snapshot, a failed fetch or plan, an unreadable target, or skipped in the dialog.
  Amended by ADR-0025: the tree ticks each file, folder and registry root, and an unreadable
  target blocks the game only while it is ticked.
- **Run: `restore::run` once per game**, in one job, in list order. Each game gets its own
  `pre-restore` safety backup and its own result. A game that fails, including a refused safety
  backup or a running wineserver for a restore of registry keys into a prefix, writes nothing
  and the next game goes on (the wineserver is checked again after the safety backup). The result
  shows `Restored N of M games` and each game's lines,
  built by the function the single restore uses. Afterwards the backups are listed again and the
  ticks cleared.

## Alternatives considered

- **One combined safety snapshot for all games:** one reindex instead of N, but it breaks one
  snapshot per game (ADR-0010), mixes games in the `pre-restore` retention sequence (ADR-0015)
  and the undo steps of the guide, which pick a game's own `pre-restore` snapshot.
- **One `backup_games` call for every safety backup before the first write:** keeps one snapshot
  per game with one reindex. Deferred: it needs a core `run_games` that splits `restore::run`
  into back up all, then write all; it is the follow-up if N reindexes are slow on remote
  backends.
- **Stepping through each game's full preview:** the one-game-at-a-time flow again.
- **Hashing both sides, or comparing with the snapshot's chunk ids or hash list:** reads more than
  a size check plus one byte compare of two local files, and the staged source is already on disk.
  ADR-0020 reverses this: the staged source is no longer downloaded before the confirm, so the
  hash list is what is compared.
- **Labels from the snapshot side (`Base/tail` of the backup):** a cross-build restore would show
  the other build's folders, not where the files go.

## Consequences

- Each game's restore reindexes once (ADR-0003 counts each safety backup as a run): N reopens
  for N games.
- During the run, every planned game's snapshot is on disk at once until the run ends.
- Comparing reads only the targets: by size and mtime, and a target's SHA-256 against the
  snapshot's hash list only when the size matches and the mtime differs (ADR-0020). A target
  changed between the confirm and the run is still saved by its safety backup first, but its
  label is out of date.
