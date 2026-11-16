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

Each game found has a checkbox and a line such as `3 files, 1.2 MiB, registry: no`. Open the
line to see the files. New games start selected. **Select all** and **Deselect all** change
every game at once.

Click **Back up selected (N)**. The tab scans again first, so a save written since the last scan
is included. It then backs up the games one by one and shows the result per game:

| Result | Meaning |
| --- | --- |
| new snapshot | The save changed and a snapshot was stored. |
| unchanged | The save is the same as in the last snapshot, so nothing was stored. |
| failed | The game could not be backed up. The reason follows. |

The repository removes duplicate data, so repeated backups of a large save that changed little
cost little space.

## What is stored

Each game gets its own snapshot, tagged `salvajocs` and `game:<slug>`, with the game's title as its
label. Files are stored relative to named folders (for example `WinDocuments` or `XdgConfig`), not
as absolute paths of this machine. That is why a save can be restored on another machine or
operating system. See [Cross-OS restore](cross-os-restore.md).

salvajocs never deletes snapshots. They accumulate, and deduplication keeps the size small.

## Next

[Restore a backup](restore.md).
