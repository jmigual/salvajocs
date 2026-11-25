# Changelog

All notable changes to salvajocs. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- A game that is behind, changed on both PCs or of unknown base has a **Back up anyway** button
  on the Backup tab. It backs up only that game, now, so this PC's saves become the newest.
- The Restore plan has a **Manifest paths** button that lists the game's save path templates and
  registry keys from the manifest or your custom game, with where each applies; right-click one to
  copy it.
- The Backup tab can sort its games by **Changes**: the number of files the next backup would
  add, change or remove, most first by default, so the games with changes are on top or at the
  bottom.
- The Restore plan overrides a whole folder: a folder's **Override** sends every file below it to
  a new absolute path, keeping their places below the folder, and **Clear** puts them back.
  Before, only single files could be overridden.
- Right-click a value in the file Details dialog (a path, a hash, a size) and choose **Copy** to
  copy all of it.
- A restore remembers which backup this PC last synced a game with, and this PC's saves after it,
  so after an undo or a partial restore the game reads behind instead of changed on both. The
  Restore tab marks a game whose newest backup is a different one, with its PC and date.
- **Delete files the backup does not have** in the Restore plan and the batch confirm (where
  **Select all** turns it on for every game) deletes this PC's files in the backup's folders that
  the backup lacks, each with its own tick; the box is remembered per game, files unticked in what
  to back up are kept, and the safety backup keeps every deleted file.

### Changed

- The file Details dialog sets This PC and the backup side by side, with the SHA-256 as a row, a
  Same or Differs mark after each label and a line on top saying how the content compares. When no
  diff is shown it says "Diff not shown:" and names the copy that is over the limit, not text or
  unreadable.
- The Backup tab compares a game with the backup this PC last synced with, not with this PC's
  last backup. A game whose newest backup comes from another PC and that this PC has not changed
  since it last synced shows a down arrow, "has newer saves", instead of a check; a game changed on
  both PCs, or that this PC has no record of syncing, has its own mark and says Back up asks.
  Back up now asks first, in the same dialog as the custom games conflicts, for each such game,
  with "Don't back up" preselected; "Back up this PC's" makes this PC's saves the newest backup.
- The Old backups rules no longer keep the newest backup of each computer. They keep the last N
  backups of a game, or those newer than the span, whichever computer made them, plus the newest
  backup of the game; the safety backups (`pre-restore`) follow the same rules on their own, with
  their newest kept. The first cleanup after the upgrade may delete each other computer's
  newest backup of a game when the rules no longer keep it.
- The custom games list says "extends the manifest entry" and "overrides the manifest entry"
  instead of "extends the manifest game" and "overrides the manifest game".
- Typing a password in Settings for a repository that was set up without one now adds that
  salvajocs cannot add one to "Wrong encryption password."; the guide shows how to add one with
  restic.
- Back up decides each game by this PC's saves against the game's newest backup, from any PC, and
  the backup this PC last synced with, kept in `sync-base.json` in the config folder. A game this
  PC has not changed while another PC made a newer backup is no longer backed up, so older saves
  never become the newest; nor is one changed both here and on another PC, or one this PC has no
  record of syncing that differs from the newest. A game this PC changed is backed up even when
  its files equal this PC's own last backup.
- The Backup results say why a game was not backed up and what to do, with their own marks
  instead of a green check or "failed": an arrow when another PC has newer saves (restore them),
  a warning when both PCs changed the saves or salvajocs does not know which backup this PC is in
  sync with.

## [0.10.0] - 2026-11-22

### Added

- Only one salvajocs runs at a time per cache folder. A second one shows "Salvajocs is already
  running" with a **Close** button, instead of sharing the backup staging files with the first.

### Changed

- A cross-build restore through a saved mapping whose target is a whole folder, such as
  `<home>` in `<home> / <winDocuments>/x`, now says so in the mapping dialog and the plan: "The
  saved mapping … writes into the whole … folder". The mapping still applies.
- After a delete or a restore, the Restore tab keeps the chosen game chosen when it still has
  backups, instead of asking you to pick it again.
- **Remove** on a custom game asks you to confirm first.
- Escape or a click outside a dialog cancels it for the delete, close-during-a-job,
  changed-on-another-PC and file details dialogs. The dialogs that must be answered and the folder
  mapping dialogs stay open.
