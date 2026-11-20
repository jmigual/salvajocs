# Using the repository with restic

A salvajocs repository is a restic repository. You can list its snapshots and copy files out of it
with the restic command line, without salvajocs. This page shows how, and lists what was tested
and what was not. Back to the [guide index](README.md).

## What was tested

Interoperability was tested with restic 0.19.1, in both directions, on a repository with an empty
password:

- restic listed the snapshots written by salvajocs's storage library (rustic_core), listed their
  files and restored them. The restored files were identical to the originals.
- restic then wrote a snapshot into the same repository. `restic check --read-data` found no
  errors, and rustic_core opened the repository again and restored that snapshot identically.

Not tested: the `rustic` command line, a repository with a password, and `restic forget` or
`restic prune` on a salvajocs repository. The Restore tab and the undo of a restore need the
snapshots, so do not remove any with restic without a copy of the repository.

## How salvajocs removes old backups

If you set the [Old backups](settings.md#old-backups) rules in Settings, salvajocs forgets the
snapshots they let go after each backup, then prunes. With the rules off it forgets nothing. Its
prune marks unused data first and deletes it at least 23 hours later. salvajocs does not take
restic's locks, and restic's locks do not stop it.

Do not run `restic prune` while salvajocs may back up into the repository. `restic prune` deletes
unused data at once, so it can remove data a salvajocs backup running at the same time is about to
use.

## Opening the repository

restic must be able to open the repository location with one of its own backends, for example a
local folder, a REST server or rclone. Reading a repository written through an OpenDAL service with
restic's own cloud backends was not tested.

```sh
restic -r /path/to/repository snapshots --tag salvajocs
```

- Use the password you set. If you chose no password, add `--insecure-no-password` to every
  command.
- restic ignores the compression level stored in the repository. It uses its own `--compression`
  option when it writes.

## What a snapshot holds

Each game is one snapshot, tagged `salvajocs` and `game:<slug>`. The safety backup a restore
makes first also has the tag `pre-restore`. The saved folder is `/salvajocs/<slug>`, and it holds:

```
files/<Folder>/<path>    the save files
salvajocs.json           where each folder came from, and the Wine prefix it was found in
registry.json            registry keys, when the game has any
hashes.json              the SHA-256 of every other file, and the size of the save files
```

salvajocs reads `hashes.json` to see which files changed since the newest backup. Backups made
before 0.6.0 do not have it, and salvajocs then compares size and modification time.

`<Folder>` is one of `WinHome`, `WinDocuments`, `WinAppData`, `WinLocalAppData`,
`WinLocalAppDataLow`, `WinPublic`, `WinProgramData`, `WinDir`, `Home`, `XdgData`, `XdgConfig`,
`GameDir`, `StoreRoot`, `Absolute` or `WinAbsolute`. They are not real paths of any machine.

To copy a game's files out, take the snapshot ID from `snapshots` (the `game:` tag names the
game):

```sh
restic -r /path/to/repository ls <snapshot-id>
restic -r /path/to/repository restore <snapshot-id> --target /tmp/out
```

`restore` gives you this staging tree, not the saves in their original places. Use salvajocs to put
them back where the game reads them. It handles the user names, the prefixes and the registry.

## Settings snapshots

The repository also holds snapshots that are not game backups. They carry the tag
`salvajocs-settings`, not `salvajocs`, so `--tag salvajocs` does not list them and the Restore tab
does not show them.

- The choices of what to back up for a game: tags `salvajocs-settings` and `settings:<slug>`,
  folder `/salvajocs-settings/<slug>`, file `settings.json`.
- The [Old backups](settings.md#old-backups) rules, shared by all games: tags
  `salvajocs-settings` and `settings:retention`, folder `/salvajocs-settings/retention`, file
  `retention.json`.

salvajocs writes a new snapshot when a backup saves your choices for a game, and when you change
the Old backups rules. It then forgets the older ones. If you remove the newest one with restic,
you lose that setting: salvajocs then backs up every file of the game, or keeps every backup. To
list them, use `--tag salvajocs-settings`.

## Writing with restic

Do not add your own snapshots with restic and expect salvajocs to restore them. The Restore tab
lists snapshots that carry the `salvajocs` tag and a `game:<slug>` tag, and a restore needs the
layout above.

Back to the [guide index](README.md).
