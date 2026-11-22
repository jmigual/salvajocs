# ADR-0015: Retention policy per game, forget after each backup, prune only after a forget

**Status:** Accepted
**Date:** 2026-11-18
**Amended:** 2026-11-21: the policy is saved on the next Back up, a policy changed on another PC
since is asked about, and the cleanup forgets nothing when this PC's rules could not be written
(ADR-0029).
**Amended:** 2026-11-22: a save that finds another machine's policy snapshot written meanwhile
forgets its own and fails with `NotWritten` (see Consequences).

## Context

ADR-0010 left retention out of v1, so snapshots accumulate. The user wants to keep the last N
backups of each game, every backup newer than a span, or both, and to forget and prune after each
backup.

With `skip_if_unchanged` (ADR-0010) a snapshot exists only when the game's files changed, so
"the last N backups" already counts changes. This holds per host: rustic picks the parent by host,
label and paths, so two machines with the same files each write their own snapshot.

## Decision

- **Policy:** `core::retention::Retention { keep_last, keep_within }`. Each rule is optional,
  and with both unset nothing is forgotten. `keep_within` is an amount of days, weeks, months
  (30 days) or years (365 days), with no calendar maths.
- **Stored in the repository:** one policy for all games, shared by every machine, in a snapshot
  at `/salvajocs-settings/retention` holding `retention.json` = `{ "format": 1, "policy": … }`,
  tagged `salvajocs-settings` and `settings:retention`, never `salvajocs` or `game:`.
  `retention::load` reindexes and reads the newest (none means off); `retention::save` writes a
  new one and forgets the older ones it read, as ADR-0014's `save` does. A higher `format` is an
  error like a parse failure. `selection::load_all` skips this snapshot: no game slug is
  `retention`, since every slug holds a hex hash.
- **Unknown fields are an error** (`deny_unknown_fields`), so an older app never applies only
  the rules it knows. A policy that fails to parse forgets nothing, and the caller reports the
  error instead of treating it as off.
- **Candidates:** snapshots tagged `salvajocs` and `game:<slug>` and not `salvajocs-settings`,
  by exact tag. Anything else in the repository is never forgotten.
- **Grouping:** per game across all hosts. Each game has two sequences, regular and
  `pre-restore`; a `pre-restore` snapshot never counts toward the regular `keep_last`.
- **Rules, within one sequence:** a snapshot is kept when any of these holds (union):
  - it is among the newest `keep_last`;
  - its time is at or after now minus `keep_within`;
  - it is the newest of its host. This also keeps the sequence's newest.
- `to_forget(policy, snapshots, keep, now)` returns the ids to forget. It is pure and has no
  rustic types. Ids in `keep` are never forgotten: the caller passes the snapshots the backup
  just made, which an older snapshot from a clock that ran ahead would otherwise outrank.
- **Prune** runs in the cleanup after a backup when its forget removed something, or when any
  index file lists packs marked for deletion (`Storage::has_marked_packs`, which reads the index
  files but no tree). Only a later prune frees a marked pack, or recovers one that a backup made
  since uses, so marks must not wait for the next forget. This holds also with every rule off. It
  uses rustic's default `PruneOptions` and never `instant_delete`. After a prune the session
  reindexes, so its next backup does not reuse data that is only in a marked pack.
- **Own rule function** instead of rustic's `KeepOptions`: the guards (newest per host,
  separate `pre-restore` sequence, `keep_within` from now) do not map onto it, and it keeps rustic
  types inside `core::storage` (ADR-0003).

## Alternatives considered

- **Policy in the local `config.yaml`:** each machine would forget by its own policy, and the
  strictest one would win.
- **`keep_within` from the newest snapshot (restic-style):** a game not played for a year would
  keep its whole last-year history. The user wants age measured from now.
- **Group per host and game:** every machine would keep its own N, so the total grows with the
  number of machines. **Group per game without a host guard:** a machine that plays less would
  lose all its snapshots to another machine's newer ones.
