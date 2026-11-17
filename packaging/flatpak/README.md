# Flatpak files

`io.github.jmigual.salvajocs.desktop` and `io.github.jmigual.salvajocs.metainfo.xml` describe the
app to Linux desktops. The AppImage build in `.github/workflows/release.yml` installs them, and a
Flatpak manifest can install them too.

salvajocs is not published on Flathub, and there is no Flatpak manifest here on purpose. Flathub
does not accept manifests written or assisted by AI (see its
[Generative AI policy](https://docs.flathub.org/docs/for-app-authors/requirements#generative-ai-policy)).
AI agents must not add one.
