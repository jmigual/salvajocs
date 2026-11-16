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
snapshots, so do not remove any without a copy of the repository.

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

Each game is one snapshot, tagged `salvajocs` and `game:<slug>`. The saved folder is
`/salvajocs/<slug>`, and it holds:

```
files/<Folder>/<path>    the save files
salvajocs.json           where each folder came from
registry.json            registry keys, when the game has any
```

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

## Writing with restic

Do not add your own snapshots with restic and expect salvajocs to restore them. The Restore tab
lists snapshots that carry the `salvajocs` tag and a `game:<slug>` tag, and a restore needs the
layout above.

Back to the [guide index](README.md).
