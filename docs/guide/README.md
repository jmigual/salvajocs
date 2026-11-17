# salvajocs user guide

This guide explains how to install salvajocs, set up a backup repository, back up your game saves,
and restore them on the same machine or on another one, including from Linux to Windows and back.
It describes the app as it is today. For the project overview, go back to the
[main README](../../README.md).

| Page | What it covers |
| --- | --- |
| [Installation](installation.md) | Install salvajocs on Linux or Windows: AppImage, Windows zip, cargo or Flatpak. |
| [First run](first-run.md) | Choose where backups are stored, the compression level and the encryption password. |
| [Backup](backup.md) | Scan for games and back them up. |
| [Restore](restore.md) | Pick a snapshot, check the preview, restore, and undo. |
| [Cross-OS restore](cross-os-restore.md) | Move saves between Wine/Proton on Linux and Windows. |
| [Custom games](custom-games.md) | Add a game the manifest does not know, or fix one it knows. |
| [Settings](settings.md) | Change the repository, the compression level, the game folders and the Wine prefixes. |
| [Using the repository with restic](restic-interop.md) | Read the repository with the restic command line. |
| [Troubleshooting](troubleshooting.md) | Common problems, with cause and fix. |
| [Releasing](releasing.md) | For maintainers: publish a version. |

New to salvajocs? Read [Installation](installation.md), then [First run](first-run.md), then
[Backup](backup.md).

## The four tabs

The window has four tabs. Each page above covers one of them.

| Tab | Use it to |
| --- | --- |
| Backup | Scan this machine for games and back them up. |
| Restore | List backups from every machine and restore one. |
| Custom games | Add games and edit their paths, pairs and skipped folders. |
| Settings | Change the repository, the compression level and the roots. |

The top right of the window shows how many games the manifest holds. The manifest is the list of
games and save locations that salvajocs uses. See
[Backup](backup.md#where-the-game-list-comes-from).
