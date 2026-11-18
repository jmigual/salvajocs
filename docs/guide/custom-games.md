# Custom games

A custom game is a game you describe yourself: its save files, its registry keys and, if needed,
its Wine prefix. Use one when the manifest does not know a game, when it has a wrong path, or to
store the choices of a [cross-build restore](cross-os-restore.md). Back to the
[guide index](README.md).

![The Custom games tab](../images/custom-games.png)

The tab lists your custom games. Each line says how the game relates to the manifest: a custom
game, a game that extends the manifest entry, or one that overrides it. **Edit** and **Remove**
change the list. **Add game** opens the form. Changes are saved to the config at once.

## The form

| Field | What to enter |
| --- | --- |
| Name | The game's title. The name is how the game is matched with the manifest, so spell it exactly as the manifest does for an existing game. Two custom games cannot share a name. |
| Integration | **Extend the manifest entry** adds your files, keys and install folders to the manifest's. **Override the manifest entry** replaces the manifest's entry of the same title. For a title the manifest lacks, the two behave the same. |
| Wine prefix | The Wine prefix of the game, for example a Heroic, Lutris or Bottles prefix. Leave it empty if the game does not need one. |

### Files

Each file is a box with a **Linux path** and a **Windows path**. Enter one or both:

- **Linux path only:** backed up only when the native Linux build of the game runs here.
- **Windows path only:** backed up only for the Windows build, on Windows or in a Wine or
  Proton prefix on Linux.
- **Both:** the two paths hold the same saves in the Linux and the Windows build. When both are
  path templates with a folder below the named folder, and they end the same way after their
  last folder without wildcards (both in `/*.sav`, say, or neither with a wildcard), a restore
  onto the other build puts the files at the other path without asking. Otherwise the restore
  may ask where the files go. A folder whose name has `[` or `]` counts as a wildcard here.

**Add file** adds a box and **Remove file** removes it. An empty box is left out. A path that is
not valid shows its error under the box, and **Save game** stays disabled until you fix it.

**Browse** next to a path opens a folder picker and turns the folder you pick into a path
template, for example `<xdgData>/my-game`. You can still edit the text afterwards. On Linux, the
folder for a **Windows path** must be inside a Wine prefix, for example
`.../compatdata/<id>/pfx/drive_c/users/steamuser/AppData/Roaming/My Game`, which becomes
`<winAppData>/My Game`. A folder outside a prefix shows "Pick a folder inside a Wine prefix" and
leaves the path as it was. A Windows host has no Linux folders, so it shows no **Browse** for
the Linux path.

### Path templates

A path template starts with a named folder, then the path below it. Examples:

- Linux path: `<xdgData>/my-game`, `<xdgConfig>/my-game`, `<home>/.mygame`
- Windows path: `<winDocuments>/My Game/*.sav`, `<winAppData>/My Game`, `<home>/My Game`
- Either path: `<base>/saves`, where `<base>` is the game's install folder, and `<root>/Saves`,
  where `<root>` is the Steam library holding the game. `<root>` is for Steam libraries only.

`<home>` is your home on Linux and your user folder (`C:\Users\<name>`, or
`drive_c/users/<user>` in a prefix) on Windows. Wildcards such as `*` and `?` match as in a
shell, and a folder matches everything inside it. `<game>`, `<osUserName>` and `<storeUserId>`
are allowed; `<storeUserId>` matches any value. Use `/` between folders after a named folder.

A path can also be absolute: `/opt/My Game/saves` as a Linux path, `C:\Games\My Game` or
`C:/Games/My Game` as a Windows path. Files from an absolute path restore only to that path, on
the same OS. A box with an absolute path gives no automatic mapping, so a restore of its other
path onto the other build may ask where the files go, as for a game the manifest knows only one
build of.

### Placeholders

The **?** next to **Files** lists these in the app. A path starts with one of the named folders of
its side:

| Linux path | Folder |
| --- | --- |
| `<home>` | Your home folder |
| `<xdgData>` | `$XDG_DATA_HOME`, `~/.local/share` |
| `<xdgConfig>` | `$XDG_CONFIG_HOME`, `~/.config` |
| `<base>` | The game's install folder |
| `<root>` | The Steam library holding the game |

| Windows path | Folder |
| --- | --- |
| `<home>` | The Windows user's folder (`users/<user>` in a Wine prefix) |
| `<winDocuments>` | Documents |
| `<winAppData>` | `%APPDATA%`, `AppData/Roaming` |
| `<winLocalAppData>` | `%LOCALAPPDATA%`, `AppData/Local` |
| `<winLocalAppDataLow>` | `AppData/LocalLow` |
| `<winPublic>` | `%PUBLIC%` |
| `<winProgramData>` | `%PROGRAMDATA%` |
| `<winDir>` | `%WINDIR%` |
| `<base>` | The game's install folder |
| `<root>` | The Steam library holding the game |

`<base>` and `<root>` can stand for several folders: the game can have several
[install folders](#install-folders), and there can be several Steam libraries. A backup tries
every library with every install folder and backs up the files found in any of them. A restore
writes to the first folder that exists.

Anywhere in a path, `<game>` is the game's install folder name (or its title when it has none),
`<osUserName>` is your user name (in a Wine prefix, the prefix's user, usually `steamuser`),
`<storeUserId>` matches any store user id and `<storeGameId>` is the game's Steam id. A path with
`<storeGameId>` is skipped for a game that has no Steam id. The wildcards are `*` (any run of
characters in a name), `?` (one character) and `[...]` (one character of a set).

### Registry keys

Keys under `HKEY_CURRENT_USER` where the Windows build keeps saves or settings, one per row, for
example `HKEY_CURRENT_USER/Software/My Game`. **Add registry key** and **Remove registry key**
change the list.

### Install folders

The folder names the game installs into in a game library, one per row. `<base>` in a path is
that folder: `<library>/steamapps/common/<install folder>` in a Steam library, or
`<folder>/<install folder>` in another games folder. Without an install folder,
the game's name is used. An install folder is one folder name, not a path. **Browse** for a folder
under an install folder fills in `<base>/...`.

### Saved restore mappings

When a restore onto the other build asks where a folder goes, and you confirm, the answer is
saved here as a **Linux** and a **Windows** folder. You can edit or remove a mapping. Both sides
must be path templates without wildcards. The section only shows when the game has mappings.

### Skipped folders

A skipped folder is a template that a cross-build restore leaves out, for example
`<home>/Foo`. It applies in both directions. The mapping dialog adds skipped folders here when
you tick **Skip**.

## Saving

**Save game** checks the form. It reports an error for an empty or taken name, a mapping or
skipped folder that is not a path template, and a game that changed while the form was open (use
**Reload**).

## Games from salvajocs 0.3

Version 0.4 stores a custom game's files as Linux and Windows paths. The files of a custom game
made with 0.3 are not converted: they are still backed up, for both builds, but the form does
not show them. To change them, remove the game and add it again.

## Using a Wine prefix from another launcher

salvajocs finds Steam's Proton prefixes and, on Linux, the prefixes of Heroic, Lutris and Bottles
by itself ([Found by salvajocs](settings.md#found-by-salvajocs)). For a game in another prefix, or
one matched to the wrong prefix, add a custom game with the same title as in the manifest,
integration **Extend**, and put the prefix path in **Wine prefix**. A game with its own prefix is
looked up there first.

Back to the [guide index](README.md).