- A new Advanced option ticks **secret** by itself when its key names a secret of a service or has
  `secret`, `password`, `token` or `key` in it, unless the key is one rustic or a non-secret
  field of a service uses, such as `use-password` or `access_key_id`. You can untick it.
- The REST and rclone keys `cacert` and `tls-client-cert` are now `http-cacert` and
  `http-tls-client-cert`, as in `rustic_backend` 0.8.0. A saved configuration with the old names
  is renamed when it is read, and the new names are written at the next save. The keys
  `timeout`, `http-cacert`, `http-tls-client-cert`, `http-insecure-tls`, `http-referer`,
  `http-read-timeout` and `http-connect-timeout` now also apply to the storage services, which
  now time out a request after 10 minutes by default; `timeout` raises the limit.
- **Restore files**, **Restore N games**, **Load backups** and every **Delete** button of the
  Restore tab are disabled, with a line saying why, while a Back up runs. Before, they waited on
  the repository without a sign. The list of games stays usable.
- A downloaded update, **Update now** or **Restart now**, waits while a custom game's form is
  open or the Settings repository form has edits you have not applied, and the bar says to save
  or cancel the form. Before, it closed the app over them.

### Fixed

- The Restore tab reads its list again after a Back up that saved backups, keeping the chosen
  game, snapshot and plan. Before, new backups and games backed up for the first time were
  missing until **Load backups**. A cleanup that removes old backups now also drops the plan and
  the batch confirm of the dropped list, which could name a removed backup.
- **Retry** and **Dismiss** no longer end up off-screen beside a long error message.
- A single restore removes the work folders that earlier sessions left in the cache, as a batch
  restore already did.
- A file whose name is not valid UTF-8 is reported as a scan problem instead of being copied
  under a changed name; the game keeps all its old backed-up files until it is renamed.
- A Windows save folder the manifest spells `<home>/Appdata/LocalLow/...` (or another case of
  `AppData`) is now stored under the same folder name as `AppData/LocalLow` and mapped to Linux
  by the Unity rule. The next **Back up** of each affected game makes one extra snapshot.
- A file or folder you unticked before that change, by a folder above `AppData` such as the home
  folder, is unticked again. Before, a saved choice on such a folder stopped applying to the
  `AppData/LocalLow`, `Local` and `Roaming` files and they were backed up.
- Two PCs saving the same game's selection, custom game or Old backups rules at the same time
  could lose one PC's change. A save that finds the other's snapshot written meanwhile is not
  counted as saved, so the change stays on this PC and is tried again at the next Back up.
- On Windows, a restore of a read-only file that failed halfway left a read-only `.salvajocs-tmp`
  file beside it that made every later restore of that file fail. It is now removed, and one an
  earlier version left is replaced.

### Security

- A custom game another PC saved to the repository, new here or changed, is no longer used on
  this PC until you accept it. **Read repository** and opening the repository list it under
  **From other PCs, not used here** in the Custom games tab, with its files, registry keys,
  old-style files and Steam id, a notice names it, and the tab label counts it, as in
  **Custom games (1)**. **Use on this PC** takes it. **Not now** moves it to a collapsed **Set
  aside** list without a notice, where **Use on this PC** stays, until another PC changes it
  again; nothing is written to the repository.
  Before, anyone who could write to the repository could make every PC's restores write any
  file, such as an autostart entry. Custom games this PC already had stay in use. A removal made
  on another PC still applies without asking. A restore no longer trusts a custom game's file
  template that is a whole folder such as `<home>`, `<home>/*` or `C:/`, nor a registry key
  outside `HKEY_CURRENT_USER` or under a system key such as `Software/Microsoft/Windows`; the
  form refuses such a template.
- **Update now** installs a release only when its `SHA256SUMS` carries a minisign signature made
  with the release key built into the app; before, the hashes came from the same release as the
  installer and proved no more than a complete download. Releases are signed in the release
  workflow, and a release without `SHA256SUMS.minisig` is refused.
- The password of a REST server is its own secret field, **REST password**, kept in the keyring or
  asked each session. It is no longer in `config.yaml`, in the keyring entry's name or in the
  repository string. A password in the URL of an existing config moves there when the app starts.
  If the keyring refuses it, it stays in the config file's URL until it can be stored. Going back
  to 0.9.1 after the move needs the URL entered again with its password.
- Settings warns when an rclone remote has `use-password` set to `false`: its local server then
  has no login, so any program on the PC can read and delete the repository.
