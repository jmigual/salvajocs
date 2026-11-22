# Custom games

A custom game is a game you describe yourself: its save files, its registry keys and, if needed,
its Wine prefix. Use one when the manifest does not know a game, when it has a wrong path, or to
store the choices of a [cross-build restore](cross-os-restore.md). Back to the
[guide index](README.md).

![The Custom games tab](../images/custom-games.png)

The tab lists your custom games. Each line says how the game relates to the manifest: a custom
game, a game that extends the manifest entry, or one that overrides it. **Edit** and **Remove**
change the list; **Remove** asks you to confirm first, and Escape or a click outside that dialog
cancels it. **Add game** opens the form. Changes stay on this PC until the next [Back
up](backup.md#custom-games-and-rules), which saves them to the repository, so every PC that uses it
can have the same custom games. **Read repository** reads the changes other PCs saved; this PC
uses them only once you accept them ([Games from other PCs](#games-from-other-pcs)). The **Wine
prefix** stays on this PC: each PC sets its own.

## The form

| Field | What to enter |
| --- | --- |
| Name | The game's title. The name is how the game is matched with the manifest, so spell it exactly as the manifest does for an existing game. Two custom games cannot share a name. |
| Integration | **Extend the manifest entry** adds your files, keys and install folders to the manifest's. **Override the manifest entry** replaces the manifest's entry of the same title. For a title the manifest lacks, the two behave the same. |
| Wine prefix | The Wine prefix of the game, for example a Heroic, Lutris or Bottles prefix. Leave it empty if the game does not need one. It is saved on this PC only, not in the repository. |

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
change the list. A key outside `HKEY_CURRENT_USER`, `HKEY_CURRENT_USER/Software` itself, or a
system key such as `Software/Microsoft/Windows`, `Software/Classes` or `Environment` cannot be
saved: a restore never writes it.

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
you tick **Skip** on a row (labelled Skip 1, Skip 2, ...).

## Saving

**Save game** checks the form, then saves the game on this PC and closes the form. It reports an
error for an empty or taken name, a mapping or skipped folder that is not a path template, and a
game that changed while the form was open, for example by **Use on this PC** or a restore
mapping: click **Reload** to see the game as it is now, then make your change again.

A game changed here shows "Not backed up yet" until a Back up saves it to the repository. A
removed game is listed as "Removed here; other PCs keep them until the next Back up: …".
Renaming a game is saved as the new name and the removal of the old one; if the repository cannot
take the new name, the old one is not removed either, and the next Back up tries both again.
Changing only the Wine prefix leaves nothing to back up, as it is never stored.

The repository is read when this PC opens it, after you connect another repository in Settings,
and when you click **Read repository**. A read never adds or changes a game on this PC: what
other PCs added or changed waits for you below the list. It only removes a game another PC
removed, unless you changed it here and did not back up yet. Without the repository (it is
missing, or could not be opened) you can still add, edit and remove custom games; they wait for
the next Back up. The Backup tab scans them too.

A game saved by a newer salvajocs shows "Written by a newer salvajocs" and cannot be edited or
removed here. A game that cannot be read from the repository shows why. Either way this PC keeps
its own copy of it, if it has one, and backs it up as before.

## Games from other PCs

A custom game decides which files and registry keys a restore may write, so a game another PC
saved to the repository is not used here until you accept it on this PC. After a read, the
section **From other PCs, not used here** lists each game that is new here or whose repository
copy differs from this PC's, and a notice names them. After you switch to another repository in
Settings, the list waits for that repository's read. Each shows its files, registry keys,
old-style files, mappings, skipped folders and Steam id; a changed game shows this PC's version
beside the repository's, with a line only one of them has in bold.

- **Use on this PC** puts the repository's version on this PC, with this PC's Wine prefix. It
  leaves nothing to back up.
- **Keep this PC's version** marks this PC's version "Not backed up yet": the next Back up saves
  it over the other PC's.
- **Remove from the repository**, for a game new here, has the next Back up remove it from the
  repository. Every PC that already uses it removes it at its next read, without asking, and
  other PCs stop being offered it.

Without an answer the game stays listed and is not used. A restore of such a game uses only this
PC's definition, and its plan in the Restore tab says that another PC's version waits here. Use
a game only if you recognise it: anyone who can write to the repository can save one. The same choice is made in the Back up question below: **Keep the repository's**
accepts that version.

The custom games this PC already had before this version stay in use without asking.

## Changed on another PC

When a game you changed here was also changed on another PC since, Back up asks before it saves
anything: the dialog "Changed on another PC" shows **This PC**'s version and **The
repository**'s side by side, with a line that only one of them has in bold. Pick **Keep this
PC's** or **Keep the repository's** for each game, then **Back up**. Keeping the repository's
version puts it on this PC, with this PC's Wine prefix, or removes the game here when the other PC
removed it. **Cancel** stops the Back up, and nothing is written.

## Other PCs and older versions

The first time a PC opens a repository with this version, it keeps all its custom games. A game
the repository already has with the same contents is left as it is. Any other game is marked
"Not backed up yet": the next Back up adds a game the repository does not have, and asks about a
game the repository holds with other contents or says was removed on another PC. The
repository's games this PC does not have wait in [Games from other PCs](#games-from-other-pcs).
The same happens after you connect another repository in Settings: this PC's custom games, and
the changes it did not back up yet, go along to the new repository.

salvajocs 0.8.0 saved custom games to the repository at once and may have left a copy of the
config as `config-before-custom-games.yaml` in the config folder; this version no longer writes
it. A PC still on 0.8.0 keeps saving its changes at once, and the next Back up here asks about
any of those games this PC changed too. salvajocs 0.9.1 and older use the repository's games
without asking. salvajocs 0.7 and older do not read custom games from
the repository. A PC with such a version keeps using its own list, and does not see games added
on other PCs until it is updated.

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
