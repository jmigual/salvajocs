# ADR-0008: Ludusavi manifest fetched with ETag, parsed with serde-saphyr, cached as JSON

**Status:** Accepted
**Date:** 2026-11-16

## Context

Ludusavi's default manifest URL is
`https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/refs/heads/data/data/manifest.yaml`.
It is about 17.8 MB raw and 2.4 MB gzipped, and supports `ETag`/`If-None-Match` (304).
Ludusavi caches a JSON copy because parsing the YAML is slow. The schema to implement is
`data/schema.yaml`, not the strict variant (research.md §1).

YAML crates on crates.io (checked 2026-11-16):

- `serde_yaml` 0.9.34+deprecated.
- `serde_yml` 0.0.13, still pre-0.1.
- `serde_norway` 0.9.42, last updated 2024-12.
- `yaml_serde` 0.10.7, the YAML org's continuation of `serde_yaml`, 2026-08.
- `serde-saphyr` 2.1.1, 2026-11-05. It deserializes straight into types with no value tree,
  has parse budgets, and has `to_string` for writing.

Measured on 2026-11-16 by parsing the live manifest with serde-saphyr 2.1.1 into a typed subset,
in a release build:

- The default budget **fails**: "budget breached: Nodes { nodes: 250001 }" at line 117 139.
- With the budget disabled (`Options.budget = None`), the full file parses in about 1.6 s.

`reqwest` (blocking, rustls) is already in the dependency tree through rustic_backend's `rest`
feature (ADR-0003).

## Decision

- **Fetch:** `reqwest` blocking client, run inside a GUI `Job`, with `If-None-Match` from a
  stored ETag. Its `gzip` feature is enabled in our dependency. The exact reqwest 0.13 feature
  names are to verify in plan step 9. A separate HTTP crate (ureq) is not added.
- **Parse:** `serde-saphyr` with the budget's limits raised above the manifest's real size, with
  headroom. Alias and anchor limits stay. The values are measured in plan step 9. The typed
  subset ignores unknown fields:
  - `files: {template: {tags, when: [{os, store}]}}`
  - `registry: {key: {tags, when: [{store}]}}`
  - `installDir: {name: {}}`
  - `steam.id`
  - `alias`: entries that have it are skipped as nicknames.
- **Cache:** `<cache>/salvajocs/manifest.json` (serde_json of the typed subset) plus
  `manifest.etag`. On start the cache is loaded and a refresh runs in the background. With no
  cache and no network, only custom games are available, and a banner says so.
- **Placeholders/rules** per research.md §1: substitute, then glob; a folder matches recursively;
  relative paths are ignored. `<storeUserId>` becomes `*`. A `when.os` entry is evaluated against
  the context: Windows-like matches `windows`, Linux-native matches `linux`. `when.store` matches
  the root kind when known.
- **Case-insensitive globbing inside Wine prefixes.** On a case-sensitive filesystem, a
  manifest `My Games/Foo` must still find a real `my games/foo`; otherwise the save is silently
  never backed up. `glob::MatchOptions { case_sensitive: false }` is not enough: the `glob`
  crate checks a part without wildcards with `fs::metadata`, which is case-sensitive.
  - Each pattern is built as `Pattern::escape(resolved base dir)` + `/` + the template's rest.
    The base dir is escaped and stays case-sensitive (it is a host path).
  - Inside a Wine prefix, every ASCII letter in the rest becomes a two-case class
    (`My` → `[mM][yY]`). Letters already inside a `[...]` class are left alone.
  - This makes every part a wildcard part, so `glob` lists the directory and compares, for
    literal and wildcard parts alike. It needs no new dependency.
  - Known limit: each part is now a directory listing instead of one `stat`. A `WinePrefix`
    root is checked against every manifest game (ADR-0007), so shared dirs like
    `AppData/Roaming` are listed thousands of times per scan. Plan step 11 times this. If it is
    too slow, the fix is to cache directory listings per scan, not to drop case-insensitivity.
  - Rejected: a hand-written per-component `read_dir` walk. It would reimplement `*`/`**`/`?`
    matching that `glob` already does.
- **Custom games** (stored in config, ADR-0009; the `CustomGame`, `Integration` and `Pair` types
  live in `core::manifest`) reuse the same entry type, plus:
  - `integration: override | extend`: `override` replaces the manifest entry of the same title;
    `extend` appends its files and keys.
  - `wine_prefix` (ADR-0007).
  - `pairs: [{linux, windows}]` for cross-build restores (ADR-0011).
- The same crate (`serde-saphyr`) writes the YAML config, so there is one YAML dependency.

## Alternatives considered

- **`yaml_serde`:** drop-in and maintained, but it builds the classic serde_yaml path. Kept as
  the fallback if raising the budget proves awkward in step 9.
- **Parse YAML on every start without a cache:** 1.6 s measured, plus the download. The JSON
  cache avoids both.
- **Bincode-style cache:** an extra dependency. JSON is already in.
- **ureq:** small, but a second HTTP stack next to rustic_backend's reqwest.

## Consequences

- The first start needs the network to have any manifest games.
- The typed subset gains fields only when a feature uses them (e.g. `gog.id` for a future
  launcher).
