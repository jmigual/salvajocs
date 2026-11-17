# Releasing

For maintainers. Back to the [guide index](README.md).

1. Set `version` in the root `Cargo.toml`. It is shared by both crates, `salvajocs` and
   `salvajocs-core`, including the `salvajocs-core` dependency entry.
2. If `Cargo.lock` changed, regenerate `packaging/flatpak/cargo-sources.json` with the command in
   [packaging/flatpak/README.md](../../packaging/flatpak/README.md#regenerate-cargo-sourcesjson).
3. Add a `<release version="X.Y.Z" date="...">` to `packaging/flatpak/io.github.jmigual.salvajocs.metainfo.xml`.
4. Commit and push, then push the tag `vX.Y.Z`, where `X.Y.Z` is the `version`.

The release workflow (`.github/workflows/release.yml`) then:

- fails unless the tag matches the version;
- publishes `salvajocs-core` and `salvajocs` to crates.io, skipping a crate that is already
  published, so you can publish by hand first;
- builds the Linux `.tgz` and the Windows `.zip` (the names `cargo binstall` expects) and the
  AppImage with its `.zsync` file;
- creates the GitHub Release with those files.
