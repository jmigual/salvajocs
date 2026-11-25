# ADR-0032: The Backup tab sorts its games by how many files changed

**Status:** Accepted
**Date:** 2026-11-25

## Context

The user asked to sort the Backup tab by "changed", so the games with backup changes (files
added, removed or modified) are on top or at the bottom. ADR-0027 sorts by figures of the
repository's snapshots; the changes are not among them. They come from the comparison of the
scan with the newest backup that the Backup tab already runs (ADR-0018), and they follow the
ticks: unticking a file changes the count.

## Decision

- **A new key `SortKey::Changes`**, saved as `changes` in `backup_sort`. Its natural direction
  is descending, "Most changes first", reversed "Fewest changes first".
- **The value of a game** is what the Backup tab's header counts, by `preview::next_backup`:

  | Next backup | Value |
  | --- | --- |
  | Differs from the newest backup by `counts` | `new + changed + removed + unknown` |
  | No backup yet | the number of included files, and 1 for the registry keys, all of them new |
  | The same as the newest backup, or unchanged since this PC's last | 0 |
  | No preview yet, the preview failed, or the game is skipped or has nothing left | none |

  A game with no value sorts after every game with one, in both directions, as in ADR-0027.
  Ties fall back to the title.
- **Core.** `stats::order` rows are `(title, Option<&BackupStats>, Option<usize>)`; the third
  is the number of changes, which only the Backup tab knows. The Restore tab passes `None`.
- **Backup tab only.** `sort_ui` takes the keys to list; the Restore sidebar lists the four
  keys of ADR-0027. A `restore_sort` of `changes`, from an edited config, is read there as the
  default, Name A to Z, and left in the file.
- **When the order is rebuilt.** The values are read when the order is built. Under Changes,
  the order is also built when the previews arrive and when the settings load, since both change
  the counts; under another key they do not, so ADR-0027's single reorder when every space
  figure is in holds. A tick does not rebuild it, so a game does not jump away under the cursor
  (ADR-0027); the next rebuild, such as a reversed order, another key, a scan or a backup,
  takes the ticks into account.

## Alternatives considered

- **Count only added, removed and modified files of games with a backup:** a game with no
  backup yet would then sort as unchanged, though a backup of it adds everything.
- **Rebuild the order on every tick:** the list would move while the user ticks a file.
- **Keep the key in the Restore tab:** it has no scan and no comparison to count.

## Consequences

- An older version reads `changes` as the default sort, as it does any key it does not know
  (ADR-0027).
- The order by changes is stale after ticks until the next rebuild.
- `change_count` in `tabs/backup/preview.rs` reads `next_backup`, so the sort and the header
  agree on what counts.
