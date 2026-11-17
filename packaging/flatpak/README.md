# Flatpak packaging

App ID `io.github.jmigual.salvajocs`, runtime `org.freedesktop.Platform` 26.08, built offline
from `cargo-sources.json`.

## Build and install locally

Run from the repository root, on a clean checkout (the `dir` source copies the whole tree).

```sh
flatpak install --user flathub org.flatpak.Builder org.freedesktop.Sdk//26.08 \
  org.freedesktop.Sdk.Extension.rust-stable//26.08
flatpak run org.flatpak.Builder --user --force-clean --install --install-deps-from=flathub \
  --state-dir=tmp/flatpak/state tmp/flatpak/build \
  packaging/flatpak/io.github.jmigual.salvajocs.yml
flatpak run io.github.jmigual.salvajocs
```

Remove it with `flatpak uninstall --user io.github.jmigual.salvajocs`.

## Regenerate `cargo-sources.json`

Run after every change to `Cargo.lock`.

```sh
git clone --depth 1 https://github.com/flatpak/flatpak-builder-tools tmp/flatpak-builder-tools
python3 -m venv tmp/fbt-venv
tmp/fbt-venv/bin/pip install aiohttp tomlkit
tmp/fbt-venv/bin/python tmp/flatpak-builder-tools/cargo/flatpak-cargo-generator.py \
  Cargo.lock -o packaging/flatpak/cargo-sources.json
```

## Lint

```sh
flatpak install --user flathub org.flatpak.Builder
flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest \
  packaging/flatpak/io.github.jmigual.salvajocs.yml
flatpak run --command=flatpak-builder-lint org.flatpak.Builder repo <repo-dir>
```

`<repo-dir>` is the directory given to `flatpak-builder --repo=<repo-dir>`. On a local build the
repo lint reports `appstream-screenshots-not-mirrored-in-ostree` and
`appstream-external-screenshot-url`; Flathub's build mirrors the screenshots, so they go away
there. Before the `v0.2.0` tag exists the screenshot URLs return 404.

## Submit to Flathub

Steps from <https://docs.flathub.org/docs/for-app-authors/submission>.

1. Use the latest release tag, for example `v0.2.0` (see
   [releasing](../../docs/guide/releasing.md)). The metainfo screenshot URLs point at that tag,
   so `docs/images/*.png` must exist in it.
2. Get its commit: `git rev-parse v0.2.0^{commit}`.
3. Fork <https://github.com/flathub/flathub> with "Copy the master branch only" unticked, and
   clone your fork on the `new-pr` branch:
   `git clone --branch=new-pr git@github.com:<you>/flathub.git && cd flathub && git checkout -b salvajocs new-pr`.
4. Copy `io.github.jmigual.salvajocs.yml` and `cargo-sources.json` into the root of that branch.
5. In the copied manifest, replace the `dir` source (the whole `type: dir` block with its
   `skip` list) by:

   ```yaml
   sources:
     - type: git
       url: https://github.com/jmigual/salvajocs.git
       tag: v0.2.0
       commit: <commit of v0.2.0>
     - cargo-sources.json
   ```

   `<commit of v0.2.0>` is the 40-character hash from step 2. Flathub requires it next to the
   tag.
6. Commit, push the branch and open a pull request against the `new-pr` branch of
   `flathub/flathub`. Flathub's bot builds it and posts the result; comment `bot, build` to
   rebuild.

### Exception justification: `--filesystem=home`

`flatpak-builder-lint` accepts `--filesystem=home`, but Flathub reviewers ask for a reason in the
submission pull request. Text to paste:

> salvajocs backs up and restores game saves. Saves live in native folders (`~/.config`,
> `~/.local/share`, `~/Documents`), in Wine and Proton prefixes, and in Steam libraries that can
> be anywhere under the home folder. The set of paths is read from the ludusavi game manifest and
> from user settings, so it cannot be listed ahead of time or limited to a portal-chosen folder.
> The app must read these paths to back them up and write them to restore. `/run/media` and
> `/mnt` cover the same data and the backup repository on external drives. The Steam Flatpak data
> under `~/.var/app/com.valvesoftware.Steam` is not part of `home`, so it is listed on its own.