- Updated `opendal` to 0.59.5 and `rustic_backend` to 0.8.0, which fixes the FTP command
  injection through CRLF in `suppaftp` (RUSTSEC-2026-0271), and `url` to 2.5.9
  (RUSTSEC-2026-0592).
- The Custom games form refuses a registry key a restore never writes: one outside
  `HKEY_CURRENT_USER`, `Software` itself, or a system key such as `Software/Microsoft/Windows`,
  `Software/Classes` or `Environment`. The row shows why, and **Save game** stays disabled.
- A REST password still in the repository URL, because the keyring refused it, is now hidden in
  error messages too.
- The manifest download accepts HTTPS only and is read up to 64 MiB.
- A restore no longer writes a registry key that a snapshot and a game spell with an empty part,
  such as `Software//Microsoft/Windows/CurrentVersion/Run`, to get past the protected system
  keys.
- The saved choices, custom games and Old backups rules of a repository, and the file compared in
  a backup's details, are read up to a size limit counted while reading. Before, a crafted
  repository could make the app read a very large file into memory.

## [0.9.1] - 2026-11-21

### Fixed

- The Restore tab's list of games scrolls when it is longer than the window.

## [0.9.0] - 2026-11-21

### Changed

- Only **Back up** writes custom games and the Old backups rules to the repository. Opening the
  app, a Settings switch and the new **Read repository** button in the Custom games tab only read
  them, and the tab no longer reads each time it is shown. **Save game**, **Remove**, a rename,
  a mapping dialog's choices and a tick of an Old backups rule change this PC only; a game not
  backed up yet is marked so, and the rules say "Saved to the repository with the next Back up.",
  with a **Discard this PC's changes** button. Custom games can be edited without the
  repository.
- Back up first checks the repository: a custom game or the rules changed here and, since then,
  on another PC are asked about in a "Changed on another PC" dialog that shows both versions;
  **Cancel** writes nothing. A failed save of a custom game or the rules keeps it on this PC for
  the next Back up, and a failed save of the rules removes no old backup. With no game ticked,
  Back up saves only these changes.
- The first open of a repository keeps every custom game of this PC, and asks at Back up about a
  game the repository holds otherwise or removed, instead of using the repository's version.
  `config-before-custom-games.yaml` is no longer written.
- The config writes the custom games marker as `custom_games_read_from` and still reads
  `custom_games_repository`, so salvajocs 0.8.0 never mirrors over games not backed up yet. It
  gains `custom_games_unsent` and `retention_unsent`, the changes waiting for a Back up.
- `salvajocs-core`: `custom_games::sync`, `custom_games::Synced`, `custom_games::save_choices`,
  `config::BEFORE_CUSTOM_GAMES_FILE` and `AppConfig::keep_before_custom_games` are removed;
  `custom_games::merge` takes the unsent changes, and `custom_games::Merged` has `new_unsent`
  and `complete` instead of `upload`, `replaced` and `removed` (a breaking change). The new
  `custom_games::Unsent`, `Conflict`, `conflicts`, `Sent`, `send` and `after_send` find the
  conflicts and save the unsent games, and `retention::Unsent` holds unsent rules. `AppConfig`
  has `custom_games_unsent` and `retention_unsent`. `custom_games::save` no longer returns
  `Changed` when the repository already holds the new copy.

## [0.8.0] - 2026-11-20

### Added

- On the Backup tab and in the plan of the Restore tab, a file with the changed icon has a
  **Details** button. It shows the size, modification time and SHA-256 of this PC's copy and of
  the backup's, how many lines of each end in CRLF, in LF and in a bare CR, whether it starts with
  a byte order mark (BOM), and for a text file the lines that differ (a last line without a line
  ending is marked), or that only the line endings, the BOM or both differ. A file over 10 MiB
  shows no lines until you click **Show anyway**.
- In the restore plan, of one game or of several, a checkbox on each file and folder, and on each
  registry key and subkey, chooses what is restored. Registry keys show as a tree of folders,
  whose **(values)** row ticks a key without its subkeys. Everything starts ticked. What you
  untick is not written; an unticked registry key keeps its values and subkeys, also the ones
  only on this PC. An unticked file, or registry key with no ticked key above it, is not in the
  safety backup; an unticked registry key under a ticked one, or above a key the restore
  creates, is, so an undo puts it back as it was before the restore. Unticking a file the
  safety backup cannot read lets the rest of the game restore, and unticking every registry key
  lets the files restore while Wine runs. A new plan ticks everything again.
