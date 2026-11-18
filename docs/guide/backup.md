# Backup

The Backup tab scans this machine for games with saves and stores the ones you choose in the
repository, one snapshot per game. This page explains the scan, the choices and the result. Back
to the [guide index](README.md).

![The Backup tab](../images/backup.png)

## Where the game list comes from

salvajocs uses the [ludusavi manifest](https://github.com/mtkennerly/ludusavi-manifest), a public
list of about 52,000 games (entries that are not aliases, measured in November 2026) and where
each one keeps its saves. salvajocs downloads it when it starts and keeps a copy in the cache
folder. The top right of the window shows the number of games loaded.

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
- a folder cannot be read.

A game with unreadable paths shows how many, because its backup may be incomplete.

## Choose and back up

Each game found has a checkbox and a line such as `3 files, 1.2 MiB, plus registry keys`. A
game's checkbox says whether it is backed up at all. New games start selected. **Select all** and
**Deselect all** change every game at once.

Open the line to see the game's files as a tree of folders, each with a checkbox, and its registry
keys under **Registry**:

- Unchecking a folder leaves out every file below it; checking it takes them all back. A folder
  with only some files checked shows a dash.
- The line counts only the files that are backed up, and says how many are excluded, as in
  `2 files, 1.1 MiB, 1 excluded`.
- A game with every file and key unchecked gets no snapshot.
- Below Windows folders, names that differ only in upper and lower case are one folder.

The choices are kept in the repository, so every machine that uses it shares them. A click is
not stored there right away: it waits in `pending-selection.json` in the config folder, which
keeps it if the app closes, and **Back up** saves it to the repository first. Other machines see
it after that backup. The checkboxes stay greyed out until the repository is open and the choices
are loaded from it. If a game's choices cannot be read, for example because a newer salvajocs
wrote them, the tab says so and backs up all of that game.

Click **Back up N games**. The tab saves your changed choices to the repository and reads every
game's choices again, so a choice another machine saved since counts. It then scans again, so a
save written since the last scan is included, backs up the games one by one, then removes old
backups (see [What is stored](#what-is-stored)). It shows the result per game under
**Last backup**:

| Result | Meaning |
| --- | --- |
| backed up | The save changed and a snapshot was stored. |
| up to date | The save is the same as in the last snapshot, so nothing was stored. |
| failed | The game could not be backed up. The reason follows. |

The repository removes duplicate data, so repeated backups of a large save that changed little
cost little space.

## What is stored

Each game gets its own snapshot, tagged `salvajocs` and `game:<slug>`, with the game's title as its
label. Files are stored relative to named folders (for example `WinDocuments` or `XdgConfig`), not
as absolute paths of this machine. That is why a save can be restored on another machine or
operating system. See [Cross-OS restore](cross-os-restore.md).

After each backup, even one where a game failed, salvajocs removes the old backups that the
[Old backups](settings.md#old-backups) rules let go, never the ones this backup just made, and
shows **Removed N old backups**. With no rule set, snapshots accumulate, and deduplication keeps
the size small. If the rules cannot be read, or the repository is append-only, nothing is removed
and a warning says so. If the cleanup stops partway, **Cleanup did not finish** says why, and
**Removed N old backups** counts those that are gone, even if it stopped while removing them;
this backup's snapshots are stored all the same.

A server that makes the repository append-only by itself, such as `rest-server --append-only`,
does not mark the repository as append-only, so salvajocs cannot tell. The cleanup then fails
after every backup that has old backups to remove. Turn the rules off in Settings for such a
repository.

A game's choices of what to back up are a separate small snapshot, tagged `salvajocs-settings`
and `settings:<slug>`, which the restore list does not show. Saving new choices forgets the
game's older choices snapshots.

## Next

[Restore a backup](restore.md).
