# ADR-0027: The Backup and Restore tabs sort their games by name, last backup or space used

**Status:** Accepted
**Date:** 2026-11-20

## Context

The user asked to sort the games of the Backup and Restore tabs by name, by the size of the
latest backup, by the space all of a game's backups use together, or by the time of the last
backup. Each game's row should show these figures whichever key sorts the list. The space used
must be exact, and the chosen sort must be remembered, one per tab.

Both lists were sorted by title in byte order: the scan's `BTreeMap` and `restore::list`.

What rustic_core 0.13.0 offers:

- A snapshot's `summary.total_bytes_processed` is the size of every file it holds, unchanged
  files included, before compression. The archiver always writes the summary. A restic before
  0.17 writes none. Reading it costs only the snapshot list.
- Nothing computes the space a set of snapshots uses. `Repository::infos_index` and
  `infos_files` give repository-wide totals only. `TreeStreamerOnce`, which visits each tree
  once across several roots, is crate-private. `Repository::ls` walks a shared subtree again for
  each snapshot.
- The public parts are enough to walk the trees ourselves: `get_tree` returns a tree's nodes,
  `Node.subtree` and `Node.content` name its subtrees and data blobs, and `get_index_entry`
  gives a blob's `location.length`, its bytes in the pack, compressed and encrypted. Trees are
  read through rustic's on-disk cache (on by default), which downloads a whole tree pack on a
  miss. No data blob is read: its length is in the in-memory index.

## Decision

- **What each figure means.** "The backup" of a game is its newest snapshot from any host that
  is not a `pre-restore` safety backup, the restore default (ADR-0010) and the Backup tab's
  baseline (ADR-0018).

  | Figure | Definition | Missing |
  | --- | --- | --- |
  | Last backup | The time of that snapshot | No such snapshot: "No backup" |
  | Last backup size | Its `summary.total_bytes_processed`: its files before compression, `salvajocs.json`, `hashes.json` and `registry.json` included | No summary: "size unknown" |
  | Space used | The sum of the packed sizes of the distinct tree and data blobs that all of the game's snapshots reference, `pre-restore` included (restic's `stats --mode raw-data`) | While computing: "…"; the walk failed: unknown, with the error on hover |

  - Settings snapshots (ADR-0014) carry `salvajocs-settings`, not `salvajocs`, so no figure
    counts them.
  - Safety backups are left out of the last backup and its size, so a restore on one machine
    never becomes everyone's last backup. They count toward the space used, since they take
    space.
  - The space used is what deleting every backup of the game and pruning would free, if no
    other game shared a blob. A blob two of the game's snapshots share counts once. The path
    trees `/salvajocs/<slug>` count, since only that game has them. Pack headers, index and
    snapshot files, and data a forget left for the next prune are not counted.
  - **A blob another game's snapshots also use counts for each game.** The figures of all games
    can then add up to more than the repository holds. The hover text and the guide say so. In
    save data such a blob is rare: identical files, or the empty folder's tree.
- **Core API (ADR-0001, ADR-0003).**
  - `SnapshotInfo.size: Option<u64>`, filled from the summary; `SnapshotInfo::new` keeps its
    arguments and `with_size` sets it.
  - `Storage::space_used(&[&str]) -> Result<u64, StorageError>` walks the trees of the given
    snapshots once each, with one set of visited trees and one of data blobs, and sums their
    index lengths. Each snapshot is read through `Storage::snapshot`, so it reindexes once
    when a tree is missing (ADR-0003). A blob missing from the index then is an error, never 0.
  - `core::stats`, pure: `BackupStats { last, last_size, space_used, snapshot_ids }`;
    `BackupStats::of` one game's snapshots; `by_game` keyed by the `game:` tag's slug, as
    `restore::list` groups; `order(rows, ListSort) -> Vec<usize>`.
- **Sort.** `SortKey { Name, LastBackup, LastBackupSize, SpaceUsed }` and `ListSort { by,
  descending }` in `core::config`.
  - Each key has a natural direction: names A to Z, sizes largest first, times newest first.
    Picking a key resets the direction to it; an arrow button reverses it.
  - A row without a value for the key (no backup, size unknown, space still computing or
    failed) sorts after every row with one, in both directions.
  - Ties fall back to the title in byte order, then the list's own order, so Name A to Z keeps
    today's order exactly.
