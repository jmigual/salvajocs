# ADR-0018: Hash list per game snapshot; backup preview and skip compare with the newest one

**Status:** Proposed
**Date:** 2026-11-19

## Context

Like ludusavi, the user wants to see which files the next backup would add, change or remove.
The machines that share a repository back up the same games, and a launcher's cloud sync can
put the same saves on two machines. rustic's `skip_if_unchanged` (ADR-0010) compares the new
root tree id with the parent's. The parent is the latest snapshot of the same host, label and
paths, tags ignored. A tree id hashes node metadata (mode, owner, mtime, ctime, inode, device),
so two machines never produce the same tree id for the same files, and each makes its own
snapshot.

A snapshot's file nodes list chunk ids, but rustic's chunker is private, and reproducing it
(copying its Rabin loop) ties salvajocs to rustic's internals on every upgrade. Whole-file
SHA-256 hashes, written by salvajocs itself, need nothing from rustic.

## Decision

- **Hash list in every game snapshot** (amends ADR-0004): staging writes `hashes.json` next to
  `salvajocs.json` and `registry.json`, from 0.6.0 on:
  `{ "format": 1, "meta": <sha256>, "registry": <sha256 or absent>, "files": { "<Base>/<tail>":
  { "sha256": <hex>, "size": <bytes>, "modified": <mtime> } } }`. Keys are the paths below
  `files/` (`selection::path_key`); `meta` and `registry` hash the two JSON files, so the list
  covers every other file of the staged directory. `layout::Hashes` is the type.
  - A copied file is hashed while it is copied. A file that is not copied keeps the previous
    list's entry when the staged file's size and mtime still match it; otherwise the staged
    file is read once. The list is written only when its bytes change, so an unchanged game
    keeps the mtime of `hashes.json` and rustic still finds it unchanged.
  - `modified` (the staged file's mtime, seconds and nanoseconds since the epoch) only checks
    that an entry still describes the staged file, also after an older version staged it.
    Comparisons ignore it.
- **Baseline:** the newest snapshot of the game (tag `game:<slug>`) from any host, ignoring
  `pre-restore` snapshots, the restore list's default (ADR-0010). `core::changes::newest` picks
  it, for the preview and the skip rule.
- **Changed means content.** For a file present in the baseline (same path):
  1. a different size is changed, nothing read;
  2. the same size and mtime is the same, nothing read: the rule staging and rustic already
     trust. It misses only a file rewritten with its size kept and its mtime put back;
  3. otherwise, when the baseline's list has the file, the local file's SHA-256 is compared
     with it;
  4. a baseline without `hashes.json` (made by 0.5.0 or older) falls back to size and mtime: a
     different mtime is changed.
- **Shared module `core::changes`** (salvajocs types only): `newest`, `compare_file`,
  `compare_game` (found files against the baseline listing; paths fold ASCII case below
  Windows-family bases, as ADR-0014 does), `counts` (by the game's selection: an unticked file
  that is in the baseline counts as removed) and `same_content` (two local files, for the
  restore's confirm tree).
- **Skip rule (amends ADR-0010).** A game gets no snapshot when
  - (A) nothing changed since this machine's last backup: rustic's `skip_if_unchanged`; or
  - (B) its staged hash list equals the hash list of the newest non-`pre-restore` snapshot of
    the game from any host: the same paths, sizes and hashes, and the same `salvajocs.json`
    and `registry.json`. A baseline without a list never matches. Checked before rustic is
    called; never a tree id compare, since tree ids differ across machines.
  - The outcome is `Stored::New`, `Stored::Unchanged { newer_elsewhere }` or
    `Stored::SameAsNewest(newest)`. When (B) holds and the newest is this machine's own
    snapshot, the outcome is `Unchanged { newer_elsewhere: None }`: an unchanged rerun reads
    "up to date". `newer_elsewhere` is the newest snapshot when it is from another host.
  - `backup_games` reads every game's baseline list right after its one reindex, before it
    writes anything, so no listing waits for another reindex (ADR-0003).
  - (B) is off for the pre-restore safety backup (`backup_games(.., same_as_newest: false)`):
    that copy belongs to the `pre-restore` retention sequence (ADR-0015), and must not rely on a
    snapshot of the other sequence that retention may later forget (ADR-0005).
- **Preview on the Backup tab:** after each scan, a background job (not counted as busy) lists
  each game's newest snapshot, reads its hash list, lists this machine's parent, compares, and
  predicts (A) by path set, size and mtime against the parent. The game line says what differs
  from the newest backup and from which host, or that no backup will be made and why; files
  carry `new` or `changed` badges; files in the baseline but no longer found (or unticked) are
  greyed `removed` rows in the tree. Registry keys and `salvajocs.json` are not previewed.
- **Shared tree:** the file tree moves to `tabs/tree.rs` with status badges and a read-only
  mode, for the restore's confirm tree.

## Alternatives considered

- **Compare chunk ids:** needs rustic's private chunker reproduced (its Rabin loop over
  `rustic_cdc`), to be checked again on every rustic upgrade. The user chose the hash list.
- **Compare tree ids for (B):** never equal across machines, nor on one machine after any
  metadata change.
- **Read the snapshot's files and compare bytes:** downloads every compared file.
- **Size and mtime only:** another machine's copy of the same save has other mtimes, so every
  file would show as changed. Kept only as the fallback for snapshots without a list.
- **Baseline = this machine's last backup:** hides that another machine has a newer save.

## Consequences

- Reading a list costs one small blob per game. Data packs are not cached (trees and index
  are), so on a remote backend each game's list is one ranged read per backup and per preview.
- The first 0.6.0 backup of each game on each machine makes a snapshot: the staging directory
  gains `hashes.json`, and older snapshots have no list for (B) to compare with.
- After a (B) skip, this machine's state lives in another machine's snapshot. Retention keeps
  each host's newest (ADR-0015); once the other machine backs up something newer, (B) is false
  here and the next backup stores this machine's own state.
- A game shown as unchanged can still be backed up when only its registry keys or roots changed.
