# Flatpak files

`io.github.jmigual.salvajocs.desktop` and `io.github.jmigual.salvajocs.metainfo.xml` describe the
app to Linux desktops. The AppImage build in `.github/workflows/release.yml` installs them, and a
Flatpak manifest can install them too.

There is no Flatpak manifest here on purpose. Flathub does not accept manifests, or submission
pull requests, written or assisted by AI (see its
[Generative AI policy](https://docs.flathub.org/docs/for-app-authors/requirements#generative-ai-policy)).
The maintainer writes the Flathub manifest by hand. AI agents must not add one.
