# ADR-0017: Launcher prefixes of Heroic, Lutris and Bottles, mapped to games

**Status:** Accepted
**Date:** 2026-11-18

## Context

ADR-0007 left Heroic, Lutris and Bottles to a hand-set `wine_prefix` or a `WinePrefix` root. The
user asked for these prefixes to be found automatically. Each launcher keeps its prefixes in its
own files:

- Heroic: `GamesConfig/<app>.json` (`winePrefix`), its store libraries for titles and, for GOG,
  the product id as the app name.
- Lutris: `games/<id>.yml` (`game.prefix`, `game.exe`, `name`, `game_slug`, `service`,
  `service_id`). The database `pga.db` also holds the game directory.
- Bottles: one directory per bottle. A bottle is not tied to one game.

A prefix used for every game costs one more glob pass of the whole manifest per scan (ADR-0008).

## Decision

- **Detection** (`launcher::detect`, called by `Environment::detect` on Linux only). It reads the
  native and Flatpak installs of each launcher:
  - Heroic: the `winePrefix` of each app not installed for Linux. An app without one uses the
    default prefix, but only when it is a Windows app. The prefix itself is used when it is
    valid, else its `pfx` (Proton).
  - Lutris: only `games/*.yml`, not `pga.db`, so there is no SQLite dependency. The prefix is
    `game.prefix`, else the prefix that encloses `game.exe`. A relative path depends on the game
    directory in `pga.db`, so it is skipped.
  - Bottles: each bottle, a bottle stored elsewhere through `placeholder.yml` or `Custom_Path`,
    and the Flatpak `/var/data` mapped to the host.
  - Only paths that pass `validate_prefix` are kept. A launcher file that cannot be read or
    parsed is a scan problem. A missing launcher is not a problem.
- **Mapping** (`launcher::GameIndex`): a Heroic or Lutris prefix maps to one manifest title. The
  rules apply in this order:
  1. The ids: `gog.id` or `gogExtra`, `steam.id` or `steamExtra`, or `id.lutris`. They must
     match exactly one game. Ids that match several games fall through to the next rule.
  2. The exact title.
  3. `normalize_title` (ported from ludusavi), when it matches exactly one game.

  A normalized title that matches several games is ambiguous, and the prefix is unmapped. A
  title-only match is used without asking. Bottles prefixes and unmapped prefixes form the
  shared set. Mapping uses manifest titles, so a custom game with a manifest title gets that
  game's prefix.
- **Scan order** (`scan::contexts`, an extension of ADR-0007's priority): Proton, the custom
  `wine_prefix`, the game's own launcher prefixes, the `WinePrefix` roots, the shared launcher
  prefixes, then the native context. A path is used once, at its first place. An own prefix is
  not checked for other games.
- **Restore default** (`restore::default_targets`, ADR-0005): the custom `wine_prefix`, else its
  Proton prefix, else the game's launcher prefix, as in the scan: a Steam app id is stronger
  evidence than a title match.
  Amended by ADR-0023: then the prefix the snapshot names, by launcher and name, then the one
  prefix that holds its save folders.
- **Switching a prefix off:** `config.yaml` gains `ignored_prefixes: [path]` (serde default,
  omitted when empty). It drops only the launcher entry: an ignored path is not scanned as a
  launcher prefix and is never a launcher default target, but the same path as a `WinePrefix`
  root or a custom `wine_prefix` is still used. Settings → Roots → **Found by salvajocs** lists
  each detected path once, with its launchers and mapped titles (several Heroic games can share
  the default prefix). Unticking a row adds its path to `ignored_prefixes` and ticking it
  removes the path; the change is saved through `State::save_config`. The list is shown on
  Linux only, detected in a background `Job` through `Env::environment`, so GUI tests inject
  it. It is detected again when another manifest loads.

## Alternatives considered

- **Read Lutris' `pga.db`:** it would need a SQLite dependency, for relative paths only.
- **Check every launcher prefix for every game, like a `WinePrefix` root:** each prefix adds one
  manifest-wide glob pass, and a game would be found in another game's prefix. Only Bottles and
  unmapped prefixes are shared.
- **Ask before using a title-only match:** rejected; an unticked checkbox covers a wrong match.
- **Pick the first game of an ambiguous normalized title:** it may be the wrong game, so the
  prefix is shared instead.
- **Windows and macOS:** these launchers' Wine prefixes are a Linux case.

## Consequences

- Each shared prefix adds a glob pass per scan (ADR-0008 cost).
- A launcher prefix that was never run has no user, so the scan reports it like any other
  prefix. Unticking the prefix silences the report.
- Lutris games whose paths are relative are not found. A `WinePrefix` root or a custom
  `wine_prefix` covers them.
- Before 0.4.0, configs have no `ignored_prefixes`, so every detected prefix is in use.
