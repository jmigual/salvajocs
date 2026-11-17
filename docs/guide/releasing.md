# Releasing

For maintainers. Back to the [guide index](README.md).

1. Set `version` in the root `Cargo.toml`. It is shared by both crates, `salvajocs` and
   `salvajocs-core`, including the `salvajocs-core` dependency entry.
2. Add a `<release version="X.Y.Z" date="...">` to `packaging/flatpak/io.github.jmigual.salvajocs.metainfo.xml`.
3. Commit and push, then push the tag `vX.Y.Z`, where `X.Y.Z` is the `version`.

The release workflow (`.github/workflows/release.yml`) then:

- fails unless the tag matches the version;
- publishes `salvajocs-core` and `salvajocs` to crates.io, skipping a crate that is already
  published, so you can publish by hand first;
- builds the Linux `.tgz` and the Windows `.zip` (the names `cargo binstall` expects), the
  AppImage with its `.zsync` file and the Windows installer
  (`salvajocs-<version>-x86_64-setup.exe`, from `packaging/windows/salvajocs.iss`);
- writes a `SHA256SUMS` file covering every one of those files;
- creates the GitHub Release with those files and `SHA256SUMS`.