- **Config (amends ADR-0009).** `backup_sort` and `restore_sort`, each `{ by: space_used,
  descending: true }`, left out when Name A to Z. A value that does not parse, such as a key a
  newer version adds or a number, reads as the default, and the rest of the config still loads:
  it goes through a `serde_json::Value` first, as `SnapshotMeta.prefix` does (ADR-0023). An
  older version drops both fields on its next save, as any newer field. The GUI keeps no copy:
  it reads `state.config` each frame and saves through `State::save_config`. `State::adopt`
  keeps both when another repository is connected.
- **UI.**
  - A **Sort by** row directly above each list: a dropdown (**Name**, **Last backup**, **Last
    backup size**, **Space used**, each with its definition on hover) and an arrow-up or
    arrow-down icon button whose hover text and accessible name say the order in words ("A to
    Z", "Largest first", "Newest first" and their reverses). On the Backup tab it sits below the
    status lines, not in the header row, which has no room at the minimum window size. On the
    Restore tab it sits below Select all and Deselect all in the sidebar.
  - The Backup tab shows one weak line under each game's title: `Last backup 2026-11-19 14:02
    UTC, 980.0 KiB. All backups use 3.1 MiB.`, or `No backup.`, or `No backup. Safety backups
    use 1.0 MiB.` The sorted figure is in the normal text color. No line shows until the
    figures are known.
  - The Restore sidebar shows a second, truncated line under each title with the sorted figure
    first: `3.1 MiB used, 2026-11-19, 980.0 KiB`. Its hover text has the full sentence.
- **Cost per frame.** Each tab keeps its row order and the formatted lines, rebuilt only when the
  sort, the figures or the list change. The Backup tab never reorders `report.games` or what is
  aligned with it; it draws through the order and keeps each row's index for every action.
- **Space job and cache.** A `Session` keeps a map from a game's sorted snapshot ids to its
  space used, so a figure stays valid as long as that exact set of snapshots does, and another
  repository starts empty. A job shared by both tabs computes the games not in the map, locking
  the repository for one game at a time, and stops between games when its tab drops it.
  Failures are not kept. The Backup tab starts it once its comparison job has listed the
  snapshots, the Restore tab when a listing arrives. A list sorted by space used is reordered
  once, when the job ends.
- **Freshness.**

  | Event | Backup tab | Restore tab |
  | --- | --- | --- |
  | Scan, backup with its cleanup, another repository, folders chosen | the comparison runs again | dropped after a cleanup forgot backups, otherwise the next Load |
  | Restore, batch restore or delete in the Restore tab | the comparison runs again | listed again |
  | Another machine backs up | the next scan or backup | the next Load |

## Alternatives considered

- **The sum of the summaries' `data_added_packed`:** free, but wrong once a forget removes the
  snapshot that added data a later snapshot still uses.
- **`Repository::ls` per snapshot:** walks the subtrees snapshots share once per snapshot.
- **Splitting a shared blob between the games that use it:** the figures would add up to the
  repository's size, but one game's backup would change other games' figures, so no figure
  could be cached by its own snapshots.
- **A space cache on disk:** with rustic's tree cache, recomputing at start is cheap (below).
- **Case-insensitive names:** would change today's order; byte order is kept.
- **Long labels ("Space used by all backups"):** do not fit the Restore sidebar's dropdown; the
  full names are in the hover text.
- **A text direction toggle, or the sort in the Backup tab's header row:** the text does not
  fit the sidebar, and the header row has no room at the minimum window size.
- **Not remembering the sort:** the user asked for it.

## Consequences

- The space used of every game is computed in the background at each start and after each
  change to a game's snapshots, for that game only.
  - Measured (release build, 2026-11-20, local repository on disk, 20 games of 20 backups
    each, each backup of ten 16 KiB files in three folders with one file changed, 5.2 MiB in
    all; `space_used_timing` in `storage/tests.rs`): all 20 games in 0.13 s with rustic's
    cache empty and 0.043 s with it filled, the repository open excluded. "Empty" is rustic's
    cache only: the OS still had the pack files in memory.
  - On a remote repository the first walk downloads each tree pack once; later starts read the
    local cache.
- The Backup tab's comparison now also runs after a restore or a delete in the Restore tab, so
  its badges are no longer stale then.
- The screenshots in the guide do not show the new row and line until they are taken again.
