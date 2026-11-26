# ADR-0024: The Backup and Restore tabs show what makes a changed file differ

**Status:** Accepted
**Date:** 2026-11-20

## Context

The Backup tab marks a file `changed` (ADR-0018) but not why. The user saw changed files, could
not tell what differed, and suspected line endings between Windows and Linux. A backup holds
every file's size, mtime and, since 0.6.0, SHA-256 (`hashes.json`), but the preview deliberately
reads no file content: ADR-0018 rejected downloading every compared file.

## Decision

- **A Details button on a `changed` row** of the Backup tab opens a dialog (`core::changes::details`
  and `tabs/details.rs`). Other rows (new, removed, not compared, unticked) have none.
- **The Restore tab has it too**, on each row of the single-game plan whose write is `Changed`
  (ADR-0020), not on new, unchanged or unreadable ones. The backup's copy is the plan's snapshot,
  which may not be the newest, found by `Listed::backed_up(write)` from the listing and hash
  list the plan was made from (the snapshot's own path, also for a cross-build or prefix target);
  this PC's copy is the write's target, overrides included. The dialog is the Backup tab's and
  is dropped with the plan. The batch confirm has none.
- **Facts of both copies:** location (this PC's path; the backup's path below `files/`, which for a
  backup of the other build is its own, not the mapped one, ADR-0021), size, mtime to the second
  (UTC) and SHA-256. The backup's hash comes from its verified hash list, else from the
  downloaded content, else it is said not to be recorded. This PC's file is read and hashed when
  the button is clicked, not taken from the scan.
  Amended by ADR-0031: the hashes are rows of one grid of the two copies, each with a Same or
  Differs mark.
- **Text diff.** Each copy is text when it is valid UTF-8 without a NUL byte. The diff is over the
  texts with every line ending (CRLF, LF, bare CR) read as LF and a byte order mark left out
  (each copy that has one is named), the backup's lines as removed and this PC's as added, with
  three lines of context; a last line without a line ending is marked. When the normalized texts
  are equal the dialog says only the line endings, the mark or both differ, and shows each copy's
  count of CRLF, LF and CR lines. The diff is `similar`'s line diff
  with a one second timeout: past it the diff is correct but not minimal, so it can differ
  between runs.
- **One download, on click.** `read_backed_up` reads the one file through `Storage::read_file`
  while the job holds the repository lock, only when the backup's copy and this PC's copy are
  both at most 10 MiB. Over that, nothing is downloaded and the dialog says no diff is shown,
  with a **Show anyway** button that runs the same job without the cap (the 1 s diff timeout, the
  1000 drawn lines and the text check stay). `read_backed_up` and `details` take the cap as an
  `Option<u64>`, `None` for no cap. Hashing and the diff run after the lock is released. The job
  only reads, so it is not counted as busy, as the preview.
  Amended by ADR-0031: the dialog says "Diff not shown" and names the copy over the limit.
- **Finding the file.** `Baseline::backed_up(key)` finds the backup's entry with the lookup
  `compare_game` uses (exact key, else the first case-folded one below Windows-family bases),
  now one helper. `Mapped.sources` records each re-keyed file's own snapshot key, so a mapped
  file is read at its original path. This adds an argument to `Mapped::new`.
- **New dependency:** `similar` 3.2.1 (Apache-2.0, no transitive dependencies, default features),
  pinned in `[workspace.dependencies]` and used by `salvajocs-core` only (ADR-0001). Core returns
  its own `#[non_exhaustive]` types, so `similar` is not part of its API.
- No snapshot, config or `hashes.json` format changes.

## Alternatives considered

- **Compare only hashes, sizes and mtimes:** cannot say that line endings are the whole difference.
- **Diff the raw bytes' lines:** every line of a CRLF file against an LF file would differ, hiding
  the changes that matter.
- **A diff library written here:** a correct, fast line diff is non-trivial; `similar` has no
  dependencies.
- **Download every changed file with the preview:** ADR-0018 rejected reading every compared file.
- **No size cap:** a click on a large binary would download and diff it all; hence the cap and
  the button that lifts it for one file.

## Consequences

- This is the first read of a file below `files/` outside a restore, and it happens only on a click.
  On a remote backend it costs a ranged read of the packs holding that file.
- A backup of the other build is read at its own path, so the dialog names two paths.
- The 1 s timeout makes the shown diff nondeterministic for large, very different texts; tests
  use small ones.
- `Mapped::new` takes `sources`: a minor-version break of `salvajocs-core`.
- Closing the dialog during a **Show anyway** read does not cancel it: the detached job goes on
  downloading and diffing the uncapped file under the repository lock until it ends. Cancelling
  is out of scope.
