# ADR-0021: The backup preview and skip map the other build's backup through the folder pairs

**Status:** Accepted
**Date:** 2026-11-19

## Context

The Backup tab compares each game with its newest backup from any machine (ADR-0018). A game
played on both the native Linux build and the Windows build keeps its saves in other folders on
each: Factorio's newest backup, made on Windows, holds `WinAppData/Factorio/...`, while this
Linux machine finds `Home/.factorio/...`. By path, every file is removed and every file is new,
so the game showed `+17 −17` with the same saves on both sides, and a backup was always made.

A cross-build restore already knows where each folder of the other build goes on this machine:
saved pairs and skips, custom game rows, engine rules and the manifest's auto-pair, through
`restore::prepare` (ADR-0011, ADR-0016). Since ADR-0020 it plans from the snapshot's metadata,
without downloading anything.

## Decision

- **"Build" is the base family of the scan's roots**, not the host OS: a Proton or Wine game on
  Linux is the Windows build, so Proton against native Windows stays a same-build comparison. A
  baseline is from the other build when its `files/<Base>` directories hold a family the scan's
  roots do not. Only a scan of exactly one family is mapped; a mixed scan, a scan of
  install-relative roots only, and a same-build baseline compare by path as before, reading
  nothing more.
- **Map like a restore would.** Core reads the baseline's metadata (`restore::read`) and plans a
  restore of it onto this scan's build with `restore::prepare`. The target is built from the
  scan, not from the restore's default targets: the native Linux folders for a Linux scan, the
  native Windows folders or the scan's first Wine prefix for a Windows scan. A root that a
  restore would ask about is skipped in memory; nothing is saved.
- **Re-key, then compare as before.** Each mapped file of the other build takes the path key of
  its planned target, and `compare_game`/`counts` run unchanged on the re-keyed listing and hash
  list. Files of this build and install-relative files keep their keys. A file of the other
  build with no target (unknown, skipped or undeclared root, unsafe path, or a key another file
  already takes) stays at its own key, so it shows as removed, with a tooltip saying it has no
  place here. The game's tooltip names the folders a restore would skip.
- **Skip rule (B) through the mapping (amends ADR-0018).** For a mapped baseline, a game gets no
  snapshot only when the baseline has a verified hash list, every one of its files is placed, no
  two land on one key, the registry hashes are equal (both absent, or the same), and the staged
  files equal the re-keyed ones by key (ASCII case folded below Windows bases, as
  `compare_game` matches), size and SHA-256. `salvajocs.json` is not compared: its roots always
  differ across builds. The pre-restore safety backup never maps (ADR-0018).
- **The preview promises a mapped skip only when neither side has registry keys.** The preview
  never reads the registry (ADR-0018), so when either side has keys the header shows the "same
  files, but a backup will be made" sign with that reason, and the backup does store a snapshot.
  Both the promise and the decision are made in `core::changes`, so they cannot drift.
- **The rule-(A) prediction keeps the snapshot's own paths.** rustic compares trees by path, so
  the prediction of `skip_if_unchanged` against this host's parent uses the unmapped listing.
- **Choose folders… on the Backup tab.** A game whose newest backup has a folder of the other
  build that a restore would ask about gets a **Choose folders…** button. It opens the restore's
  mapping dialog; the choices are saved as `pairs` and `skips` exactly as after a restore
  (ADR-0011), then the comparison runs again. No config schema or snapshot format changes.

## Alternatives considered

- **Show as today:** the user reported the `+17 −17` as the problem.
- **Compare by file name only:** wrong whenever the two builds lay their folders out
  differently.
- **Map from the restore's default targets:** on Linux with a Proton prefix they hold both
  families, so `prepare` maps nothing, and the direction would follow the prefix rather than the
  build that was scanned.
- **Keep comparing `salvajocs.json` in (B):** it holds the roots, which never match across
  builds, so (B) could never hold.
- **Loosen the auto-pair** to ignore install-relative templates: changes every restore of such
  games and still leaves Factorio's `config` folder unmapped, since it is not save-tagged.

## Consequences

- A same-build baseline costs what it did (listing plus `hashes.json`). A baseline of the other
  build costs a second walk of its tree (`restore::read` lists it again with its entry kinds;
  trees are cached, so that walk reads nothing remote once the first has) and up to three more
  small ranged reads (`salvajocs.json`, `registry.json`, and `hashes.json` again) per preview
  and per backup; nothing is downloaded. A metadata read that fails falls back to the
  comparison by path.
- After a mapped (B) skip, this machine's state lives in a snapshot of the other build.
  Restoring it here needs the mapping to still be known (the pair or row kept, or the dialog
  answered again), and on Linux the native build must be picked as the restore target, not a
  Proton prefix (ADR-0005).
- Unity games keep `PlayerPrefs` in the registry on Windows and in a `prefs` file on Linux, so
  across builds they always back up, and the `prefs` file shows as removed or new.
- `backup::backup_games` takes the mapping inputs instead of a flag, and `changes::baseline`
  gives way to one that maps: a minor-version break of `salvajocs-core`.