- An **About** section at the end of the Settings tab shows the version, the commit, the Rust
  compiler, the target, the build profile and the storage library version, with a link to the
  release notes and a **Copy** button for bug reports.
- The Backup tab's games can be sorted by name, last backup, last backup size or the space all
  of a game's backups use, with the **Sort by** row above the list and an arrow that reverses
  the order. Each game shows its last backup, that backup's size and the space its backups use.
  The choice is saved in the config.
- The Restore tab's list of games has its own **Sort by** row, and a short line under each title
  with the same figures, the sorted one first.

### Changed

- Custom games are stored in the repository, so every PC that uses it has the same ones; a
  change made on one PC shows on another when it opens the repository or the Custom games tab.
  The Wine prefix of a custom game stays on each PC. The first time a PC opens a repository with
  this version, its custom games are saved to it; when the repository already has a game of the
  same name with other contents, the repository's version is used, and a game the repository
  marks as removed is removed here; a message names each, and the config from before the first
  such change is kept as `config-before-custom-games.yaml`. Connecting another repository in
  Settings takes this PC's custom games along. Without the repository, the Custom games tab
  lists them read-only. salvajocs 0.7 and older keep using their own list.
- `salvajocs-core`: the new `custom_games` module stores custom games in the repository.
  `AppConfig` gains `custom_games_repository`, and `config::BEFORE_CUSTOM_GAMES_FILE` with
  `AppConfig::keep_before_custom_games` keep the config from before. `crossbuild::apply_choices`
  takes the custom game list (`&mut Vec<CustomGame>`) instead of the whole config (a breaking
  change).
- `salvajocs-core`: `changes::Mapped::new` takes the new `sources` field, the snapshot's own path
  of each mapped file, and `changes::Baseline::backed_up` finds a file of the backup. The new
  `changes::details` module reads and compares one file; it uses the `similar` crate.
  `restore::Listed::backed_up` finds the snapshot's copy of a planned write.
- `salvajocs-core`: `registry::RegistryExport::tree` lists the keys of an export as
  `registry::KeyNode`s, with the keys a Wine export leaves out between them, and `select` gives
  what a restore of the ticked ones writes and keeps. `registry::wine::RegFile::replace_subtrees`
  and `registry::windows::replace_subtrees` take the kept keys, whose keys they leave as they
  are (a breaking change), and refuse a key both written and kept with the new
  `RegistryError::KeptAndWritten`. `restore::RestorePlan::registry_kept` carries the kept keys
  to `restore::run`.
- `salvajocs-core`: `config::AppConfig` has `backup_sort` and `restore_sort`, a
  `config::ListSort` each.
- `salvajocs-core`: `storage::SnapshotInfo` has a `size` field, the size of the snapshot's
  files, set with `with_size`. `Storage::space_used` sums the packed blobs a set of snapshots
  uses. The new `stats` module holds each game's last backup, its size and its space used, and
  orders a game list by them.
- The docs say how to reach an SFTP server from Windows: use an rclone remote of type `sftp`.
- The restic guide lists `hashes.json`, the settings snapshots and how salvajocs removes old
  backups.

### Fixed

- A tab keeps its scroll position when you show another tab and come back: the Backup tab's list
  of games no longer jumps to the top.
- After a restore or a delete in the Restore tab, the Backup tab compares the games with their
  backups again, instead of showing what differed before.

## [0.7.0] - 2026-11-20

### Added

- The AppImage checks for updates at start and updates itself: **Update now** replaces the
  AppImage with the new release, checked against `SHA256SUMS`, and **Restart now** starts it.
- On the Backup tab, a game whose newest backup is from the other build and has folders without
  a known place here shows **Choose folders…**. It opens the restore's mapping dialog; the
  choices are saved with the game as after a restore, and the game is compared again.
- The list of games on the Restore tab can be widened by dragging its right edge.
- A backup of a game found in a Wine prefix records which prefix it was, by launcher and name
  (Proton app id, Heroic app, Lutris game, Bottles bottle, or a folder in your home), without
  paths of the machine. The first backup after updating makes one new snapshot of each such
  game.
