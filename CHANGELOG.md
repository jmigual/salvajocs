# Changelog

All notable changes to salvajocs. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/jmigual/salvajocs/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/jmigual/salvajocs/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/jmigual/salvajocs/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/jmigual/salvajocs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jmigual/salvajocs/releases/tag/v0.1.0
