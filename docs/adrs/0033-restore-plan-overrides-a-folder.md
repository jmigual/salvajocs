# ADR-0033: The restore plan overrides a whole folder

**Status:** Accepted
**Date:** 2026-11-25

## Context

The restore plan lets the user send one file to another absolute path, for this restore only
(ADR-0005). The user found that this works per file only: to restore a folder of saves somewhere
else, each file needs its own Override.

`RestorePlan::override_target` already redirects one write and makes the safety backup stage its
target by its full path (ADR-0005), so what the restore overwrites stays backed up. The plan's
tree is keyed by each write's planned `<Base>/<tail>` key, which the ticks use (ADR-0025).

## Decision

- **A folder row has an Override button.** It opens the folder and a field "New target of
  folder `<key>`" at the top of its body. The folder is the deepest one of a chain drawn as one
  row, the one its label names. The tree reports each folder, a function listing its present
  files' row positions (called only on Apply or Clear) and where to draw to the caller
  (`FileTree::ui_with_folder_rows`); `ui_with_rows` is unchanged and lists nothing.
- **Apply sends every write below the folder into the new path.** A write's new target is the
  path typed, then the last plain components of its planned target, as many as the file's key
  has below the folder (a drive is its letter, as in the key), so each file keeps its place and
  never leaves the new path. A write whose planned target has fewer plain components than that
  is refused and no write changes. This goes through `RestorePlan::override_target` per write,
  so a relative path is refused with that error and no write changes. The planned target, not
  the current one, supplies the components, which keeps the case a Windows-family prefix has on
  disk.
- **The header shows where the folder goes**, with the path truncated to the window and a
  **Clear** button, which puts every write below back to its planned target. The files' own rows
  show their new targets as for a per-file override.
- **An override ends once it no longer describes its folder.** A folder override replaces the
  file overrides below it. Applying or clearing a file's override below a folder override, or
  overriding or clearing a folder inside or around it, ends that override; the files stay where
  they are and show their own targets and Clear buttons.
- **Ticks, safety backup and run are unchanged.** Ticks stay keyed by the planned key (ADR-0025),
  so an unticked file below an overridden folder is not written and not in the safety backup.
  The restore backs up and writes the one filtered plan, so the safety backup holds what is at
  the overridden targets of the ticked writes.
- **Targets are compared in a job.** A folder can hold many files, and each target costs a stat
  and at most one hash, so a read-only job labels the moved writes. While it runs the override
  buttons and Restore files wait. A job that fails marks the writes as unreadable, which blocks
  the restore.
- **An unapplied folder field blocks Restore files**, as a file's does.
- **Nothing is saved.** An override is for one plan and is dropped with it. The batch confirm
  still has no overrides (ADR-0019).

## Alternatives considered

- **A core `RestorePlan::override_folder`:** the GUI knows which writes are below a folder
  (the tree's keys); `override_target` already validates and clears the safety-backup fields.
- **Saving a folder override as a pair (ADR-0011):** a pair is per root and permanent; this is
  for one restore.
- **Keeping a folder override when a file below changes:** its header would claim a target some
  files no longer have.
- **Comparing the targets on the UI thread:** a large folder would freeze the window.

## Consequences

- A pre-restore snapshot made after a folder override holds its files under `Absolute` or
  `WinAbsolute`, so it is not restorable through the normal flow (ADR-0005).
