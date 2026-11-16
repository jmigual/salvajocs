# ADR-0007: Prefix discovery in v1: Steam/Proton plus manually configured prefixes

**Status:** Accepted
**Date:** 2026-11-16

## Context

Windows games on Linux live in Wine prefixes created by different launchers (research.md §2, §6):

- Proton: `<steamroot>/steamapps/compatdata/<appid>/pfx`, user `steamuser`.
- Plain Wine: `~/.wine`, user = the Unix user.
- Heroic: per-game `winePrefix` in `GamesConfig/*.json`.
- Lutris: `game.prefix` in `games/*.yml`.
- Bottles: `$XDG_DATA_HOME/bottles/bottles/<name>`. Ludusavi does not support Bottles.

Ludusavi validates a prefix by `drive_c`, `drive_c/users` and one of
`system.reg`/`user.reg`/`dosdevices`. It picks the user as the single non-system user, else
`steamuser`, else reports the prefix as ambiguous.

## Decision

v1 discovers:

1. **Steam roots.** Linux: `~/.steam/steam`, `~/.local/share/Steam`, and the Flatpak path
   `~/.var/app/com.valvesoftware.Steam/.local/share/Steam` (confirmed in the flathub
   `com.valvesoftware.Steam` sources: `--persist=.` and `steam_wrapper.py` move `Steam` to
   `<state>/.local/share`; installs not yet migrated may still have `.../data/Steam`). Windows:
   `%ProgramFiles(x86)%\Steam`, else `C:\Program Files (x86)\Steam` (only partly verified: no
   Valve page states it; ludusavi `config.rs` `find_missing_roots` uses the same paths). A directory is a Steam root when it
   has `steamapps`. Extra library folders are added by the user as Steam roots.
2. **Proton prefixes** at `<steamroot>/steamapps/compatdata/<steam.id>/pfx` for manifest entries
   that have `steam.id`.
3. **Manual prefixes:**
   - A root of kind `WinePrefix` in Settings: every game is checked against it, which covers
     `~/.wine`.
   - A custom game's `wine_prefix`, which covers Heroic, Lutris and Bottles by hand.

The prefix check and user choice follow ludusavi: validate a prefix as above; the user is the
single non-system entry of `drive_c/users` (system = `Public` and `root`; a prefix run as root
has a `root` dir), else `steamuser`, else a typed `Ambiguous` error. A prefix with no user dir (never run) is an error on restore: run the game once.

Priority when several prefixes match a game: the game's own prefix (Proton appid or
`wine_prefix`) first, then `WinePrefix` roots in configured order. When several match, the first
one wins per logical path. Registry keys use the same context priority. Scan problems, such as
an `Ambiguous` prefix, are returned typed, never dropped.

## Alternatives considered

- **Auto-discover Heroic, Lutris and Bottles in v1:** three config formats to parse. Each is a
  small additive step later, and the manual `wine_prefix` covers them now.
- **Ask the user to pick the prefix for every game:** too much friction for Steam, the common case.

## Consequences

- Non-Steam shortcuts in Steam (generated appids) are not matched automatically.
- Discovery is filesystem-only, so it can be tested with tempdir fixtures.
- Default roots derive from `$HOME`. In a container with a separate home (the user's distrobox:
  `$HOME` is `~/Documents/Distrobox/dev`, Steam is at `/home/jmigual/.local/share/Steam`)
  auto-detection misses Steam. A Steam root added in Settings covers it.