- The Restore tab says why it filled in a prefix: from the backup, found the saves there, the
  custom game's prefix, or the game's own prefix. It says when this machine does not have or
  does not use the backup's prefix, and names the prefixes when the saves are in several, or
  when the backup's prefix name fits several and none holds the saves. The "Into …" and
  "Registry" lines name a prefix by its launcher, such as "the Steam Proton prefix of app
  1868140", with its path on hover.

### Changed

- The plan of a single game on the Restore tab is a file tree with closed folders, as in the
  plan of several games, so a game with thousands of files stays responsive. A file's path shows
  on hover. Each file has an **Override** button that opens its path field, and an override can
  be cleared.
- On the Backup tab, a file shown as changed by modified time only, because its backup has no
  list of hashes (made by 0.5.0 or older), says so in its tooltip.
- The manifest status and the update notice moved from the top bar to a thinner bar along the
  bottom of the window. On the Backup tab this bar also holds the repository and **Back up N
  games**, in place of the tab's own bar.
- When a game's newest backup is from the other build (native Linux or Windows), the Backup tab
  matches its folders to this machine's as a restore would and compares the files in their
  matched places, instead of showing every file as removed and new. Folders with no known place
  here are named in the tooltips. The backup is skipped when the files are the same once matched
  and the registry keys are too.
- On Linux, a restore of a game with no custom prefix and no prefix of its own (Proton, or a
  Heroic or Lutris prefix of the game) fills in the Wine prefix the backup came from when this
  machine has it; when that names several prefixes here, such as a bottle of both the native and
  the Flatpak Bottles, it takes the one holding the saves. Otherwise, for a backup of the Windows
  build, it fills in the prefix that already holds the folders of the backup's files in the
  Windows folders (a backup of registry keys only is not looked for), and none when several do.
  Otherwise it restores into the native build as before. A restore of several games does the
  same, and choosing another snapshot works the default out again unless you changed it.
- `salvajocs-core`: `restore::default_targets` takes the snapshot read by `restore::read`
  (`Option<&StagedGame>`) and returns a `restore::DefaultTargets`, which says why it chose its
  prefix.
- `salvajocs-core`: `backup::backup_games` takes an `Option<&changes::Mapper>` instead of
  `same_as_newest: bool`; `None` turns the skip against the newest backup off, as `false` did.
  `changes::baseline` is replaced by `changes::Mapper::baseline`, which returns a
  `changes::Baseline`.

### Fixed

- Scrolling the Backup tab no longer lags or spikes the CPU: each game's comparison icons are
  worked out once, not on every frame.
- In the list of games on the Restore tab, game names are left-aligned next to their checkbox
  and cut with "…" when they do not fit, instead of centered and running past the edge, and
  the checkboxes show their outline.
- The confirm of a restore of several games named native Linux as a target of a game whose files
  and registry all go into a Wine prefix. It now names only the targets the restore writes into,
  and the plan of a single game shows the same "Into …" line.

## [0.6.0] - 2026-11-19

### Added

- A **Theme** switch in Settings, under **Appearance**: **System** (the default), **Light** or
  **Dark**. It applies at once and is kept in `config.yaml`.
- On the Restore tab, tick several games and restore them in one go with **Restore N games**.
  Each game uses its default snapshot and targets and gets its own safety backup. Games whose
  folders need mapping get the mapping dialog one after another. The plan does not download the
  backups; the files are downloaded when you click **Restore N games**.
- Before restoring several games, the plan shows each game's files as a tree. Each file that is
  new, changed (will be overwritten), could not be compared or will fail has an icon with a
  tooltip. Each folder counts its files' icons, and each game shows icons counting its new,
  changed, unchanged and not compared files. The icons come from the backup's hash list, or for
  backups made before 0.6.0 from size and modification time.
- `salvajocs-core`: `restore::read` plans a restore from a snapshot's file list without
  downloading it, `restore::download` downloads the planned game, `restore::compare_writes`
  compares a plan's targets with that file list (`WriteState::Unknown` for a target it cannot
  read), and `Storage::snapshot_entries` lists a snapshot's entries with their kinds.
- After a scan, the Backup tab compares each game with its newest backup from any machine, by
  content. Each game shows icons counting the files that are new, changed or removed since that
  backup, or a check when no backup will be made; their tooltip says what differs or why no
  backup will be made. In the file tree, files carry the same icons with a tooltip, and files
  only in the backup are greyed rows.

### Changed

