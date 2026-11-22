# Releasing

For maintainers. Back to the [guide index](README.md).

1. Set `version` in the root `Cargo.toml`. It is shared by both crates, `salvajocs` and
   `salvajocs-core`, including the `salvajocs-core` dependency entry.
2. Move the `[Unreleased]` entries in `CHANGELOG.md` under a new `## [X.Y.Z] - YYYY-MM-DD`
   heading, and update the compare links at the bottom.
3. Add a `<release version="X.Y.Z" date="...">` to `packaging/flatpak/io.github.jmigual.salvajocs.metainfo.xml`.
4. Commit and push, then push the tag `vX.Y.Z`, where `X.Y.Z` is the `version`.

The release workflow (`.github/workflows/release.yml`) then:

- fails unless the tag matches the version and `CHANGELOG.md` has a section for it;
- builds the Linux `.tgz` and the Windows `.zip` (the names `cargo binstall` expects), the
  AppImage with its `.zsync` file and the Windows installer
  (`salvajocs-<version>-x86_64-setup.exe`, from `packaging/windows/salvajocs.iss`);
- writes a `SHA256SUMS` file covering every one of those files, and signs it with the release
  key into `SHA256SUMS.minisig` (the release fails when the `MINISIGN_SECRET_KEY` secret is
  missing or is not the key matching the public key in the repository);
- creates the GitHub Release with those files, `SHA256SUMS` and `SHA256SUMS.minisig`, and uses
  the version's `CHANGELOG.md` section as the release notes;
- then, only once the GitHub Release exists, publishes `salvajocs-core` and `salvajocs` to
  crates.io, skipping a crate that is already published, so you can publish by hand first.

If the crates.io job fails after the release job succeeded, the GitHub Release already exists:
re-run only the failed job, as the publish step skips crates that are already published.

## Signing key

The in-app update (ADR-0013) installs only a release whose `SHA256SUMS` is signed with a minisign
key; the app embeds the public key from `crates/salvajocs-core/src/update/salvajocs.pub`.

One-time setup:

1. Make an unencrypted key pair, as CI cannot type a password:
   `rsign generate -W -p salvajocs.pub -s salvajocs.key` (rsign2, or `minisign -G` with an empty
   password).
2. Commit the public key as `crates/salvajocs-core/src/update/salvajocs.pub`.
3. In the repository's Settings, Environments, create the environment `release` with a required
   reviewer. If it restricts deployment branches or tags, allow `v*` tags.
4. In that environment, add the secret `MINISIGN_SECRET_KEY` with the content of `salvajocs.key`.
   Make it an environment secret, not a repository secret: a tagged commit whose `release.yml`
   drops `environment:` can read a repository secret, and the reviewer gate (ADR-0013) would not
   hold.
5. Keep an offline backup of `salvajocs.key` (a password manager), then delete the local file.

A release signed with a key other than the embedded one is refused by every installed app. If the
key is lost or replaced, make a new pair and replace the secret and `salvajocs.pub`; the apps
built with the old public key refuse the releases signed by the new one: in the installer and the
AppImage **Update now** fails with "not signed with the salvajocs release key", and its users
install the release by hand once from the release page. Other builds (zip, `cargo install`)
never update in place and keep the release page link. Apps built before the signature was added
(up to 0.9.1) keep checking only `SHA256SUMS`.

If the key leaks, replacing the secret does not protect the builds that ship the old public key:
they keep trusting it, so whoever holds the leaked key can sign assets they accept. Their users
must install a new build by hand.

## Dependency bumps and the docs

- After bumping `opendal` or `rustic_backend`, re-check [Advanced options](advanced-options.md)
  against the new versions: the keys of each service, the value rules, and the "Checked against"
  line at the top.
- The **Option reference** link in the app points at the page in the tag of the running version
  (`v<version>`), so a page the GUI links to must exist in the tagged tree.
