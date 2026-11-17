# Installation

Pick one of the options below, in order of effort. Back to the [guide index](README.md).

## AppImage (Linux)

Download `salvajocs-<version>-x86_64.AppImage` from the
[latest release](https://github.com/jmigual/salvajocs/releases/latest), then:

```sh
chmod +x salvajocs-*-x86_64.AppImage
./salvajocs-*-x86_64.AppImage
```

It needs glibc 2.35 or newer (Ubuntu 22.04 or later), OpenGL and FUSE 2. Without FUSE 2, run it
with `--appimage-extract-and-run`.

The file embeds update information that points at the latest GitHub release. Open it in
[Gear Lever](https://github.com/mijorus/gearlever) or run
[AppImageUpdate](https://github.com/AppImageCommunity/AppImageUpdate) on it. Both download only the
blocks that changed, using the `.zsync` file next to each release.

## Windows

Download `salvajocs-x86_64-pc-windows-msvc-v<version>.zip` from the
[latest release](https://github.com/jmigual/salvajocs/releases/latest), extract it and run
`salvajocs.exe`. There is no installer and no automatic update.

## cargo-binstall

```sh
cargo binstall salvajocs
```

This downloads the Linux (`x86_64-unknown-linux-gnu`) or Windows (`x86_64-pc-windows-msvc`)
archive of the latest release and installs `salvajocs` in cargo's bin folder. On any other target
it falls back to building from source.

## Install with cargo

Needs Rust 1.95 or newer, from [rustup](https://rustup.rs), and the build requirements below.

```sh
cargo install --locked salvajocs
```

`--locked` makes cargo use the dependency versions the project tested with. For the latest
unreleased code:

```sh
cargo install --locked --git https://github.com/jmigual/salvajocs salvajocs
```

## Flatpak

Not on Flathub yet; a submission is planned. You can build and install it locally. See
[packaging/flatpak/README.md](../../packaging/flatpak/README.md).

## Build requirements

These apply to `cargo install` and to building from a checkout. The release downloads need none of
them.

- A C compiler. Two dependencies build C code: `zstd-sys` and, on Linux, a vendored copy of
  libdbus used for the keyring. On Windows the default MSVC toolchain is enough.
- On Linux, OpenGL at run time. The window is drawn with the `glow` (OpenGL) renderer of egui.
  X11 and Wayland are both enabled.

The project's CI builds and tests on a stock `ubuntu-latest` runner and on `windows-latest`
without installing any extra package. Supported systems are Linux and Windows. macOS is not
supported.

## Build from a checkout

```sh
git clone https://github.com/jmigual/salvajocs
cd salvajocs
cargo run --release -p salvajocs
```

`cargo build --release -p salvajocs` builds `target/release/salvajocs` without starting it.

## Where salvajocs keeps its files

| What | Linux | Linux (Flatpak) | Windows |
| --- | --- | --- | --- |
| Settings (`config.yaml`) | `~/.config/salvajocs/` | `~/.var/app/io.github.jmigual.salvajocs/config/salvajocs/` | `%APPDATA%\salvajocs\` |
| Game list cache and temporary work folders | `~/.cache/salvajocs/` | `~/.var/app/io.github.jmigual.salvajocs/cache/salvajocs/` | `%LOCALAPPDATA%\salvajocs\` |

The backups themselves live in the repository you choose on first run.

## Next

Start the program and follow [First run](first-run.md).