- **`pre-restore` never forgotten:** they would accumulate forever. **`pre-restore` only aged out
  by `keep_within`:** with only `keep_last` set they would never go.
- **Prune after every backup, or weekly:** every backup would pay a full prune on remote backends;
  a schedule needs state and a timer. After a forget, or while marks wait, is when there is
  something to free or recover.
- **Prune only after a forget:** a pack marked by one machine and then reused by another
  machine's backup (made from an index loaded before the mark) would stay unrecoverable until some
  later forget, with no upper bound, and marked space would never come back without one.
- **Read the marks from the prune plan:** building the plan walks every snapshot's trees, the
  full cost of a prune, on every backup. Reading the index files is enough to know whether a
  prune has anything to do.

## Consequences

- **Prune safety** (checked in rustic_core 0.13.0 source, `commands/prune.rs`): rustic takes no
  repository locks. Prune deletes in two phases: an unused pack is first marked, and removed only
  by a later prune once `keep_delete` (default 23 h) has passed. A marked pack that a snapshot
  needs again (a backup from another machine meanwhile) is recovered by the next prune.
  `instant_delete` would skip the marking. Until that next prune, such a snapshot cannot be
  restored; the cleanup after the backup that made it is that prune. Marked packs are freed by
  the first cleanup at least 23 h after the one that marked them.
- While marks wait, every backup's cleanup runs a full prune: for at least 23 h after each forget.
  With `keep_last` set and a game changing daily, some forget happens every day, so marks nearly
  always wait and nearly every backup prunes, close to the "prune after every backup" alternative.
  Without marks the check lists `index/` and reads the index files not cached yet; with marks it
  is a full prune plan. Measured in a debug test build on a local repository with 40 games of 5
  snapshots (200 index files): about 15 ms for the check, about 330 ms for the prune.
- A newer-format or corrupt stored policy means nothing is forgotten, and every save of the
  policy fails until another version replaces it or it is forgotten by hand.
- When the policy cannot be loaded for any reason, the cleanup after a backup applies the default
  (every rule off) and shows the load error as a warning: nothing is forgotten, but marked packs
  are still pruned.
- In an append-only repository the cleanup neither forgets nor prunes, since rustic refuses both
  (a refused prune would first plan it in full). The snapshots the policy would forget are
  counted, and the backup shows that count once as a warning. A server that enforces
  append-only itself (`rest-server --append-only`) leaves the repository config unmarked, so
  every cleanup with something to forget fails; turning the rules off avoids it.
- Two machines editing the policy at once each write their own; the one that then finds the
  other's snapshot forgets its own and fails with `NotWritten`, and its rules stay unsent.
  The narrow cases that can still lose a policy are those of ADR-0014.
- The Settings tab reloads the policy each time it is shown, unless an edit is still being
  saved, but a tab left open shows its own copy until then.
- `keep_last` ranks a game's snapshots across all hosts by time, so a machine whose clock runs
  far ahead can take all the `keep_last` places and the other machines keep only their newest
  snapshot each.
- `keep_within` counts from the local clock: a machine whose clock jumps far ahead forgets
  everything that `keep_last` and the newest-per-host guard do not keep.
- Clock skew also affects `keep_delete`: a pruning machine whose clock is about 23 h or more
  ahead can delete marked packs early, including one that another machine's in-flight backup
  reuses.
- Running `restic prune` while salvajocs may back up is unsafe: restic's lock does not stop
  salvajocs, and restic deletes unused packs at once, including ones a running backup is about to
  reference. The restic guide says so.
- Months and years are approximate (30 and 365 days).
- The Restore tab can also delete chosen backups by hand (`retention::delete`): only snapshots
  that are candidates here, any of a game's, then the same forget and prune, append-only
  handling and partial count as the cleanup. A game's settings snapshot stays.