- Choosing a game or snapshot on the Restore tab shows the plan at once, read from the backup's
  file list; the Preview button is gone. Files are downloaded only when you click **Restore
  files**. The tab says why **Restore files** is disabled, and a backup with folders of the other
  build waits behind **Choose folders…** instead of opening the mapping dialog by itself. Each
  file of the plan has an icon saying whether it is new, changed, could not be compared or will
  fail, as in the plan of several games.
- `salvajocs-core`: public enums and result structs are `#[non_exhaustive]`, so later releases
  can add error kinds, variants and fields without a breaking version. Build such structs with
  their new `new` constructors or `Default`, and add a `_` arm when matching such enums.
- rclone no longer asks for the password of an encrypted rclone config. Set
  `RCLONE_CONFIG_PASS` or add `--password-command` to the `rclone-command` option
  ([advanced options](docs/guide/advanced-options.md#rclone-remote)).
- Closing the window while a backup, restore, clean-up or other save is running asks first: wait
  for it or close anyway.
- Each game backup also holds `hashes.json`, the SHA-256 and size of every saved file. The first
  backup of each game after updating therefore makes a new snapshot, even when nothing changed.
- A backup skips a game whose saves are the same as its newest backup from any machine, such as
  saves a launcher's cloud sync copied, instead of storing them again. The backup results say
  `same as the newest backup, from <host>`.

### Fixed

- The Restore tab reads a backup another machine made after salvajocs opened the repository,
  instead of failing with "Tree ID … not found in index".
- On Windows, the app no longer opens a terminal window.
- On Windows, the app is named "Salvajocs" in Task Manager, the Start menu and Apps & Features.
- Dialogs wider than the window shrink to fit it, and scroll when their content is still wider,
  instead of being cut off on the left.
- The window opens centered at 1100 by 640 points, not at the small default size, and cannot be
  made narrower than 840 points.
- In a narrow window, the update notice and manifest status in the top bar are shortened with
  "…" instead of overlapping the window buttons.
- On Windows, an rclone repository no longer opens a terminal window.
- When the storage server refuses the login, the app says "The server refused the login. Check
  the user name and password." instead of "Path `config` does not exist".
- Without an encryption password, Settings no longer deletes a remembered server password from
  the keyring on **Apply**: the box, now **Remember passwords in the system keyring**, starts
  ticked when a secret field is in the keyring.

## [0.5.0] - 2026-11-19

### Changed

- When the configured repository no longer exists, the window says so and offers **Open Settings**
  to create a new one or choose another location.
- When there is no repository at the chosen location, the form is replaced by the question
  "Do you want to create one?". The compression level is asked there, not in the form.
- The AppImage's process is named `salvajocs`, not `AppRun`.
- On the Backup tab, folders that hold only one folder are shown on one line, such as
  `Home/.var/app`, instead of one line per level.

## [0.4.0] - 2026-11-18

### Added

- An [advanced storage options reference](docs/guide/advanced-options.md): how the key and value
  pairs are read, which values are accepted, and the keys of every backend.
- A warning under an Advanced option key that the chosen backend does not read.
- On the Backup tab, choose which files, folders and registry keys of each game are backed up in
  a tree of checkboxes. The choices are saved in the repository at the next backup, so every
  machine that uses it shares them.
- **Browse** next to each path of a custom game file turns a picked folder into a path template;
  on Linux, a Windows path is picked inside a Wine prefix.
- Install folders of a custom game can be edited, for `<base>` paths.
- A **?** next to Files in the custom game form lists the placeholders of the Linux and Windows
  paths and links to the guide.
- In Settings, choose which old backups to keep: the last N of each game, those newer than a
  span, or both. The rules are stored in the repository, and old backups are removed after each
  backup.
- On the Restore tab, delete one backup of a game, or all of them, from the repository.
- On the Backup tab, a game found in a Wine or Proton prefix shows the prefix.
- Each folder in the Backup tab's tree has a button that opens it in the file manager.
- On Linux, the scan finds the Wine prefixes of Heroic, Lutris and Bottles. A Heroic or Lutris
  prefix is matched to its game by id or title and checked for that game only; the others are
  checked for every game. A restore of such a game fills in its prefix.
- Settings → Roots lists these prefixes under **Found by salvajocs**; untick one to leave it out
  of scans and restores.
- While a scan runs, also the one before a backup, the Backup tab shows how many games of the
  manifest and custom games it has checked.

### Changed

- A key typed in Advanced options twice, or as a field and an option, is refused whatever its
  letter case, and for the Azure Blob, Google Cloud Storage and S3 fields also under their alias
  names, as opendal reads them.
- The **Option reference** link of the repository form opens that page, in the docs of the
  running version, instead of docs.rs or the rustic config.
- The app draws its own title bar, with the tabs and the window buttons in one bar.
- Custom game files are boxes with a Linux and a Windows path; only one is required. A box with
  one path is backed up only for that build; with two path templates, a restore onto the other
  build uses the other path without asking. Breaking: the files of custom games from 0.3.x are
  not converted and the form does not show them; remove such a game and add it again to edit
  them.
- A custom game's registry keys are a list of rows, and its Linux and Windows pairs show as
  "Saved restore mappings" only when a restore saved some.
- The scan skips a path whose first folder is not on disk without searching it, so each Wine
  prefix checked for every game adds about a tenth of the time it did.
- The manifest cache keeps the GOG, Steam and Lutris ids of each game and is now
  `manifest-v2.json`, so the first start of this version downloads the full manifest once.

### Removed

- `salvajocs_core::config::opendal_docs_url`, which built the docs.rs link.

### Fixed

- A dialog taller than the window, such as the one that creates a repository, scrolls.
- The "Update available" notice shows in the first-run and repository secrets dialogs, which
  cover the top bar.

## [0.3.1] - 2026-11-17

### Changed

- The app is called "Salvajocs" in the app menu, the window title and the AppStream metadata.

### Removed

- Support for running inside a Flatpak sandbox: game folders from the host's `HOST_XDG_*`
  variables, and the "close Wine and confirm" step before a registry restore. salvajocs is not
  published as a Flatpak.

### Fixed

- On Wayland the task bar and title bar show the app icon when an AppImage integrator (Gear
  Lever, AppImageLauncher, appimaged) installed the desktop file under another name.

## [0.3.0] - 2026-11-17

### Changed

- Each storage backend has its own named fields, a one-line hint and a link to its docs page. The
  backend list uses friendly names, such as "WebDAV" and "Amazon S3 (or compatible)".
- Options without a field of their own are under **Advanced options**.
- The repository password is now called the **encryption password**, so it is not confused with
  the login of the storage server.
- Compression is a dropdown: Off, Fast, Default, Smaller, Smallest, or a custom level.

### Added

- The rclone backend lists the remotes that `rclone listremotes` reports.

### Fixed

- A backend secret stays with its service when you switch to another backend and back.
- A failed rclone command without error output reports its exit status.

## [0.2.0] - 2026-11-17

### Changed

- A new look: green light and dark themes from the app icon, the Atkinson Hyperlegible Next font,
  icons, and a save bar on the Backup tab.
- Restore lists the games in a sidebar, and each plan row is boxed.
- Labels, counts and messages are plainer and consistent.
- Keyboard and screen reader order follows the order on screen.
- The license is `MIT AND OFL-1.1`. The release archives, the AppImage and the Windows installer
  include the license texts of the embedded fonts and icons.

### Fixed

- Primary buttons show a visible focus outline in the light theme.

## [0.1.0] - 2026-11-17

First release.

- Back up and restore game saves in a restic-format repository, using the ludusavi game manifest.
- Saves from Wine and Proton prefixes on Linux restore on Windows, and the other way round.
- Registry keys: the live registry on Windows, `user.reg` inside a Wine or Proton prefix.
- Storage in a local folder, a REST server, an rclone remote or an OpenDAL service.
- Restore preview, per-file target override, a safety backup and undo.
- Custom games, Wine prefixes and cross-OS game pairs.
- Windows installer with in-app updates, Windows zip, Linux AppImage and tarball, and crates.io.

[Unreleased]: https://github.com/jmigual/salvajocs/compare/v0.10.0...HEAD
[0.10.0]: https://github.com/jmigual/salvajocs/compare/v0.9.1...v0.10.0
[0.9.1]: https://github.com/jmigual/salvajocs/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/jmigual/salvajocs/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/jmigual/salvajocs/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/jmigual/salvajocs/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/jmigual/salvajocs/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/jmigual/salvajocs/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/jmigual/salvajocs/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/jmigual/salvajocs/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/jmigual/salvajocs/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/jmigual/salvajocs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jmigual/salvajocs/releases/tag/v0.1.0
