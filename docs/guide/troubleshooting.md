# Troubleshooting

Common problems, with cause and fix. Back to the [guide index](README.md).

## The restore says a wineserver is running

The message is "a wineserver is running: close every Wine and Proton program first". A restore that
includes registry keys for a Wine prefix edits `user.reg`. Wine keeps the registry in memory and
saves it again later, which would overwrite the change. salvajocs refuses and writes nothing.

Close every Wine and Proton game, then restore again. salvajocs checks for any `wineserver`
process on the machine, not only the one of the prefix.

Backing up is not blocked. What the scan reads from `user.reg` can be up to 30 seconds old if a
game is running.

## Wrong encryption password

The prompt shows "Wrong encryption password." Type the encryption password again. If you stored it
in the keyring and later changed it, the prompt asks again and stores the new one.

If you lost the encryption password, the repository cannot be opened. Without it the data is
unreadable.

## "The repository could not be opened"

The red message in the window includes the reason: a folder that is unreadable, a network error,
or bad credentials for a remote backend. Fix the cause and click **Retry**. To point salvajocs at
another repository, use [Settings](settings.md#repository).

"The server refused the login. Check the user name and password." means the storage server
rejected the login of your server or storage account, not the encryption password. Correct the
login fields, such as **Server password**, in [Settings](settings.md#repository) and click
**Apply**.

## "No repository at ..."

The repository in the config does not exist any more: it was deleted or moved, or its drive is not
mounted. If it is on a drive, mount it and click **Retry**. Otherwise click **Open Settings**,
then in **Repository** keep the location to create a new repository there, or enter another
location, and click **Apply**. salvajocs does not create a repository unless you confirm it.

## The keyring does not work

salvajocs asks for the password every time you start it. This is expected on Linux without a
Secret Service daemon (such as GNOME Keyring or KWallet). A warning "Could not store a secret in
the keyring" means the store refused the value. The Windows Credential Manager rejects values over
2560 bytes. In both cases the password mode becomes "asked once per session".

## The scan finds no games

- Check the bottom left of the window. "Manifest: loading" means wait. "Manifest: not available"
  means the game list could not be downloaded and there is no cached copy. Only
  [custom games](custom-games.md) are scanned until the download works. The first start needs
  network access.
- Steam may be somewhere salvajocs does not look. It looks in `~/.steam/steam`,
  `~/.local/share/Steam` and the Flatpak Steam folder. If `$HOME` is not where Steam lives (as in
  some containers), add the Steam folder as a **Steam library** in
  [Settings](settings.md#steam-libraries).
- Games on another drive or library need that library added as a **Steam library**. salvajocs does
  not read Steam's list of libraries.
- Non-Steam games, and Wine games that Steam does not manage, need a prefix. Add a **Wine prefix**
  in Settings, or set the prefix on a [custom game](custom-games.md). Heroic, Lutris and Bottles
  prefixes are not found by themselves.
- A game that has never been run has no save files yet.

## Scan problems in yellow

| Message | Cause and fix |
| --- | --- |
| the Wine prefix has several users (...) and none is steamuser | The prefix has several user folders in `drive_c/users`, and salvajocs cannot tell which is yours. It accepts several only when one is named `steamuser`. |
| the Wine prefix has no user yet | The game never ran in this prefix. Start it once. |
| not a Wine prefix | The folder lacks `drive_c/users`, or `system.reg`, `user.reg` and `dosdevices` are all missing. Check the path. |
| cannot be read | The OS refused access. Fix the permissions. |

## Unreadable folders

The line under a game says "N unreadable paths (the files may be incomplete)" when a folder could
not be read, for example because of permissions. The backup still runs with the files that could be
read. Fix the permissions and back up again.

## A file or the game is skipped on restore

The restore plan lists each skipped item with its reason. The common causes are:

- "game not found under any root": the file belongs in the game's install folder and salvajocs
  cannot find the game. Add the Steam library or games folder in
  [Settings](settings.md#roots).
- "no target on this machine": there is no prefix, so the files of the Windows build have nowhere
  to go. Choose a prefix in [Restore](restore.md#targets), or use the mapping dialog for
  [Cross-OS restore](cross-os-restore.md).
- "registry: no Windows target": the registry keys need a Windows or Wine target. Restore into a
  prefix to include them.

## The prefix has no user, or has several

On restore, an error mentions "no user directory in prefix" or "several users in prefix". Run the
game once in that prefix to create its user folder. A prefix with several users works only if one
is named `steamuser`.

## sftp is missing on Windows

The `sftp` service of the OpenDAL backend is not built for Windows. The backend list on Windows
does not show it, and the dialog says so. A repository on `opendal:sftp` therefore cannot be used
from a Windows build. Pick a backend that both systems have, such as a REST server, rclone or
another cloud service.

## The Unity settings did not come across

Unity keeps its settings (`PlayerPrefs`) in the registry on Windows and in a file on Linux.
salvajocs does not convert them. See
[Cross-OS restore](cross-os-restore.md#the-unity-playerprefs-limit).

## A restore went wrong

Restore the snapshot tagged `pre-restore` for that game. See [Restore](restore.md#undo).
