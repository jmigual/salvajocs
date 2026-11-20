# ADR-0026: A restore ticks registry keys one by one

**Status:** Proposed
**Date:** 2026-11-20

## Context

ADR-0025 gave the restore plan one checkbox per registry root, because the restore replaces each
exported key's whole subtree (ADR-0006): a ticked parent would delete an unticked subkey at the
target and not write it back. The user asked for the registry subkeys to be ticked like folders
in the file tree. The restore must then stop deleting what is unticked.

Wine exports have gaps: wineserver writes no block for a key that has subkeys but no values, so
`RegFile::export` holds the declared root and every key with a block, but not a valueless key in
between. A Windows export holds every key.

## Decision

- **Nodes.** `RegistryExport::tree` gives the plan's export E as nodes in preorder: every key of
  E, plus each missing key between a key of E and its nearest ancestor in E (an added node, what
  a Wine export leaves out). Keys equal but for case are one node, by the replace's rule (whole
  `\` elements, Unicode lowercase). The fill never goes above a root, so `Software\Co\A` and
  `Software\Co\B` stay two roots. Each node has one tick, all ticked at first.
- **Selection.** `RegistryExport::select` gives W, the export to write, and K, the kept keys.
  W holds every ticked node with E's values, under all of its case spellings. A ticked added
  node goes into W, without values, only when its parent node is unticked; under a ticked parent
  it would change nothing. K holds one spelling of every unticked node.
- **The deepest node owns a key.** The owner of a key X on the target is the deepest key of
  W ∪ K that X is at or below. A key in both W and K is refused before anything changes
  (`RegistryError::KeptAndWritten`). The restore (`replace_subtrees(export, kept)`, Wine and
  Windows):
  1. every key owned by W loses its values, and is removed unless a key owned by K lies below
     it;
  2. then every key of W is written with its values;
  3. a key owned by K, or by nothing, is left exactly as it is; a Wine block stays byte for
     byte.

  With K empty, this deletes and writes what the restore did before.
- **Wine.** A block is dropped if and only if its owner is in W; dropping the block of a key
  that has a kept descendant is safe, as `user.reg` holds a key implicitly through its subkeys'
  blocks. The blocks of W are appended as before, except that a key of W without values gets no
  block when a block lies below it after the replace. That is wineserver's own save rule. Without
  it, an added node's empty block would make the next export of the prefix hold an extra key, so
  the next backup would differ from the newest snapshot (ADR-0018).
- **Windows.** For each key w of W, `remove_tree(w)` as before when no key of K lies below it.
  Otherwise w is cleared: its values are removed and each subkey is skipped when K owns it,
  cleared the same way when a key of K lies below it, else removed with `remove_tree`. Listings
  go through the same settle check as an export, and a listed subkey that cannot be reopened
  fails. All deletes still run before any write, and K is validated with W before any change.
- **GUI.** `KeyTree` holds the nodes and their ticks, never saved. A key with subkeys is a folder
  with a tri-state checkbox over its subtree and `N of M keys`, closed at first. Its first row,
  "(values)", ticks only that key's own node: its values and its subkeys that exist only on this
  PC. A key without subkeys is a plain checkbox. `RestorePlan::registry_kept` carries K to
  `restore::run`.
- **Safety backup.** It exports each key of W with its whole subtree from this PC, as before.
  An unticked subkey under a ticked key is therefore in the safety backup as it was before the
  restore. A key of W that this PC lacks adds its nearest ancestor this PC has, within the roots
  of W ∪ K, unless a key of W already covers it: the undo then replaces that ancestor whole and
  removes the key the restore created. With K empty the set is W, as before. No snapshot,
  `registry.json` or config format changes.

## Invariants

- **I1, nothing lost:** every key the restore deletes or overwrites lies at or below a key of W,
  so the safety backup holds its pre-restore values.
- **I2, unticked untouched:** a key owned by K is identical after the restore; in `user.reg` its
  block text is unchanged.
- **I3, undo exact:** restoring the `pre-restore` snapshot with everything ticked brings every
  key under the replaced keys back to its pre-restore state and touches nothing else. That
  includes removing a key the restore created under a key this PC had, also an unticked one,
  whose subtree the undo then puts back as it was before the restore. A created key with no
  existing ancestor within the plan's roots stays, as before.

Core tests prove I1 to I3 on Wine, and I1 and I2 on Windows CI.

## Alternatives considered

- **Generalising the file tree (`tabs/tree.rs`):** it splits on `/`, which a key name may hold,
  folds ASCII case only below `Win*` bases, has no state for a folder's own values and counts
  files and sizes. It would also change the Backup tab's widget.
- **A list of excluded subtrees without deepest-node ownership:** it fails for a ticked key under
  an unticked one, and for a valueless Wine key with keys that exist only on this PC.
- **An additive registry restore, like files:** not asked for, and stale keys would remain.
- **A safety backup of exactly what the restore deletes or overwrites:** the `pre-restore`
  snapshot would then have to record K, or its undo would delete the unticked subkeys. That is a
  `registry.json` format change, which older salvajocs versions would misread unless the new
  shape fails to parse there.
- **The folder's checkbox as the key itself plus everything below, without a "(values)" row:**
  a folder would show mixed while every row inside it is ticked, hiding the state the user set.

## Consequences

- An undo with everything ticked also sets an unticked subkey under a ticked key back to its
  state before the restore, and so an unticked key above a key the restore created. If nothing
  wrote it since, that is its current state; unticking it in the undo's plan leaves it as it is.
- A safety backup with kept keys reads the plan's root keys once more, to find which written
  keys this PC lacks.
- On Windows, a value name that does not reopen (lossy UTF-16) can make the selective delete
  fail midway; the safety backup is taken first, so nothing is lost.
- `replace_subtrees` (Wine and Windows) takes the kept keys, a breaking change for
  `salvajocs-core` callers. `RegistryExport::roots` and `remove_subtree` (ADR-0025, unreleased)
  are replaced by `tree` and `select`.
- The "Registry" line counts the ticked nodes, added ones included.
- The replace is still not atomic, so the safety backup stays mandatory.
