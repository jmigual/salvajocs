# Backup

The Backup tab scans this machine for games with saves and stores the ones you choose in the
repository, one snapshot per game. This page explains the scan, the choices and the result. Back
to the [guide index](README.md).

![The Backup tab](../images/backup.png)

## Where the game list comes from

salvajocs uses the [ludusavi manifest](https://github.com/mtkennerly/ludusavi-manifest), a public
list of about 52,000 games (entries that are not aliases, measured in November 2026) and where
each one keeps its saves. salvajocs downloads it when it starts and keeps a copy in the cache
folder. The bar along the bottom of the window shows the number of games loaded.

- The first start needs network access to get any game. Without a cache and without network, only
  your [custom games](custom-games.md) are scanned, and the tab says so.
- The scan and backup buttons wait while the manifest is loading.

## Scan

Click **Scan for games**. The scan checks, for every game in the manifest and every custom game:

- the save folders of this machine (native Windows folders on Windows, the XDG folders on Linux);
- on Linux, the Proton prefix of each Steam game, under every Steam root salvajocs finds
  (`~/.steam/steam`, `~/.local/share/Steam` and the Flatpak Steam folder) and under every Steam
  library you added in [Settings](settings.md#roots);
- on Linux, every Wine prefix you added in Settings, and the `wine_prefix` of each custom game;
- on Linux, the Wine prefixes of Heroic, Lutris and Bottles that are ticked in
  [Settings](settings.md#found-by-salvajocs). A Heroic or Lutris prefix is checked for its own
  game; the others for every game;
- the registry keys of the game under `HKEY_CURRENT_USER`: the live registry on Windows, or
  `user.reg` inside the prefix on Linux. Other registry hives are not backed up.

Inside a Wine prefix, folder names match without regard to upper and lower case, because Windows
games do not care about it.

The scan runs in the background. Checking the whole manifest can take several seconds.

### Scan problems

Problems are listed in yellow above the games and do not stop the scan:

- the Wine prefix has several users and none is `steamuser`;
- the Wine prefix has no user yet (run the game once);
- the folder is not a Wine prefix;
- a folder cannot be read;
- a Heroic, Lutris or Bottles file cannot be read.

A game with unreadable paths shows how many, because its backup may be incomplete.

## Choose and back up

Each game found has a checkbox and a line such as `3 files, 1.2 MiB, plus registry keys`. A
game's checkbox says whether it is backed up at all. New games start selected. **Select all** and
**Deselect all** change every game at once.
A game found in a Wine or Proton prefix shows it as `Wine prefix: <folder>`, one line per prefix
its files or registry keys came from.

### Sort and backup sizes

Once the comparison below has listed the backups, each game shows a line such as
`Last backup 2026-11-19 14:02 UTC, 980.0 KiB. All backups use 3.1 MiB.`:

- The last backup is the newest one from any machine, not counting the safety backups a restore
  makes. Its size is that of its files before compression; it also counts the few small files
  salvajocs adds, so it can read a little larger than the files found here.
- All backups use is the space the game's backups take in the repository, compressed, safety
  backups included. A file two backups share counts once. Data that another game's backups also
  use counts for both games, so the figures of all games can add up to more than the repository
  holds. Data a removed backup left behind until the next cleanup is not counted.
- `…` means the space is still being worked out, and `size unknown` a backup made by a tool that
  does not record its size. A game without a backup says `No backup.`, or how much space its
  safety backups use.

The **Sort by** row above the list orders the games by **Name**, **Last backup**, **Last backup
size**, **Space used** or **Changes**; hover an item to read what it means. The arrow button
next to it reverses the order and says the current one, such as **Largest first**. Picking a key
starts in its natural order: names A to Z, times newest first, sizes largest first, most changes
first. Games without the value, such as one without a backup, stay at the end either way. The
figure the list is sorted by is drawn brighter on each line (for **Changes**, the count in the
game's header). The choice is saved, and the Restore tab keeps its own.

**Changes** counts the files the next backup would add, change or remove compared with the
newest backup, the number each game's header shows. A game without a backup yet counts all its
files, and a game with no changes counts 0. A game whose comparison has not arrived, has failed
or is not backed up stays at the end. The order is built when the comparisons arrive, so
ticking or unticking a file does not move a game; reverse the order, pick another key, scan or
back up to take the ticks into account. The Restore tab has no **Changes**.

Open the line to see the game's files as a tree of folders, each with a checkbox, and its registry
keys under **Registry**:

- Unchecking a folder leaves out every file below it; checking it takes them all back. A folder
  with only some files checked shows a dash.
- A folder that holds only another folder is shown on one line with it, as `Home/.var/app`. The
  checkbox covers the whole line, and the button opens the last folder.
- The line counts only the files that are backed up, and says how many are excluded, as in
  `2 files, 1.1 MiB, 1 excluded`.
- A game with every file and key unchecked gets no snapshot.
- Below Windows folders, names that differ only in upper and lower case are one folder.
- The button at the end of a folder's line, **Show … in the file manager**, opens that folder. A
  folder whose files come from two places, such as two prefixes or install folders, opens the
  first.

### What differs from the last backup

After each scan, the tab compares every game with its newest backup from any machine that uses
the repository, leaving out the safety backups made before a restore. While it runs, the list
says **Comparing with the last backups**. Each game's line then ends in icons, each with the
number of files it counts. Counts of zero are left out. Hover the icons to read the full sentence,
such as `1 new, 2 changed since the backup from <host>, <date>`.

| Icon | Meaning |
| --- | --- |
| Plus in a circle | New: files the backup does not have. |
| Pencil | Changed: files whose content differs from the backup. |
| Minus in a circle | Removed: files in the backup that the next backup will not hold. |
| Question mark | Files that could not be read to compare them. |
| Check mark in a circle, no number | No backup will be made. Either nothing changed since that backup, or nothing changed since this PC's own last backup, though another machine's newer backup differs. |
| Equals sign, no number | No changes since that backup, but a backup is still made: that backup has no list of hashes (it was made by salvajocs 0.5.0 or older), or it is from the other build and one side has registry keys. |
| Plus in a circle, no number | No backup yet: the repository has no backup of the game. |

If the backups could not be read, the game says `Could not compare: <error>` under its title.

A game that is not backed up, because it is unchecked or has nothing checked, gets no icons.

In the file tree, a file that differs from the backup has one of these icons after its size.
Hover it to read `New: not in the backup`, `Changed since the backup`,
`Removed: the next backup will not hold it` or `Could not be compared` with the reason. When the
backup has no list of hashes, the changed tooltip adds `by modified time`. A file
that is in the backup but not found any more, or that you unchecked, is removed: files no longer
found are greyed rows without a checkbox. A folder's line counts the icons of the files below
it. A game with unreadable paths removes nothing, since its backup keeps the files it could not
check.

A file counts as changed by its content: a different size is a change; the same size and
modification time is not; otherwise its SHA-256 is compared with the list of hashes the backup
holds. A backup made by salvajocs 0.5.0 or older has no such list, and a list that does not match
its backup's files is not used, so there the modification time decides.

To see what makes a file differ, click the **Details** button after a file with the changed
icon. The dialog shows, for this PC's copy and the backup's, where the file is (the backup's path
below `files/`, which for a backup of the other build is its own folder), its size, its
modification time to the second (UTC) and its SHA-256, which you can select and copy. The
backup's hash comes from its list of hashes; a backup without one gets it from the downloaded
file.

A text file also shows the lines that differ, with three lines of context: lines of the backup
marked `−`, lines on this PC marked `+`. Windows ends a line with CRLF and Linux with LF, so the
dialog counts each copy's line endings (`2 Windows (CRLF), 0 Linux (LF)`) and compares the lines
with every line ending read as LF and a byte order mark (BOM, which some Windows editors write
at the start of a file) left out; a copy with a BOM says so. When that leaves no difference, it
says that only the line endings, the BOM or both differ. A last line without a line ending is
marked `(no newline at end)`.
A copy is text when it is valid UTF-8 without a NUL byte. When a copy is not text, or either copy
is larger than 10 MiB, no lines are shown, and nothing is downloaded from the repository for a
file that large, until you click **Show anyway**, which reads both copies without that limit. A
file that is not text still shows no lines. A long diff shows its first 1000 lines. The backup's
copy is read when you click the button, so the repository must be reachable; if it is not, the
dialog says it could not read the backup.

The comparison leaves out the registry keys and the list of save folders, so a game shown as
unchanged is still backed up when only those changed.

The newest backup may come from the other build of the game: the native Linux build on one
machine and the Windows build (native, or in Wine or Proton) on another. Its folders are then
matched to this machine's the way a [cross-OS restore](cross-os-restore.md#how-a-folder-is-matched)
matches them, without asking anything and without saving anything, and the files are compared
in their matched places. The game's tooltip says that the backup is from the other build. A
folder a restore would have to ask about, or one you skipped, has no known place here: its files
show as removed, with a tooltip saying that the newest backup is from the other build and that
the file has no place here, and the game's tooltip names the folders. The same goes for a file
whose matched path another file of the backup already takes. When a restore would ask about a
folder, the game also shows **Choose folders…**: it opens the restore's
[mapping dialog](cross-os-restore.md#the-mapping-dialog), saves your choices with the game just
as a restore does, and compares again. Across builds the
registry keys are not compared either, so a game with registry keys on either side shows the
equals sign and is backed up.

When no backup is made because the files equal the other build's newest backup, this machine's
saves live only in that backup. Restoring them here then needs the folder matching to still be
known (keep the pair or row, or answer the mapping dialog again), and on Linux the native build
as the target, not a Proton prefix.

The choices are kept in the repository, so every machine that uses it shares them. A click is
not stored there right away: it waits in `pending-selection.json` in the config folder, which
keeps it if the app closes, and **Back up** saves it to the repository first. Other machines see
it after that backup. The checkboxes stay greyed out until the repository is open and the choices
are loaded from it. If a game's choices cannot be read, for example because a newer salvajocs
wrote them, the tab says so and backs up all of that game.

After a scan, the bar along the bottom of the window shows the repository, with a lock that says
whether it is encrypted, and **Back up N games** at its right end. During a backup the bar shows
its progress in place of the repository.

Click **Back up N games**. The tab saves your changed choices to the repository and reads every
game's choices again, so a choice another machine saved since counts. It then scans again, so a
save written since the last scan is included, backs up the games one by one, then removes old
backups (see [What is stored](#what-is-stored)). It shows the result per game under
**Last backup**:

| Result | Meaning |
| --- | --- |
| backed up | The save changed and a snapshot was stored. |
| up to date | The save is the same as in this machine's last snapshot, so nothing was stored. |
| up to date with this PC's last backup; the newest backup, from `<host>`, differs | Nothing changed since this machine's last snapshot, so nothing was stored, but another machine's newer snapshot has other files. |
| same as the newest backup, from `<host>` | Another machine's newest snapshot holds the same files, for example after a launcher's cloud sync, so nothing was stored. |
| failed | The game could not be backed up. The reason follows. |

The repository removes duplicate data, so repeated backups of a large save that changed little
cost little space.

### Custom games and rules

Changes to your [custom games](custom-games.md) and to the
[Old backups](settings.md#old-backups) rules stay on this PC until a Back up. When there are
some, the bar and its button show even before the first scan. Back up first checks the repository
("Checking the repository"), then saves them ("Saving the custom games and rules to the
repository") before your choices, the scan and the games, and shows **Saved N custom games to the
repository** and **Saved the Old backups rules**. With no game ticked, the button reads **Back
up** and saves only these changes: nothing is scanned, backed up or removed.

If another PC changed one of them since this PC's change started, the dialog "Changed on another
PC" asks which version to keep ([Custom games](custom-games.md#changed-on-another-pc)). **Cancel**
stops the Back up, and nothing is written. A change that cannot be saved stays on this PC, the
results say why, and the next Back up tries it again.

## What is stored

Each game gets its own snapshot, tagged `salvajocs` and `game:<slug>`, with the game's title as its
label. Since 0.6.0 it also holds `hashes.json`, the SHA-256 and size of every saved file, which
the comparison above and the "same as the newest backup" check read. The first backup of each
game after updating to 0.6.0 therefore makes a new snapshot. Files are stored relative to named
folders (for example `WinDocuments` or `XdgConfig`), not as absolute paths of this machine. That
is why a save can be restored on another machine or operating system. See
[Cross-OS restore](cross-os-restore.md).

After each backup, even one where a game failed, salvajocs removes the old backups that the
[Old backups](settings.md#old-backups) rules let go, never the ones this backup just made, and
shows **Removed N old backups**. With no rule set, snapshots accumulate, and deduplication keeps
the size small. If the rules cannot be read, or the repository is append-only, nothing is removed
and a warning says so. If this PC's changed rules could not be saved, nothing is removed either,
since the rules in the repository may keep fewer backups: "The Old backups rules were not saved,
so no old backup was removed". If the cleanup stops partway, **Cleanup did not finish** says
why, and **Removed N old backups** counts those that are gone, even if it stopped while removing
them; this backup's snapshots are stored all the same.

A server that makes the repository append-only by itself, such as `rest-server --append-only`,
does not mark the repository as append-only, so salvajocs cannot tell. The cleanup then fails
after every backup that has old backups to remove. Turn the rules off in Settings for such a
repository.

A game's choices of what to back up are a separate small snapshot, tagged `salvajocs-settings`
and `settings:<slug>`, which the restore list does not show. Saving new choices forgets the
game's older choices snapshots.

## Next

[Restore a backup](restore.md).
