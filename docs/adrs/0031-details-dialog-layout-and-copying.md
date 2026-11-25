# ADR-0031: The Details dialog sets the copies side by side and copies a value from a menu

**Status:** Accepted
**Date:** 2026-11-25

## Context

ADR-0024's dialog drew size, mtime and location in a two-column table, but each SHA-256 on a
line of its own below it, and said "Not shown: a copy is over 10.0 MiB" without saying that the
diff was what was not shown, or which copy was over. A value could be copied only by selecting
it and pressing Ctrl+C. Right-click, then Copy on a selection cannot work in egui 0.37: any
press on a selectable label, the right button included, resets its selection
(`text_cursor_state.rs`), a press on anything else, such as a menu item, clears it
(`label_text_selection.rs`), and selected text leaves egui only on Ctrl+C or as the PRIMARY
selection.

## Decision

- **One grid, two value columns of equal width** for This PC and the Backup (its host and time
  in a weak line under its heading). Rows: Location, Size, Modified, SHA-256, and Line endings
  and Byte order mark when they matter. The hashes are rows; the backup's hash carries a weak
  line, "from its hash list" or "from its content", or "not recorded in this backup".
- **A mark after each label**, except Location: **Same** (equals sign, weak) or **Differs**
  (pencil, warn, the Changed badge's glyph and color). Size compares bytes, Modified the time to
  the second when both copies have one, SHA-256 the hex when both are known, line endings the
  counts, the mark the flag. None when this PC's copy could not be read, and none on line
  endings and the mark when a copy over the limit was not read, which shows "not read" there.
- **A verdict line above the grid** is the one emphasized element (semibold, an icon): "The text
  differs: N lines removed, M added.", "Only the line endings differ." (or the mark, or both),
  "Same content.". When no diff is shown it is weak and starts "Diff not shown:", naming the
  copy: over the 10.0 MiB limit (with its size), not text, or this PC's copy could not be read.
  **Show anyway** follows the over-the-limit sentence, with a tooltip saying it downloads the
  backup's copy. The diff, with its legend, stays below the grid.
- **Copy from a right-click menu on the value, not on the selection.** Each value (location,
  size, modified, hash, line endings, mark) and the key line under the heading is a selectable
  label with a context menu of one item, **Copy**, that copies the whole value; a hash is
  copied as its bare 64 hex digits. `widgets::copyable` draws it. Drag and Ctrl+C still copy a
  part. egui is not patched.
- No core, snapshot or config change.

## Alternatives considered

- **Patch egui so a right-click keeps the selection:** a fork of a dependency for a menu item.
- **A Copy button after each value:** eight more buttons in a dialog whose point is to compare.
- **Keep the hashes on their own lines:** they repeat what the columns already say.

## Consequences

- Right-click copies a whole value, never a selected part of one.
- The dialog's tests find the dialog by "Location" or "Close details" rather than the old
  "Backup (host, time)" heading.
- Amends ADR-0024: its "facts of both copies" and "no diff is shown" wording, not its comparison
  or its one download.
