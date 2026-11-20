# ADR-0020: A restore plans from the snapshot's metadata and downloads only on Restore

**Status:** Proposed
**Date:** 2026-11-19

## Context

A restore plan needs the snapshot's paths, `salvajocs.json` and `registry.json`, never the
saves' bytes. Yet the Restore tab's **Preview** restored the whole `/salvajocs/<slug>` subtree
into the cache dir before showing the plan (ADR-0004), and the batch (ADR-0019) did so for every
ticked game before its confirm. On a remote backend that is every data blob of every game, read
before the user has decided anything. The user asked why a preview downloads the snapshot when
the file paths and their hashes are enough.

Since 0.6.0 every game snapshot holds `hashes.json` (ADR-0018). Tree blobs and the index are
cached (ADR-0018), so a snapshot's listing is served from the cache after its first read, and a
small file costs one ranged read of a data pack.

## Decision

- **Plan from the listing (amends ADR-0004).** `restore::read` lists `/salvajocs/<slug>`
  (`Storage::snapshot_entries`, every node with its kind: file, dir or other) and reads
  `salvajocs.json`, `registry.json` and `hashes.json` alone. It writes nothing to disk. It
  refuses what `restore::fetch` refuses: an entry of `files/` that is no base dir
  (`UnknownBase`), a `files/` that is no dir or a JSON file that is no regular file
  (`WrongKind`), and an entry path that would leave the work dir. A symlink below
  `files/<Base>/` stays a write that the restore refuses (`SourceNotAFile`). A missing
  `salvajocs.json` is `FetchError::NoMeta`. Names that are not UTF-8 are converted lossily, as
  `fetch` does, so they fail one write, not the game.
- **The plan keeps its work dir.** `read` returns a `StagedGame` whose `root` is the work dir the
  download fills later, so `plan`, `prepare`, `Write` and `run` are unchanged. `plan` builds each
  `Write.source` lexically (`layout::target_path` without case folding), so the plan is the same
  before and after the download; a test checks this.
- **Download on Restore.** `restore::download(&Listed)` runs `fetch` of the snapshot the listing
  was read from (`Listed` carries it) into the planned `root`, and refuses with
  `FetchError::NotAsPlanned` unless the result equals the planned `StagedGame`. Then
  `restore::run` makes the safety backup and writes, as before (ADR-0005). A failed download
  returns before `run`: no safety backup, nothing written.
- **Labels from the hash list (reverses ADR-0019's "compare by content").**
  `restore::compare_writes(plan, &Listed)` takes the source side from the listing and compares
  each target by `changes::compare_file`, ADR-0018's rule: a different size is changed, the same
  size and mtime is unchanged (`run` sets a written file's mtime to the snapshot's), otherwise
  the target's SHA-256 against a verified hash list; without one (0.5.0 or older, unreadable, or
  not verified) a different mtime is changed. A target that cannot be read is
  `WriteState::Unknown`, shown as `could not be compared`, never unchanged. The hash is looked up
  by the snapshot-side key, so a cross-build write finds it. Its refusals stay: a symlinked
  target, a dir there, a source that is no regular file, and an unreadable target file that
  fails the whole game. `changes::same_content` goes.
- **Batch: download every game first, then run each.** Planning reads each game's listing
  into `restore-batch/<pid>/<index>` without creating it. The run job clears the batch dirs
  (ADR-0019's stale-dir rule moves here), downloads each planned game, then runs
  `restore::run` per downloaded game. A game whose download fails is reported failed with
  nothing written. Cancel has nothing on disk to remove.
- **Single game: the plan shows by itself.** The Preview button goes. The plan starts when a
  game, a snapshot and the targets are chosen and no read runs; at most one read runs, and a
  result for an older choice is dropped. A target change re-plans from the last read without
  touching the repository. A typed prefix applies on Enter or focus loss. While there is no
  plan, a line next to the disabled **Restore files** says why. **Restore files** clears the
  work dir, downloads, then runs.
- **Cross-build mapping: a button, not a modal.** A plan that needs mapping says so with a
  **Choose folders…** button that opens the mapping dialog (ADR-0011). Cancel closes it; the
  button stays.
- **Labels in the single-game plan too.** Each write of the single-game plan carries the batch's
  labels: new, changed, could not be compared or will fail. An overridden write is compared
  again against its new target. This reverses the plan's first proposal to leave them out.
- **The single-game plan is the batch's file tree.** Its rows are keyed by where each write
  lands as planned, its folders start closed and it is built once in the plan job; a list with a
  field per write drew about 15,000 widgets a frame for 3,000 files. A file's path shows on
  hover. Its **Override** button opens a path field under that row only; an overridden write
  keeps its row and shows its new target with **Clear**, which puts the planned write back.

## Alternatives considered

- **`Write.source` as the snapshot key `(Base, tail)`, with `run` taking the download dir:** the
  plan holds no path that does not exist yet, but `Write`, `plan`, `run`, about ten `Write`
  constructions and their tests change. The work dir fixed at plan time keeps the restore's
  invariants and tests as they are.
- **Download and run per game in the batch:** less on disk at once, but each download after
  the first safety backup waits for a reindex (ADR-0003), N extra reopens.
- **Open the mapping dialog by itself:** no new UI, but a click on a game in the list pops a
  modal, and after Cancel the dialog is only reached by choosing the game again.
- **Keep the Preview button:** the plan stays one click away, and the user's complaint was that
  nothing explained why **Restore files** is disabled.
- **A separate `SnapshotEntry` type:** one more public type with the fields of `SnapshotFile`.

## Consequences

| Step | Before | After |
| --- | --- | --- |
| Single preview | Snapshot, trees and every data blob of the game, written to the cache dir | Snapshot and trees (cached after the first read), plus at most three small blob reads |
| Changing targets | A new full fetch | No repository access |
| Restore files | Local copy | Full subtree download, then the same run |
| Batch planning, N games | N full downloads | N listings plus at most three small reads each |
| Batch run | N × (reindex, safety backup, writes) | N downloads first, then the same |
| Batch cancel | Downloads wasted, then removed | Nothing downloaded |

- The reindex count is unchanged: one in each safety backup, and the first read after a
  restore reindexes once (ADR-0003).
- For a snapshot made before 0.6.0, a target with the same bytes and another mtime is labelled
  `changed` (ADR-0018's fallback). It is written either way.
- Comparing hashes a target only when the sizes match and the mtimes differ, on local disk.
- `download` takes the snapshot from the listing, so it cannot write another snapshot that has
  the same paths; snapshot ids are content addresses, so the same id restores the same bytes.
