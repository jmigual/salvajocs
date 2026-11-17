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

## Windows installer

Download `salvajocs-<version>-x86_64-setup.exe` from the
[latest release](https://github.com/jmigual/salvajocs/releases/latest) and run it. It installs for
your user only, in `%LOCALAPPDATA%\Programs\salvajocs`, needs no administrator rights and adds a
Start menu shortcut.

The installer is not code-signed, so Windows SmartScreen may show "Windows protected your PC".
Click **More info**, then **Run anyway**.

To uninstall, use **Settings → Apps → Installed apps**. This removes the program only; your
settings, cache and backups stay (see [Where salvajocs keeps its files](#where-salvajocs-keeps-its-files)).

The app offers updates when a newer release exists.

## Windows zip

Download `salvajocs-x86_64-pc-windows-msvc-v<version>.zip` from the same page, extract it and run
`salvajocs.exe`. This copy does not update itself.

## mise

[mise-en-place](https://mise.jdx.dev) can install salvajocs on Linux and Windows. From the GitHub
release, using the prebuilt archive for your system:

```sh
mise use -g github:jmigual/salvajocs
```

mise normally picks the archive by itself. The release also holds an AppImage and the setup
program, which mise cannot use. If it picks one of them, keep only the archives:

```sh
mise use -g 'github:jmigual/salvajocs[matching=salvajocs-x86_64-]'
```

Or build from crates.io with the [cargo backend](https://mise.jdx.dev/dev-tools/backends/cargo.html):

```sh
mise use -g cargo:salvajocs
```

With `cargo-binstall` installed (`mise use -g cargo-binstall`) it downloads the same prebuilt
archive. Without it, mise runs `cargo install` and needs Rust and the
[build requirements](#build-requirements).

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

| What | Linux | Windows |
| --- | --- | --- |
| Settings (`config.yaml`) | `~/.config/salvajocs/` | `%APPDATA%\salvajocs\` |
| Game list cache and temporary work folders | `~/.cache/salvajocs/` | `%LOCALAPPDATA%\salvajocs\` |

The backups themselves live in the repository you choose on first run.

## Next

Start the program and follow [First run](first-run.md).
