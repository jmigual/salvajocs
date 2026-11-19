# ADR-0010: One snapshot per game per backup, found by tags, titled by label; all hosts visible

**Status:** Accepted
**Date:** 2026-11-16

## Context

The restore list needs, per game, the available backups from every machine that shares the
repository. rustic_core snapshots carry `hostname`, `paths`, `tags` and a free-text `label`
(ADR-0003). `SnapshotOptions::add_tags` splits its input on commas, so a tag cannot contain one.
Game titles can contain commas, colons and non-ASCII characters. In the ADR-0003 spike,
`label = "Title, with comma: ünï"` was stored unchanged. Parent detection groups by host, label
and paths by default.

## Decision

- Each backup run makes **one snapshot per game**, with `as_path = /salvajocs/<slug>`
  (ADR-0004) and `label` = the exact title.
- Tags: `salvajocs` and `game:<slug>`. The slug is the title lowercased, with ASCII alphanumeric
  runs joined by `-`, truncated to 40 characters, plus `-` and 8 hex digits of FNV-1a 32 over the
  exact title. A `-` left at the end by the cut is dropped, and a title without ASCII
  alphanumerics gives the 8 hex digits alone. FNV is a few lines of code rather than a
  dependency. The slug must be stable forever: a unit test pins known title→slug pairs.
- A safety backup made before a restore (ADR-0005) gets an extra `pre-restore` tag.
- **Restore list:** one `get_matching_snapshots` call filtered on the `salvajocs` tag, grouped by
  the `game:` tag. Each row shows the title from `label`. Each game lists all snapshots from all
  hosts, newest first, with host and time. `pre-restore` snapshots are listed with a
  "pre-restore" label but never chosen as the default. Otherwise, after a restore on machine A,
  every other machine would default to A's overwritten old state. The default is the newest
  snapshot without that tag.
- The host is rustic's default (the machine hostname). It is shown, never filtered on, because
  cross-machine restore is the point.
- `skip_if_unchanged` is set, so unchanged games don't add snapshots. The repository is reindexed
  once per backup run (ADR-0003, index freshness).
  - Amended by ADR-0018: a game is also skipped when its staged hash list equals that of the
    newest non-`pre-restore` snapshot of the game from any host. The pre-restore safety backup
    keeps only `skip_if_unchanged`.

## Alternatives considered

- **One snapshot per backup run with all games:** one backup call instead of N. But finding the
  games a snapshot holds needs an `ls` per snapshot or a tag per game. Every game would also
  share one history, so restoring game A means picking through runs that didn't touch it.
  Revisit if step 12's timing shows N per-game backups are too slow on remote repos.
- **Title in a tag:** commas are split and length is unbounded. `label` holds it instead.
- **Group by slug only, no label:** a custom game from another machine would then show as a bare
  slug.

## Consequences

- A full backup of N games is one index load plus N backup calls. They run one after another,
  which keeps progress reporting simple; save sets are small.
  - Measured in plan step 12 (release build, 2026-11-16, local repo on tmpfs): one reindex plus
    20 per-game backups of one 100 kB file each takes 0.32-0.38 s when all 20 are new and
    0.29 s when all are unchanged and skipped.
- Retention (`forget`/`prune`) was out of scope for v1, so snapshots accumulated; see ADR-0015.
