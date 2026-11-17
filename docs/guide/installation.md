# Installation

salvajocs has no releases, installers or packages yet. You build it from source with Rust. This
page lists what you need and the two ways to build. Back to the [guide index](README.md).

## What you need

- Rust 1.95 or newer, from [rustup](https://rustup.rs).
- A C compiler. Two dependencies build C code: `zstd-sys` and, on Linux, a vendored copy of
  libdbus used for the keyring. On Windows the default MSVC toolchain is enough.
- On Linux, OpenGL at run time. The window is drawn with the `glow` (OpenGL) renderer of egui.
  X11 and Wayland are both enabled.

The project's CI builds and tests on a stock `ubuntu-latest` runner and on `windows-latest`
without installing any extra package. Supported systems are Linux and Windows. macOS is not
supported.

## Install with cargo

```sh
cargo install --locked --git https://github.com/jmigual/salvajocs salvajocs
```

The command installs a program called `salvajocs`. `--locked` makes cargo use the dependency
versions the project tested with.

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
