# ADR-0015: Retention policy per game, forget after each backup, prune only after a forget

**Status:** Accepted
**Date:** 2026-11-18

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
- **Stored in the repository:** one policy for all games, shared by every machine. The step that
  stores it decides how; whatever holds it must not carry the `salvajocs` tag.
- **Unknown fields are an error** (`deny_unknown_fields`), so an older app never applies only
  the rules it knows. A policy that fails to parse means no cleanup (forget nothing), and the
  caller reports the error instead of treating it as off.
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
- **Prune** runs only after a backup whose forget removed something, with rustic's default
  `PruneOptions` and never `instant_delete`.
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
  a schedule needs state and a timer. After a forget is when there is something to free.

## Consequences

- **Prune safety** (checked in rustic_core 0.13.0 source, `commands/prune.rs`): rustic takes no
  repository locks. Prune deletes in two phases: an unused pack is first marked, and removed only
  by a later prune once `keep_delete` (default 23 h) has passed. A marked pack that a snapshot
  needs again (a backup from another machine meanwhile) is recovered by the next prune.
  `instant_delete` would skip the marking. So marked packs are freed only by a later
  forget-triggered prune at least 23 h after the one that marked them; with no further forget,
  that space does not come back.
- `keep_within` counts from the local clock: a machine whose clock jumps far ahead forgets
  everything that `keep_last` and the newest-per-host guard do not keep.
- Running `restic prune` while salvajocs may back up is unsafe: restic's lock does not stop
  salvajocs, and restic deletes unused packs at once, including ones a running backup is about to
  reference. The restic guide says so.
- Months and years are approximate (30 and 365 days).
