# ADR-0025: A restore plan chooses its files and registry keys with checkboxes

**Status:** Proposed
**Date:** 2026-11-20

## Context

The user asked for a checkbox on each file of the restore plan, to decide which files are
restored, both in the plan of one game and in the plan of several games. Until now both plans
were read-only trees (ADR-0019, ADR-0020): a restore wrote every planned file and registry key.
In the batch, a target the safety backup could not read failed the whole game, with no way to
restore the rest of it.

The Backup tab already chooses what a backup holds with a checkbox per file, per folder and per
registry key (ADR-0014, ADR-0018). The user asked the restore to do the same.

A restore replaces registry keys by subtree: `replace_subtrees` (Wine and Windows, ADR-0006)
deletes the subtree of every key in the export, then writes the exported keys. `registry.json`
is a flat map, and an export of a declared key `K` holds `K` and every key below it.

## Decision

- **Checkboxes in both plans.** The single game's tree and each game's tree in the batch confirm
  draw in the Backup tab's select mode: a checkbox on every file and folder, tri-state folders
  with `N of M files`. Everything starts ticked. The batch has no game-level checkbox; the
  sidebar's tick is that.
- **One checkbox per registry root.** A root is an export key with no ancestor key in the export:
  the declared key as backed up. Unticking it leaves its whole subtree out. A single subkey cannot
  be left out: its parent's subtree replace would delete it at the target without writing it
  back. Roots are pairwise disjoint, so the other roots' replace never reaches into an unticked
  one. `RegistryExport::roots` and `remove_subtree` in `salvajocs-core` compute them, with the
  replace's whole-element, case-ignoring match. Nested declared keys share the outer root's box.
  The GUI works the roots and their key counts out once per plan, not per frame.
- **The GUI filters the plan.** The selection is an in-memory `selection::GameSettings` per plan,
  keyed by each write's planned `<Base>/<tail>` key (not the overridden one) and by root key.
  At Restore a clone of the plan keeps the ticked writes and drops the unticked roots' subtrees
  (no registry at all when none is left). That one filtered plan goes to `restore::run`, which
  backs up and writes it (ADR-0005), and to the result lines. An unticked item is neither written
  nor in the safety backup.
- **Ticks reset with each plan.** A new snapshot, target, mapping confirm or Try again ticks
  everything again, as a new plan drops its overrides. Override, Apply and Clear keep the tick;
  an unticked overridden file is not written.
- **Only ticked writes block.** The "safety backup cannot read" list, the "will fail" lines, the
  heading's count and the chips count ticked writes. Unticking an unreadable target lets the rest
  of the game restore, in the single game and in the batch (amends ADR-0019, where it failed the
  game). Restore is disabled while nothing is ticked; the batch leaves out a game with nothing
  ticked or with a ticked write that blocks. A plan with no write and no registry says "Nothing
  to restore." instead of asking for a tick; the batch leaves such a game out and says so.
- **The batch labels each write.** It plans with the single game's labelling (one `Label` per
  write and the same tree), replacing its own whole-plan compare and key-based badges.
- **Nothing is saved.** No config, settings snapshot or `pending-selection.json` holds the
  ticks; the snapshot, repository and `RestorePlan` formats are unchanged. The download still
  fetches the game's whole subtree (ADR-0020).

## Alternatives considered

- **One registry checkbox per game:** the Backup tab has one per key, and the user asked for the
  same.
- **One checkbox per flattened subkey:** unsafe, as a ticked parent's subtree replace deletes an
  unticked child at the target.
- **Keeping ticks by key across plans:** a Linux ↔ prefix switch changes the keys' bases, so
  ticks would carry over partly.
- **A core `RestorePlan::retain` API:** the plan's fields are public; filtering is the GUI's
  choice, and `run` stays unchanged.
- **Making `selection::covers` public:** `GameSettings::set_path` and `path_included` already
  apply it.
- **Saving the ticks:** a restore is a one-off choice; saved ticks would hide files from later
  restores.
- **Downloading only the ticked files:** a later optimisation; the download and its check
  against the plan stay as they are.
- **Recording registry roots in `SnapshotMeta`:** a format change, not needed, since the roots
  derive from the export.

## Consequences

- An undo of such a restore holds only the ticked items, which are all it replaced.
- Unticking every registry root lets the files restore while a wineserver runs (ADR-0006 refuses
  only a registry restore into a prefix).
- Nested declared keys (`Software\Co` and `Software\Co\Game`) show one checkbox, the outer one's.
- Roots are per game: in a batch, unticking game B's root does not protect it when game A
  declares an ancestor key, as A's subtree replace still deletes it; A's safety backup holds it,
  so undoing A recovers it.
- Writes that land on one key share a checkbox; the later one would overwrite the earlier
  anyway.
