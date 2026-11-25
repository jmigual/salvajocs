# Restore

The Restore tab lists the backups of every machine that uses the repository, shows exactly what a
restore would write, backs up what it is about to overwrite, and then writes the files. This page
walks through those steps and explains how to undo a restore. Back to the
[guide index](README.md).

![The Restore tab](../images/restore.png)

## Steps

1. Click **Load backups**. The list shows every game in the repository, including backups made
   on other machines. Under each title a short line gives the date of its last backup, that
   backup's size and the space all its backups use, such as `2026-11-19, 980.0 KiB, 3.1 MiB
   used`; hover it for the full sentence. `… used` means the space is still being worked out,
   and `Only safety backups` a game with only the backups a restore makes. The **Sort by** row
   above the list works as on the [Backup tab](backup.md#sort-and-backup-sizes), with a choice
   of its own and without **Changes**, and the figure the list is sorted by comes first on each
   line. **Load backups** waits while a backup runs. Once a **Back up** has saved new backups, a
   loaded list is read again by itself and keeps the game and snapshot you chose.
   A down arrow in a game's row means the newest backup is not the one this PC last synced:
   hover it to read `The newest backup is not the one this PC last synced: from <host>, <date>`.
   See [Which backup this PC is in sync with](#which-backup-this-pc-is-in-sync-with). A game
   with no arrow is one this PC has no record for, or whose newest backup is the one it last
   synced with.
2. Click a game. The **Snapshot** list shows its backups, newest first, as `time, host`. A backup
   made by a restore adds `, pre-restore`. The newest backup that is not a `pre-restore` one is
   preselected.
3. Choose where to restore (see below). A prefix path you type applies when you press Enter or
   leave the field.
4. The **Plan** shows by itself, read from the backup's file list. Nothing is downloaded or
   written yet. Until it shows, a line next to the disabled **Restore files** says why: a snapshot
   or a target to choose, the file list being read, a backup running, or folders to choose (see
   [Cross-OS restore](cross-os-restore.md#the-mapping-dialog)). If reading fails, the error shows
   with **Try again**.
5. Read the **Plan**. Untick what you do not want restored, and fix anything you do not like
   (see "Overrides").
6. Click **Restore files**. salvajocs downloads the backup, then restores it.

## Targets

On Windows, saves go into your own Windows folders. There is nothing to choose.

On Linux you choose the build you want to restore into:

- **Native Linux build**: the XDG folders of this machine.
- **Windows build in a Wine/Proton prefix**: a prefix path, such as `.../compatdata/<id>/pfx`.
  **Browse** opens a folder picker. salvajocs fills in the first prefix it finds of:
  1. the `wine_prefix` set on a [custom game](custom-games.md);
  2. the game's own prefix: a Proton prefix from its Steam id, else its Heroic or Lutris prefix
     ([Found by salvajocs](settings.md#found-by-salvajocs));
  3. the prefix the backup came from. A backup records it by launcher and name (a Proton app id,
     a Heroic app, a Lutris game, a Bottles bottle, or a folder in your home), so another machine
     finds its own copy. When that name fits several prefixes here, such as a bottle of both the
     native and the Flatpak Bottles, it takes the one that holds the backup's save folders;
  4. for a backup of the Windows build, the one prefix that already holds the folders of the
     backup's files in the Windows folders, such as `AppData/Roaming/Godot/app_userdata/<game>`.
     A backup of registry keys only is not looked for.

  A line under the field says why it was filled in, as long as the field still shows that
  prefix. Another line says when the backup came from a prefix this machine does not have, or
  does not use (switched off in Settings, or without a user folder). When the save folders are
  in several prefixes, salvajocs fills in none and names them, so you can type or browse to the
  one to use. It also names them when the backup's prefix name fits several prefixes, none of
  them holds the save folders and no other prefix does either. Choosing another snapshot fills
  the targets in again, unless you changed them.

You can tick both. Each folder in the backup goes into the build it belongs to. Saves of the other
build are mapped onto the one you chose. See [Cross-OS restore](cross-os-restore.md).

The prefix must have a clear user. salvajocs uses the only user folder in `drive_c/users`
(`Public` and `root` do not count). With several, it uses `steamuser`. With several and no
`steamuser`, or with none, it shows an error. Run the game once to create the user folder. Inside
a prefix, folder names match without regard to case, so `My Games` and `my games` never end up
both existing.

## The plan

The Plan shows:

- **Into …**: only the targets the files and registry keys go into. A prefix is named by its
  launcher, such as "the Steam Proton prefix of app 1868140" or "the Bottles bottle
  Generic-Game"; hover it to read its path. While it is the prefix salvajocs filled in, the reason
  follows in brackets, such as "(from the backup)".
- **Files to write (N).** A file tree, as in the plan of
  [several games](#restore-several-games). Each file is under the folder it goes into, named by
  its base, such as `WinAppData/Game`. A file saved as a plain path goes under `Absolute`
  (`WinAbsolute` on Windows). Hover a file's name to read the full path it goes to on this
  machine. Folders start closed; click one to open it. A folder that already exists has a button
  that opens it in the file manager. Each file and folder has a checkbox, all ticked at first,
  as on the [Backup](backup.md) tab: a folder's checkbox ticks or unticks everything below it,
  and its line says how many of its files are ticked, as `1 of 2 files`. Only ticked files are
  restored. A file you untick is not written and not in the safety backup, and the heading
  counts it as left out. N in the heading counts the ticked files. After a file's size, an icon
  says what restoring it does: a plus in a circle when nothing is there yet, a pencil when the
  file there differs and will be overwritten, a question mark when salvajocs cannot read the
  file there to compare it, and a cross in a circle when salvajocs will not write it. Hover it
  to read why. An unchanged file has no icon; it is written all the same. A folder's line counts
  the icons below it, and icons after the heading count the ticked files of each kind. When the
  safety backup cannot read a ticked file at a target, the plan says that nothing will be
  restored and lists those files, and **Restore files** stays disabled. Untick them to restore
  the rest, or make them readable and click **Try again**. With nothing ticked, **Restore
  files** stays disabled too, and a plan with nothing to restore says so. A file with the pencil
  also has a **Details** button, which shows what makes it differ: the backup's copy of the
  chosen snapshot against the file that would be overwritten, as in the
  [Backup tab](backup.md#what-differs-from-the-last-backup).
- **Registry.** The number of ticked registry keys and the prefix, named as on the "Into" line,
  or the Windows registry they go into. Under it, the registry keys as a tree, all ticked at
  first. A key with subkeys is a folder, closed at first, like a folder of files: its checkbox
  ticks or unticks it with every key below it, and its line counts its ticked keys. The first row
  in an open key, **(values)**, ticks only that key itself: its values, and its subkeys that only
  this PC has. A ticked key is replaced: its values and its subkeys that only this PC has are
  deleted, then the backup's are written. A key you untick is left as it is, with its values and
  its subkeys, also the ones only this PC has, unless you tick a key below it. An unticked key
  with no ticked key above it is not in the safety backup; one under a ticked key is, as it was
  before the restore, and so is one above a ticked key this PC does not have yet, so that an
  undo removes the new key (see "Undo"). In a restore of several games, another game that
  declares a key above it still replaces it; that game's safety backup holds it. With every key
  unticked, the files restore even while Wine runs (see below).
- **Skipped (N).** Items that will not be restored, each with its reason.
- A **Cross-build restore** warning when the plan maps folders from the other build. The
  other build may use a different save format.
- **Note** lines, for example about Unity settings. See
  [Cross-OS restore](cross-os-restore.md#the-unity-playerprefs-limit).

**Manifest paths**, beside **Restore files** (or beside **Choose folders…**), opens the save
path templates and registry keys the game has in the Ludusavi manifest or your custom game, such
as `<winAppData>/Game`, with where each applies (system, store) and what it holds. It says where
the entry comes from. Right-click a template and choose **Copy** to copy it.

### Why an item is skipped

| Reason shown | Meaning |
| --- | --- |
| no target on this machine | There is no folder for it here and no mapping applies. |
| game not found under any root | The file lives in the game's install folder and salvajocs cannot find the game. Add the folder in [Settings](settings.md#roots). |
| skipped by choice | You skipped that folder in the mapping dialog. |
| path leaves its base directory | The path would escape its folder, so salvajocs refuses it. |
| outside the game's save roots | The file lies under none of the folders recorded in the backup. |
| save root not declared for this game | The file's folder is not one the game declares. salvajocs treats a snapshot as untrusted and restores only paths and registry keys the game is known to use. |
| registry: no Windows target | The backup has registry keys but you are restoring only into a native Linux build. |
| registry key not declared for this game | The key is not one the game declares. |

### Overrides

Each file has an **Override** button; hover it to read where the file goes now. Click it to
open a field under that file, type an absolute path and click **Apply** to send that one file
somewhere else, for this restore only. The file stays in its folder, with the new path and
`(overridden for this restore)` under it, and its icon compares it with the new target. The
heading counts the overridden files, so you see them while the folders are closed. **Clear**
sends a file back to its planned target. A path that does not fit is refused with an error. If
you typed a path and did not click **Apply**, **Restore files** stays disabled and the tab tells
you to apply or clear it. Clicking **Override** again closes the field and drops what you typed.
An unticked file is not written, overridden or not; **Override**, **Apply** and **Clear** keep
its tick. A new plan, after you change the snapshot or where to restore, starts without
overrides and with everything ticked again.

A folder has an **Override** button too. It opens the folder and a field at its top: type an
absolute path and click **Apply** to send every file in the folder to that path, each keeping its
place below the folder (`sub/save.dat` goes to `<path>/sub/save.dat`). A folder override replaces
the overrides of the files below it. The folder's header then shows the path with a **Clear**
button, which puts every file in the folder back to its planned target. If you then override one
of its files, or a folder inside it, the folder's header no longer shows the path and its
**Clear**, as not every file is there any more; the files show their own targets and **Clear**
buttons. Files you left unticked are not written, and the safety backup holds what is at the
overridden targets of the ticked ones. While salvajocs compares the new targets, **Restore files**
waits.

## What happens when you click Restore files

1. **Download.** salvajocs downloads the game's backup into its cache folder. If the download
   fails, nothing is restored and no safety backup is made.
2. **Safety backup.** Before writing, salvajocs stores the files and registry keys the restore
   would replace, and the files it will delete, as a new snapshot of the same game tagged
   `pre-restore`, in the repository the backup came from. A ticked registry key is stored with
   every key below it, unticked ones included. If this fails, nothing is restored. If nothing
   exists yet at the targets, no safety backup is needed, and the tab says so. If the repository
   already holds the same state, no new snapshot is added.
3. **Files.** Each file is written to a temporary file and moved into place. A target that is a
   symbolic link is never written through.
4. **Registry.** On Windows, the keys are written to the live registry under `HKEY_CURRENT_USER`.
   In a Wine prefix, salvajocs edits `user.reg` directly. For each ticked key it replaces the
   whole subtree, except the keys you unticked and what lies below them.
5. **Deletions.** With **Delete files the backup does not have** on, each ticked file to delete
   is deleted last (see [below](#deleting-files-the-backup-does-not-have)).

The result lists each file as restored or failed, the registry result, and each deletion.

One thing blocks a restore: a restore that includes registry keys for a Wine prefix is refused
while any `wineserver` is running, because Wine would overwrite the change. Nothing is written in
that case. It is checked again after the safety backup and before the first write, so a game
started meanwhile also stops the restore. Close every Wine and Proton game and try again, or
untick the registry keys to restore only the files.

## Deleting files the backup does not have

A restore only adds and overwrites. Games that rotate several auto-saves pile them up this way: a
restore on PC A keeps A's older auto-saves next to the restored ones, and the next Back up stores
the mix. To get this PC's folders to match the backup, tick **Delete files the backup does not
have**.

The box is above the plan, one per game, off at first. salvajocs remembers it for each game on
this PC. When it is on, the plan lists the files it will delete as rows after the files to write,
each with a **will be deleted** mark (hover it to read `Will be deleted: the backup does not have
it`) and a checkbox, ticked at first. Untick a row to keep that file. A folder's checkbox covers
the files to write and the files to delete below it. Hover the box to read what it does. The
safety backup keeps every deleted file, and the plan says so in a line above **Restore files**:
`N files will be deleted. The safety backup keeps them; restoring it brings them back.`

A file is offered for deletion only when all of these hold:

- It is a file, not a folder, in one of the save folders the backup has files for, or in a folder
  below it. Folders themselves are never deleted, and files in other folders are left alone.
- That save folder is not the game's install folder itself, nor a whole base folder such as your
  home folder or `AppData`. A save folder inside the install folder can be cleaned.
- The game's save paths cover it. A game that saves `*.sav` files never loses a `notes.txt`.
- The backup does not have it, and this restore does not write it.
- It is not unticked in what to back up, neither in today's choices on the
  [Backup tab](backup.md#choose-and-back-up) nor in the choices recorded in the backup, which
  Back up now records. A file you excluded from backups is kept, even on a PC that never backed
  it up. An older backup records none, so only today's choices count.

A whole game unticked on the Backup tab still deletes as usual: unticking a game only stops its
backups. Untick rows to keep files.

The box is greyed out, with the reason on hover, when:

- you restore a `pre-restore` safety backup, since it holds only what a restore replaced;
- what to back up of the game cannot be read, so no file is deleted;
- this PC has no file the backup lacks in the backup's folders.

A folder that cannot be listed deletes nothing, and the plan says so. A file that the
restore writes, for example after an **Override**, is kept, and its row says `kept: the restore
writes this file`. Files below a folder override are kept too: `kept: its folder is overridden for
this restore`.

When you click **Restore files**, the restore reads what to back up again, just before it runs.
A file unticked on the Backup tab since the plan was made is kept, and the result says `<path>:
kept: unticked in what to back up since the plan`. If it cannot read it, or the Backup tab could
not save its last change, nothing is deleted and the result says so.

The restore stores the files to delete in the safety backup first. If one cannot be read, nothing
is restored, as for any file it would overwrite. It deletes a file only if it is still the one
that was stored: a file changed since is kept and reported as failed. The result lists each
deletion as `<path>: deleted` or `<path>: failed: …`. A failed deletion also stops this PC from
recording the restore as synced, as a failed write does.

To bring the deleted files back, restore the game's newest `pre-restore` backup with everything
ticked. Files the first restore added stay (see [Undo](#undo)).

In a restore of [several games](#restore-several-games), the plan has the box for each game, and
**Select all** and **Deselect all** after "Delete files the backups do not have:" turn it on or
off for every planned game, remembered like the box.

## Restore several games

![The plan of two games restored together](../images/restore-batch.png)

To restore more than one game in one go:

1. Tick the checkbox before each game in the list, or click **Select all**. A game whose only
   backups are `pre-restore` ones cannot be ticked. Drag the right edge of the list to widen it;
   a name that is cut off shows in full when you hover it.
2. Click **Restore N games**. Each ticked game uses its newest backup that is not a `pre-restore`
   one, and the targets salvajocs fills in for it (see "Targets"). To choose another snapshot or
   target, or to override a file, restore that game on its own.
3. If a game's backup holds folders of the other OS's build that salvajocs cannot place, the
   mapping dialog opens for it, one game after another. See
   [Cross-OS restore](cross-os-restore.md). The choices you confirm are saved with the game, as in
   a single restore. The prefix option is not offered here. **Skip this game** leaves the game
   out and saves nothing. **Cancel** stops the whole batch, and nothing is restored; folder
   choices you already confirmed for earlier games stay saved.
4. Read the **Plan**. For each game it shows the snapshot, where the files go, each file that
   will fail, and the same warnings, registry and skipped items as a single game's plan. Games
   that will not be restored are listed with the reason. Nothing is written yet. At the end of the
   snapshot's line, icons count the files that are new (a plus in a circle: nothing there yet),
   changed (a pencil: the file there differs), unchanged (an equals sign) or could not be
   compared (a question mark). Counts of zero are
   left out; hover the icons to read the counts as text.

   Under it, a file tree lists every file the game writes, by the folder it goes into on this
   machine, with a checkbox on each file and folder as in a single game's plan, and the tree of
   registry keys with a checkbox on each key and subkey, also as there. Only what is ticked is
   restored. A file has an icon after
   its size: a plus in a circle when nothing is there yet, a pencil when the file there differs
   and will be overwritten, a question mark when salvajocs cannot read the file there to compare
   it, and a cross in a circle when salvajocs will not write it. Hover the icon to read
   `New: not on this PC yet`, `Changed: will be overwritten`, `Could not be compared` or
   `Will fail` with the reason. An unchanged file has no icon; it is written all the same. The
   icons compare with the backup's hash list (size and modification time for backups made
   before 0.6.0), as the files are when the plan is made. The plan downloads no backup. A
   folder's line counts the icons below it. A folder that already exists has a button that
   opens it in the file manager.

   A game with nothing ticked says so and is not restored; tick something to bring it back. A
   game whose plan has nothing to restore says so and is left out.
   When the safety backup cannot read a ticked file of a game, the plan lists those files and
   the game is not restored until you untick them; the rest of that game is then restored.
   **Restore N games** counts the games that will be restored.
5. Click **Restore N games**. salvajocs downloads every game's backup first, then restores the
   games one after another. A game whose backup cannot be downloaded writes nothing.

Each game is restored as described above, with its own safety backup, and the next game goes on
whatever happens to it. A game whose restore is refused writes nothing: for example, when its
restore includes registry keys for a Wine prefix while a `wineserver` runs, or when its safety
backup fails. The result shows `Restored N of M games`, then each game's lines. A game that was
not refused counts as restored even if some of its files failed; those files are listed under it
as failed.

## Which backup this PC is in sync with

Each PC remembers the last synced backup of each game: see
[Syncing PCs](backup.md#syncing-pcs). A restore sets it to the backup you restored, when no file
failed. An undo is a restore too, so it sets it to the safety backup you restored. This memory is
also what puts the down arrow in the game list: the newest backup is not the last synced one.

After each restore, salvajocs also stores a list of this PC's files for the game, as Back up
would list them. It needs the list because this PC can differ from the backup it restored: it may
be another kind of PC, the restore may not have touched some files, and cleanup may remove the
backup later. With the list, the [Backup tab](backup.md) can tell what changed on this PC after
the restore.

Files the restored backup does not have stay on your disk. That includes a save this PC never
backed up. Right after the restore, they show as new on the Backup tab, and the next Back up
stores them together with the restored saves. If another PC backs up first, the game shows
behind and Back up skips it. Then these files stay on this PC but are in no backup until this
PC's saves change or you press **Back up anyway**. To remove such files at restore time, see
[Deleting files the backup does not have](#deleting-files-the-backup-does-not-have).

## Undo

The safety backup is the undo. In the **Snapshot** list, choose the newest entry marked
`pre-restore` for that game, check its plan, and restore it.

An undo is a restore like any other. This PC is then in sync with the safety backup, and the
newest backup is a different one. So after an undo the game reads behind, with the arrow on the
Backup tab, until this PC's saves change. Then Back up asks. If the saves the undo brings back
are the newest backup's, the game is up to date instead.

An example, with PCs A and B:

1. B backs up newer saves. On A, you restore them. A first saves its own files as a safety
   backup.
2. On A, you undo the restore: you restore the safety backup. A has its old saves again.
3. On A, the Backup tab shows the game with the down arrow: a newer backup exists, and A has not
   changed its saves since it last synced. Back up does not store it. Until you play or press
   **Back up anyway** under the game, A's saves are only in the safety backup. Pressing **Back up
   anyway** makes A's saves the newest backup. If you play on A first, Back up asks what to do.

An undo has limits:

- It puts back what the restore overwrote. Files the restore created are not in the old state,
  so they stay. A registry key the restore created below a key that already existed is removed
  by the undo; a ticked key that did not exist before stays, with everything the restore wrote
  below it.
- Files and registry keys you unticked were not touched, so they are not in the safety backup.
  Two kinds of unticked registry key are the exception: one under a ticked key, and one above a
  key the restore created. They are in the safety backup, so the undo puts them back as they were
  before the restore. Untick them in the undo's plan to leave them as they are now.
- A safety backup of a file that you redirected with **Override** is stored by its full path. Such
  files show up as skipped when you restore that snapshot through the normal flow.

## Deleting backups

To delete one backup, choose it in the **Snapshot** list and click **Delete snapshot**. To delete
every backup of the chosen game, click **Delete all backups of …**, which names the game. A
`pre-restore` backup can be deleted like any other. Both buttons, and the dialog's **Delete**,
wait while a backup runs.

A dialog names the game and lists the backups that go, as in the **Snapshot** list: `time, host`,
plus `, pre-restore` for a safety backup. Deleting removes them from the repository for every
computer that uses it, and cannot be undone. **Cancel** has the keyboard focus, and Escape or a
click outside the dialog also cancels. Click **Delete** to go ahead.

Deleting all backups of a game keeps your file choices for it (see
[Backup](backup.md#what-is-stored)). The buttons wait while the game manifest loads, the targets
are detected, or a restore runs. When the delete is done, the tab says how many
backups it removed and lists the games again; the game stays chosen if it still has backups. A
restore lists the games again the same way and keeps its game chosen. If a delete stops partway,
the error says how many were removed before it.

As with [old backups](settings.md#old-backups), the disk space comes back at a later cleanup, at
least 23 hours later. In an append-only repository nothing is deleted, and the tab says so.

## Next

Restoring a Linux save on Windows, or the other way round: [Cross-OS restore](cross-os-restore.md).
