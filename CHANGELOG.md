# Changelog

All notable changes to salvajocs. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/jmigual/salvajocs/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/jmigual/salvajocs/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/jmigual/salvajocs/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/jmigual/salvajocs/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/jmigual/salvajocs/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/jmigual/salvajocs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jmigual/salvajocs/releases/tag/v0.1.0
