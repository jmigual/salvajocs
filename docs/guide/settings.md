# Settings

The Settings tab changes the repository, the compression level and the folders salvajocs looks
in for games. This page covers each section. Back to the [guide index](README.md).

![The Settings tab](../images/settings.png)

## Repository

The top of the tab shows the current repository and the encryption password mode (**None**,
**Stored in the system keyring** or **Asked once per session**). Below it is the same form as on
[first run](first-run.md): backend, location or service fields, advanced options, encryption password, **Remember in the system
keyring**, and compression for a new repository.

Click **Apply** to connect to a different repository. salvajocs checks it the same way as on
first run, and asks before it creates a new one. The tabs then show the new repository.

Keyring entries belong to one repository and its non-secret options. After you switch to another
repository, the tab offers **Delete entries** to remove the old ones, or **Keep entries**.

## Compression

The **Compression** dropdown shows the level of the open repository. Change it and click **Apply
compression**. The entries are the same as on [first run](first-run.md#compression). A level
without its own entry, such as 5, shows as **Custom level** with the number filled in.

The level is a property of the repository, so the change affects every machine that writes to it.

## Roots

Roots tell salvajocs where games live on this machine. Add the ones you use. Each root has a
kind:

| Kind | Use it for | Effect |
| --- | --- | --- |
| Steam library | A Steam library folder, the one that holds `steamapps`. | Finds games in `steamapps/common/<install folder>` and Proton prefixes in `steamapps/compatdata`. |
| Games folder | A folder that holds game install folders. | Finds games in `<folder>/<install folder>`. |
| Wine prefix | A Wine prefix, for example `~/.wine`. | Every game is checked against the prefix. |

To add one, choose a **Kind**, type or **Browse** for the **Root folder**, and click **Add root**.
Click **Remove** to delete one. The changes are saved at once.

### Steam libraries

salvajocs finds the default Steam folders by itself: `~/.steam/steam`, `~/.local/share/Steam` and
the Flatpak Steam folder on Linux, and `Program Files (x86)\Steam` on Windows. A folder counts
only if it contains `steamapps`.

salvajocs does not read Steam's list of library folders. If your games are on another drive, add
that library as a **Steam library** root. If Steam lives somewhere salvajocs does not look, for
example because `$HOME` points elsewhere in a container, add its folder too.

### Wine prefixes

A **Wine prefix** root covers a prefix that does not belong to a Steam game, such as the default
`~/.wine`. It must contain `drive_c/users` and one of `system.reg`, `user.reg` or `dosdevices`.
Prefixes of Heroic, Lutris and Bottles are not found by themselves. Add them here, or set the
**Wine prefix** of a [custom game](custom-games.md).

When several prefixes match a game, the game's own prefix (its Proton prefix, or its custom
`wine_prefix`) comes first, then the Wine prefix roots in the order you added them.

## Updates

On Windows, **Check for updates at start** (on by default) asks GitHub for a newer release each
time salvajocs starts. Untick it to stop the check. Other systems do not show this setting.

## Where settings are stored

All settings except the compression level are in `config.yaml`, listed in
[Installation](installation.md#where-salvajocs-keeps-its-files). The compression level is in the
repository; passwords and secret options are in the keyring or asked each session.
