# ADR-0034: Retention keeps the last N of each game across all PCs

**Status:** Accepted
**Date:** 2026-11-25
**Approved:** 2026-11-25 by the user, as required for a change to what is deleted (CLAUDE.md).
**Amends:** ADR-0015 (the "newest of its host" rule is dropped) and the consequence of ADR-0018
that retention keeps each host's newest.

## Context

ADR-0015 ranks a game's snapshots across all hosts but also keeps the newest snapshot of each
host, so that a machine that plays less does not lose all its snapshots to another machine's
newer ones. The same holds for the `pre-restore` sequence.

The user rejected this: "the cleanup rules for old backups always keep the newest backup from
each PC is a very stupid rule ... What happens if I stop using a PC?" Such a PC's newest backup
of every game it ever backed up, and its newest safety backup of every game it ever restored,
stay for ever, whatever the rules say, until deleted by hand. A renamed PC counts as a new host,
so the old name's backups stay too. The user's words for the rule: "If the user decides to keep
the last 5 backups that's the last 5 independently of which PC made it".

## Decision

- **One list per game, across all hosts.** A regular snapshot of a game is kept when it is
  among the newest `keep_last`, or no older than `keep_within` before now (union, as before), or
  it is the newest of the list, or it is in `keep` (the snapshots a backup just made).
- **The newest of the list is always kept**, also when only `keep_within` is set and it is older
  than the span: it is the game's current saves. With both rules off nothing is forgotten, as
  before.
- **No protection per host.** A backup made on a PC that is behind or changed on both sides is
  one version in the list like any other.
- **`pre-restore` stays its own sequence** under the same rules, with its newest kept, so the
  last undo never goes by age alone. A safety backup is an undo point, not a version of the
  saves: counted in the regular list, two restores could push real versions out of `keep_last`.
- Nothing is rewritten in existing repositories. The first cleanup after the upgrade may forget
  many old per-host snapshots at once; the Back up result shows the count it already reports.

## Alternatives considered

- **Keep the newest of each host (ADR-0015):** a PC that is no longer used keeps a backup of
  every game for ever.
- **Keep the newest of each host that backed up within `keep_within`:** a second rule and a
  second clock to reason about; the user asked for the last N independently of the PC.
- **`pre-restore` in the regular list:** rejected above. **`pre-restore` without a kept
  newest:** with only `keep_within` set, the last undo would vanish after the span.

## Consequences

- A machine that plays less can lose all its snapshots of a game to other machines' newer ones,
  when its backups are outside `keep_last` and `keep_within`. That is what the rules say.
- The first cleanup after upgrading removes the other PCs' "newest per host" snapshots that the
  rules do not keep. It is in the release notes.
- A machine whose clock runs far ahead still takes the places of `keep_last` by time, and its
  backup counts as the newest; the others keep what falls in their place by the rules, not one
  each. Clock skew with `keep_within` is as in ADR-0015.
