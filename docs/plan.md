# Implementation plan (v1)

Each step is sized for one coder agent. Read `docs/adrs/README.md` first, then the ADRs the step
names. Every step ends with `cargo build`, `cargo test` and `cargo clippy --all-targets -- -D
warnings` passing for the workspace. That check is implied below and not repeated.

Storage runs in-process (rustic_core, ADR-0003), so repository tests are ordinary tests against
a temp local repo. Only tests that need the network or an OS credential store are `#[ignore]`.

Dependency graph:

- 1 → {2, 3, 4, 5, 6} in parallel
- 7 ← 6
- 8 ← 2, 3, 6; 9 ← 2; 10 ← 4, 7
- 11 ← 2, 3, 5, 8
- 12 ← 4, 6, 10, 11
- 13 ← 2, 3, 8
- 14 ← 3, 4, 13
- 15 ← 5, 12, 14
- 16 ← 8, 9; 17 ← 12, 16; 18 ← 15, 16; 19 ← 16

## 1. Workspace skeleton (blocks everything)

- **Goal:** empty workspace that builds, tests and lints clean (ADR-0001).
- **Files:**
  - `.gitignore`: `/target`. The repo is already initialised (branch `main`, remote `origin` =
    github.com/jmigual/salvajocs).
  - Root `Cargo.toml`: virtual workspace, `resolver = "3"`, `[workspace.package] edition =
    "2024"`, `rust-version = "1.91"` (rustic's MSRV). `[workspace.dependencies]` has the crates
    the ADRs name: serde, serde_json, serde-saphyr, thiserror, anyhow, reqwest, dirs, glob,
    `rustic_core = "=0.13.0"`, `rustic_backend = "=0.7.0"`, eframe, egui, egui_kittest, rfd,
    tempfile. `[workspace.lints.rust] unsafe_code = "forbid"`.
  - `crates/salvajocs-core/{Cargo.toml,src/lib.rs}`.
  - `crates/salvajocs-gui/{Cargo.toml,src/lib.rs,src/main.rs}`: `main` opens an empty eframe
    window titled "salvajocs".
  - Add only the dependencies a crate actually uses. Later steps add theirs to their crate.
- **Accept:** `cargo build && cargo test && cargo clippy --all-targets -- -D warnings` exit 0.

## 2. Manifest model and parsing (depends on 1; parallel with 3–6)

- **Goal:** typed subset of the ludusavi schema, YAML parse, JSON cache round trip, custom-game
  merge (ADR-0008).
- **Files:** `core/src/manifest.rs`, `core/src/manifest/tests.rs`,
  `core/tests/fixtures/manifest-sample.yaml`. The fixture is a handful of real entries copied by
  hand: files with `when`, registry, `installDir`, `steam.id`, an `alias`, and a Unity game with
  both Linux and Windows entries.
- **Content:**
  - `Manifest`, `GameEntry`, `FileRule`, `RegistryRule`, `When`, `Os`, `Store`, `Tag`.
  - `CustomGame { name, entry: GameEntry, integration: Integration, wine_prefix:
    Option<PathBuf>, pairs: Vec<Pair> }`, `Integration::{Override, Extend}` and
    `Pair { linux, windows }` (ADR-0011).
  - `parse_yaml(&str)` with raised serde-saphyr budget limits (ADR-0008), and
    `to_json`/`from_json`.
  - `merge_custom(&mut Manifest, &[CustomGame])`.
- **Accept:** `cargo test -p salvajocs-core manifest`. Tests cover: an alias entry is skipped;
  an unknown field is ignored; YAML → JSON → struct is equal; override replaces; extend appends.

## 3. Layout: bases, contexts, templates, snapshot format (depends on 1; parallel)

- **Goal:** pure path logic and the snapshot format for ADR-0004/0005.
- **Files:** `core/src/layout.rs`, `core/src/layout/tests.rs`.
- **Content:**
  - `Base` enum.
  - `WinFolders` (with `native()` under `cfg(windows)` and `for_prefix(prefix, user)`),
    `LinuxFolders::native()`, `Roots`/`Root{path, kind}`. Both folder types carry `user`, the
    value of `<osUserName>` (`native()`: the OS user; `for_prefix`: the Wine user), read
    through `Context::user()`.
  - `Context::{WindowsNative(WinFolders), WinePrefix { prefix, folders }, Linux(LinuxFolders)}`.
    The prefix variant keeps the prefix path, which restore needs for `user.reg` and for
    case-insensitive matching.
  - `classify(template, &Context) -> (Base, rest)`, including the `<home>/AppData/{Roaming,
    Local,LocalLow}` normalization (ADR-0004).
  - `glob_free_prefix(template)`.
  - `Context::case_insensitive() -> bool`: `true` for `WinePrefix`. A native Windows filesystem
    already ignores case.
  - `glob_pattern(base_dir, rest, case_insensitive) -> String`: escapes `base_dir` and, when the
    flag is set, rewrites each ASCII letter of `rest` into `[xX]`, leaving letters inside an
    existing `[...]` class alone (ADR-0008).
  - `expand(template, &Context, &GameVars) -> Vec<(PathBuf, String)>`: (base dir, glob
    pattern), one per candidate base dir. A match's tail is `tail_of(match, base dir)`. Every
    pattern is built by
    `glob_pattern(base_dir, rest, ctx.case_insensitive())`, so callers cannot bypass the
    case-insensitive rewrite. `GameVars` holds `<game>`, `<base>` and `<root>` candidates and
    `<storeGameId>`; `<osUserName>` comes from the context; `<storeUserId>` becomes `*`.
  - `GameVars::new(title, install_dirs: Vec<String>, …)`: holds the `installDir` candidates,
    falling back to the title. It takes plain strings, so this step stays independent of
    step 2.
  - `base_dir(Base, &Context, &GameVars) -> Option<PathBuf>`; `None` for `WinAbsolute`.
  - `win_absolute_target(tail, &Context) -> Option<PathBuf>`: resolves a `WinAbsolute` tail
    (drive letter first) in a `WindowsNative` context.
  - `tail_of(real_path, base_dir)`.
  - `target_path(base_dir, tail, case_insensitive: bool)`.
  - `SnapshotMeta { format, title, roots: Vec<(Base, String)> }` (serde) and the names `files/`,
    `salvajocs.json` and `registry.json`.
- **Accept:** `cargo test -p salvajocs-core layout`. Must include:
  - `home_under_windows_context_is_win_home`;
  - `home_appdata_locallow_normalizes_to_win_local_app_data_low`;
  - `home_dot_config_normalizes_to_xdg_config` (and `.local/share` → `XdgData`);
  - `prefix_and_native_windows_map_same_base_to_same_tail`;
  - `linux_base_has_no_target_in_windows_context`;
  - `target_path_reuses_existing_dir_case_insensitively`;
  - `glob_pattern_matches_lowercase_dir`;
  - `glob_pattern_escapes_base_dir`;
  - `native_win_folders_exist` (`#[cfg(windows)]`, runs in step 7's Windows job).

## 4. Wine registry text format and the normalized model (depends on 1; parallel)

- **Goal:** ADR-0006, model and Wine side.
- **Files:** `core/src/registry.rs` (model `RegistryExport`, `RegValue`, JSON (de)serialization),
  `core/src/registry/wine.rs`, `core/src/registry/wine/tests.rs`,
  `core/tests/fixtures/user.reg`. Build it as a trimmed excerpt of a real Proton `user.reg`;
  there are some under `/home/jmigual/.local/share/Steam/steamapps/compatdata/*/pfx/`. Keep the
  header and a few key blocks, and strip anything personal. It must contain at least one
  non-ASCII string, a `hex:` value with line continuation, `str(2):`, `str(7):` and `dword:`.
- **Content:**
  - `parse(&str) -> RegFile` (header and untouched blocks kept verbatim).
  - `RegFile::export(keys) -> RegistryExport` (absent keys give an empty export).
  - `RegFile::replace_subtrees(&RegistryExport)`.
  - `RegFile::to_string()`.
- **Verified here:** Wine escapes non-ASCII as `\x<hex>` per UTF-16 unit (see ADR-0006). A
  real prefix verified it for a key name; value strings are verified from wineserver's source
  only (no real prefix has one). The HKCU root and empty path elements are refused.
- **Accept:** `cargo test -p salvajocs-core registry`:
  - `parse_then_write_is_byte_identical`;
  - `export_then_replace_round_trips`;
  - `replace_drops_old_subkeys`;
  - one test per value kind.

## 5. Prefix and root discovery (depends on 1; parallel)

- **Goal:** ADR-0007.
- **Files:** `core/src/prefix.rs`, `core/src/prefix/tests.rs`.
- **Content:**
  - `validate_prefix(path)`.
  - `wine_user(prefix) -> Result<String, PrefixError>`. System dirs are `Public` and `root`. The
    error has an `Ambiguous` variant.
  - `steam_roots()`, `proton_prefix(steam_root, app_id)`.
  - `wineserver_running() -> bool` (Linux `/proc/*/comm`; `false` on Windows).
- **Verified in step 5:** the Flatpak Steam path is confirmed from the flathub sources; the
  Windows default `%ProgramFiles(x86)%\Steam` is only partly verified (see ADR-0007). A real Steam
  root for manual checks is `/home/jmigual/.local/share/Steam`; the dev `$HOME` (distrobox) is
  different.
- **Accept:** `cargo test -p salvajocs-core prefix`:
  - `proton_prefix_picks_steamuser`;
  - `single_user_prefix_picks_that_user`;
  - `root_and_public_dirs_are_not_users`;
  - `two_users_without_steamuser_is_ambiguous`;
  - `dir_without_drive_c_is_not_a_prefix`.

## 6. Storage over rustic_core (depends on 1; parallel)

- **Goal:** ADR-0003. The only module that names rustic types.
- **Files:** `core/src/storage.rs`, `core/src/storage/tests.rs`.
- **Content:**
  - `RepoLocation { repository: String, options: BTreeMap<String, String> }`, built into
    backends with `rustic_backend::BackendOptions`.
  - `probe(&RepoLocation, password) -> RepoState::{Ready, Missing, WrongPassword, Error}`.
  - `init(&RepoLocation, password, Compression)` and `open`. `Compression::{Off, Default,
    Level(i32)}` maps to the repo config's `set_compression`. `Compression::from_field(Option<i32>)`
    validates the first-run integer: `None` = default, `0` = off, `1..=22` and `-7..=-1` are
    levels, anything else is a typed error.
  - `compression()` and `set_compression()` (via `apply_config`).
  - `reindex()`: loads the index; called once at the start of each backup run (ADR-0003).
  - `backup_dir(src, as_path, tags, label, extra_tags) -> Option<SnapshotInfo>` sets
    `skip_if_unchanged`; `None` means skipped.
  - `snapshots(tag) -> Vec<SnapshotInfo{id, time, host, label, tags}>`.
  - `restore_path(snapshot_id, path_in_snapshot, dest_dir)`.
- **Accept:** `cargo test -p salvajocs-core storage` (temp local repos):
  - `init_and_open_with_empty_password`;
  - `wrong_password_is_reported`;
  - `missing_repo_is_missing_not_error`;
  - `compression_set_at_init_and_changed`;
  - `backup_with_as_path_then_restore_round_trip`;
  - `second_unchanged_backup_is_skipped` (reopen and reindex between the two runs, as two app
    sessions would);
  - `compression_level_out_of_range_rejected`;
  - `label_keeps_commas_and_unicode`;
  - `opendal_memory_backend_round_trip`: `opendal:memory` proves the non-local path works
    without network; verify the exact repository string here;
  - `snapshot_path_is_the_same_on_every_os`: asserts the stored path is `/salvajocs/<slug>`;
    the Windows half runs in step 7.

## 7. CI on GitHub Actions (depends on 6)

- **Goal:** every push runs the suite on Linux and Windows. This is the gate for every
  Windows-only check (`WinFolders::native()`, `windows-registry`, Known Folders, storage paths,
  keyring on Windows).
- **Files:** `.github/workflows/ci.yml`.
- **Content:**
  - Jobs on `ubuntu-latest` and `windows-latest`. Each runs:
    1. `cargo clippy --workspace --all-targets -- -D warnings`;
    2. `cargo test --workspace -- --include-ignored`.
  - Linux has no Secret Service daemon on the runner, so the Linux test command adds
    `--skip keyring_`.
  - Use the Cargo cache action. No release packaging.
- **Accept:** push a branch to `origin`; `gh run watch` shows both jobs green.

## 8. Config, password and backend secrets (depends on 2, 3, 6)

- **Goal:** ADR-0009.
- **Files:** `core/src/config.rs`, `core/src/config/tests.rs`, `core/src/secret.rs`,
  `core/src/secret/tests.rs`.
- **Content:**
  - `AppConfig { repository, repository_options: Vec<RepoOption{key, value: Option<String>,
    secret: bool}>, password: PasswordMode, roots: Vec<Root>, custom_games:
    Vec<manifest::CustomGame> }`.
  - `load(dir) -> Result<Option<AppConfig>>` (`None` = first run) and atomic `save(dir)`.
  - `AppConfig::location(&secrets) -> RepoLocation` merges secret option values.
  - `secret::{get, set, delete}` keyed by repository, or repository `#` option key.
  - `backend_kinds() -> Vec<BackendKind>`: local, rest, rclone, and the opendal services for
    this OS; no `sftp` on Windows (ADR-0003).
  - `missing_secrets(&AppConfig, lookup) -> Vec<SecretKey>`: the password (when its mode needs
    one) and every secret option that the keyring does not return.
- **Verify here:** which keyring crate(s) a library should depend on (`keyring` 4.x vs
  `keyring-core` plus store crates), and whether the Linux store persists across reboot. Update
  ADR-0009.
- **Accept:** `cargo test -p salvajocs-core config`:
  - `missing_file_is_first_run`;
  - `save_then_load_is_equal`;
  - `config_file_never_contains_password_or_secret_options`;
  - `backend_kinds_exclude_sftp_on_windows` (`cfg`-gated assertion);
  - `missing_secrets_lists_password_and_secret_options` (fake lookup).
  `keyring_set_get_delete` is `#[ignore]` and runs in step 7's Windows job.

## 9. Manifest fetch and cache (depends on 2)

- **Goal:** ADR-0008 network part.
- **Files:** `core/src/manifest/fetch.rs`, `core/src/manifest/fetch/tests.rs`.
- **Content:** `refresh(cache_dir) -> Result<Freshness::{NotModified, Updated}>` with ETag and
  gzip through `reqwest` blocking; `load_cached(cache_dir)`.
- **Verified here (recorded in ADR-0008):**
  - the reqwest 0.13 feature names (`blocking`, `rustls`, `gzip`);
  - the serde-saphyr budget limits needed for the full manifest (the default fails at 250 001
    nodes): about 1.9 M nodes and 2.6 M events.
- **Accept:**
  - `cargo test -p salvajocs-core fetch`: cache and ETag file handling, no network.
  - `#[ignore]` real download: asserts > 10 000 games parsed and a second call returns
    `NotModified`; prints the parse time.

## 10. Windows registry access (depends on 4, 7)

- **Goal:** ADR-0006 Windows side.
- **Files:** `core/src/registry/windows.rs` and its `tests.rs` (`cfg(windows)`).
- **Content:** `export(keys) -> RegistryExport` and `replace_subtrees(&RegistryExport)` through
  `windows-registry`, HKCU only, spelled as the registry stores each key; not atomic (the restore
  safety backup of step 15 covers failures).
- **Accept:** step 7's Windows job is green and includes `windows_registry_round_trip` under
  `HKCU\Software\salvajocs-test`.

## 11. Scan (depends on 2, 3, 4, 5, 8, 10)

- **Goal:** find every game's save files on this machine and list the registry keys to export.
- **Files:** `core/src/scan.rs`, `core/src/scan/tests.rs`.
- **Content:**
  - `scan(&Manifest, &AppConfig, &Environment) -> ScanReport { games: Vec<GameScan>, problems:
    Vec<ScanProblem> }`. `Environment` holds the native context (which carries its OS user
    name) and the Steam roots, built by `Environment::detect()` and constructible in tests.
    Prefixes come from `config.roots` (`WinePrefix`), a custom game's `wine_prefix` and Proton
    lookups under the Steam roots.
  - `GameScan { title, files: Vec<FoundFile{base, tail, real, size}>, roots: Vec<(Base,
    String)>, registry_keys, registry_source: Option<RegistrySource>, unreadable: Vec<PathBuf>
    }`. `roots` holds the glob-free prefixes of templates that matched, as raw template text
    (a `<game>` template keeps `<game>` in `roots`). `registry_keys` is the manifest's
    applicable HKCU list. `RegistrySource` is `Windows` or `WinePrefix(PathBuf)`.
  - Registry: walk the contexts in priority order and record the first whose export of the
    game's keys is non-empty. A prefix's `user.reg` is parsed with `registry::wine` once per
    prefix per scan; native Windows uses `registry::windows::export`. A game is kept when it
    has files or a `registry_source`. Backup reads the keys fresh from the source.
  - Errors: `WinePrefix` roots are resolved once per scan. A `PrefixError` (such as
    `Ambiguous`) becomes a `ScanProblem { path, kind }` instead of being dropped. A
    `read_dir` or glob failure becomes a problem and is added to that game's `unreadable`. So
    does an unreadable or malformed `user.reg` (or a failed Windows export): the registry walk
    stops there, so a lower-priority source never stands in for it.
    Nothing is dropped silently.
  - Prefix priority and first-wins per logical path (ADR-0007). Dedupe by real path. Patterns
    come from `layout::expand`; each match's tail is `tail_of(match, base dir)` with the base dir
    of the pair that produced it.
- **Measure here:** wall time of a full-manifest scan against one `WinePrefix` root (a real
  prefix under `/home/jmigual/.local/share/Steam/steamapps/compatdata/`). Record it in
  ADR-0008.
- **Accept:** `cargo test -p salvajocs-core scan`:
  - `proton_game_files_get_win_bases`;
  - `native_linux_game_files_get_linux_bases`;
  - `same_file_from_two_templates_listed_once`;
  - `game_specific_prefix_beats_wine_prefix_root`;
  - `prefix_scan_matches_case_insensitively`;
  - `roots_record_glob_free_prefixes` (also asserts a `<game>` template keeps `<game>` in
    `roots`);
  - `registry_only_game_is_offered`;
  - `registry_source_is_first_context_with_keys`;
  - `user_reg_parsed_once_per_prefix`;
  - `ambiguous_wine_prefix_root_reported`;
  - `unreadable_dir_reported_not_dropped`;
  - `os_user_name_comes_from_the_context`,
    `prefixes_differing_in_case_list_a_logical_path_once` and
    `extend_custom_game_keeps_the_manifest_files`.

## 12. Backup (depends on 4, 6, 10, 11)

- **Goal:** stage and back up per game (ADR-0004, 0010).
- **Files:** `core/src/backup.rs`, `core/src/backup/tests.rs`, `core/src/slug.rs`.
- **Content:**
  - `slug(title)` (FNV-1a 32).
  - `stage(game_scan, registry: &RegistryExport, stage_dir)` syncs the mirror (copy when size or
    mtime differ, then `File::set_modified`; remove files whose source is gone). It writes
    `SnapshotMeta` (with `roots`) and `registry.json` only when their bytes change. The
    registry export is read fresh from the scan's `registry_source`; an empty export is dropped
    (no `registry.json`). Nothing is removed from the mirror, and `SnapshotMeta` keeps the roots
    it already lists, when the scan reported `unreadable` paths for the game.
  - `backup_games(&mut Storage, games: &[GameScan], stage_root, extra_tags, progress) ->
    Result<Vec<GameBackup>>`: reindexes once, then one game after another, staged in
    `stage_root/<slug>`, `as_path = /salvajocs/<slug>`, `label` = title, tags `salvajocs` and
    `game:<slug>` plus `extra_tags`. Only the reindex fails the call; each `GameBackup` holds
    its game's result. The registry export comes from `registry::wine` (prefix) or
    `registry::windows` (native).
- **Measure here:** wall time of one reindex plus 20 sequential per-game backups on a local repo.
  Note it in ADR-0010.
- **Accept:** `cargo test -p salvajocs-core backup`:
  - `slug_is_stable`;
  - `stage_layout`;
  - `restage_unchanged_keeps_inode_and_mtime`;
  - `restage_removes_deleted_source`;
  - `backup_creates_tagged_labelled_snapshot_per_game`;
  - `unchanged_game_makes_no_new_snapshot`;
  - `registry_exported_from_recorded_source`;
  - `restage_keeps_copies_when_source_unreadable`.

## 13. Cross-build mapping (depends on 2, 3, 8; parallel with 11, 12)

- **Goal:** ADR-0011, pure.
- **Files:** `core/src/crossbuild.rs`, `core/src/crossbuild/tests.rs`.
- **Content:**
  - `ENGINE_RULES`: Unity and Godot as data (ADR-0011).
  - `propose(&SnapshotMeta, Option<&GameEntry>, &[Pair], target_family) -> Vec<MappingEntry>`.
    `MappingEntry { root, files, proposed: Option<String>, confidence, candidates, notes }`.
    Order: saved pair, engine rule, auto-pair, unknown. `Saved`, `EngineRule` and `AutoPair`
    entries come back already resolved; only `Unknown` needs a choice (ADR-0011).
  - `Choice::{Template(String), Skip}`. A template must classify with `layout::classify` in the
    target family (ADR-0011); real paths are not accepted.
  - `resolve(&[MappingEntry], &[(root, Choice)]) -> Result<Vec<ResolvedMapping>, ResolveError>`.
    A choice for a root replaces its proposal, whatever the confidence. It errors on an
    unclassifiable template and on any unknown root without a choice.
  - `apply_choices(&mut AppConfig, title, &[(root, Choice)])` stores choices as `Pair`s on the
    game's custom entry, creating an `extend` custom game if needed.
- **Accept:** `cargo test -p salvajocs-core crossbuild`:
  - `auto_pairs_single_linux_and_windows_save_dir`;
  - `no_auto_pair_when_ambiguous`;
  - `custom_pair_maps_tail`;
  - `unity_rule_pairs_persistent_data_path`;
  - `unity_playerprefs_registry_adds_note`;
  - `godot_rule_pairs_app_userdata`;
  - `unity_rule_matches_home_dot_config_spelling`;
  - `auto_pair_single_file_template_maps_with_empty_tail` (`<home>/.config/_12ibt6/savedata.ini`);
  - `auto_pair_file_glob_uses_parent_dir` (the 1001 Spikes shape);
  - `unknown_root_lists_target_os_candidates`;
  - `user_choice_saved_as_pairing`;
  - `resolve_rejects_unclassifiable_template`;
  - `resolve_choice_overrides_engine_rule_proposal`;
  - `resolve_requires_every_unknown_root`.

## 14. Restore plan (depends on 3, 4, 13)

- **Goal:** the pure mapping from a staged game to target paths (ADR-0005, 0011).
- **Files:** `core/src/restore.rs` (types, `plan`), `core/src/restore/tests.rs`.
- **Content:**
  - `StagedGame { root, meta: SnapshotMeta, files: Vec<(Base, tail)>, registry:
    Option<RegistryExport> }`.
  - `Targets { linux: Option<LinuxFolders>, win: Option<Context> }`.
  - `plan(&StagedGame, &Targets, &GameVars, Option<&GameEntry>, &[Pair], &[ResolvedMapping])
    -> Result<RestorePlan, PlanError>`. The entry is the game's manifest entry with its custom
    game applied; the pairs are its saved pairs.
    - Bases resolve against their own family's target. `WinAbsolute` resolves through
      `win_absolute_target`, not `base_dir`.
    - Files of a family with no target go through the mapping.
    - `registry.json` with no `win` target is skipped with "registry: no Windows target".
    - A snapshot is untrusted (ADR-0005): a file under no `SnapshotMeta.roots` entry is skipped
      with "outside the game's save roots". A root is validated against the entry: it must
      equal the classified glob-free prefix of an entry file template, a pair side or an
      engine-rule target of those, else its files are skipped with "save root not declared for
      this game" (all of them without an entry). Only registry keys at or below a key the entry
      declares (case-insensitive) are restored, others are skipped with "registry key not
      declared for this game", and without an entry every key is skipped.
    - `<osUserName>` stays literal in the roots and matches exactly one tail component. A
      write's tail is the root's rest (or the mapping target's) with `<osUserName>` replaced by
      the target family's `user`, plus the matched remainder (ADR-0004). Of two roots matching
      to the same depth, the one with fewer `<osUserName>` components wins.
    - A mapping whose target is `Absolute`/`WinAbsolute` is skipped as having no target.
    - `RestorePlan { writes, registry, skipped, cross_build: bool, notes }`.
  - `prepare(&StagedGame, &Targets, &GameVars, Option<&GameEntry>, &[Pair],
    &[(root, Choice)]) -> Result<RestorePlan, PrepareError>`. This is the one call the GUI
    makes: it runs `crossbuild::propose`, `resolve` and `plan`, and returns
    `NeedsMapping(Vec<MappingEntry>)` while choices are missing (ADR-0011).
    - `SnapshotMeta.roots` are raw glob-free template prefixes (`<game>` etc. unsubstituted), and
      so are the proposed targets. `prepare`/`plan` substitute `GameVars` into the root's rest
      to match file tails and into the resolved target.
    - A scan's `roots` keep the template's case while file tails keep the disk's, so matching a
      file to a root ignores case for `Win*` bases.
    - `prepare` fills `MappingEntry.files` from `StagedGame.files` (`propose` leaves 0).
    - `prepare` adds the Unity PlayerPrefs note when `StagedGame.registry` has the matching
      HKCU key (`propose` only checks the manifest entry; ADR-0011's "or registry.json").
    - `prepare` copies every `MappingEntry.notes` into `RestorePlan.notes`, whether the root
      was mapped automatically or chosen.
    - `crossbuild::propose` does not propose `Absolute`/`WinAbsolute` roots; they restore to
      the same OS only.
  - `RestorePlan::override_target(index, PathBuf) -> Result<()>`: per-file override for this
    restore only; the path must be absolute (ADR-0005).
- **Accept:** `cargo test -p salvajocs-core restore::tests`:
  - `proton_save_plans_into_windows_folders`;
  - `windows_save_plans_into_prefix_user`;
  - `native_linux_save_restores_without_prefix`;
  - `mixed_snapshot_restores_both_families`;
  - `game_dir_resolves_through_install_dir`;
  - `game_dir_skipped_when_not_installed`;
  - `cross_build_unknown_entry_blocks_restore` (via `prepare`, which returns `NeedsMapping`);
  - `prepare_with_choices_returns_plan`;
  - `prepare_engine_rule_returns_plan_without_choices`;
  - `linux_save_maps_onto_windows_via_pair` (sets `cross_build`);
  - `registry_skipped_without_windows_target`;
  - `override_target_rejects_relative_path`;
  - `os_user_name_in_tail_is_swapped_proton_to_windows`;
  - `mixed_restore_uses_each_family_user`;
  - `mapped_root_with_os_user_name_covers_file`;
  - `unity_note_kept_when_mapped_automatically`;
  - `file_outside_recorded_roots_skipped`;
  - `undeclared_registry_key_skipped`;
  - `registry_skipped_without_game_entry`;
  - `linux_snapshot_restored_into_proton_build`;
  - `crafted_root_cannot_read_outside_the_staged_files`;
  - `store_game_id_substituted_into_root`;
  - `snapshot_registry_keys_do_not_declare_themselves`;
  - `os_user_name_does_not_match_dot_dot`;
  - `crafted_root_covering_home_is_skipped`;
  - `declared_root_still_restores`;
  - `pair_side_root_is_trusted`;
  - `os_user_name_root_loses_a_tie_to_a_literal_root`.

## 15. Restore list, fetch, safety backup, apply (depends on 5, 12, 14)

- **Goal:** ADR-0005, 0006, 0010 runtime side.
- **Files:** `core/src/restore/run.rs`, `core/src/restore/run/tests.rs`.
- **Content:**
  - `list(&Storage) -> Vec<GameBackups>`, grouped by slug, titled by `label`. The default pick
    is the newest snapshot without `pre-restore`.
  - `default_targets(title, &Manifest, &[CustomGame], &Environment) -> Targets`.
  - `fetch(&Storage, snapshot, work_dir) -> StagedGame` restores `/salvajocs/<slug>`.
  - `run(&Storage, &RestorePlan, wineserver_running: bool) -> Result<RestoreReport>`. It is the
    only public way to write:
    1. registry writes into a `WinePrefix` while `wineserver_running` → typed error;
    2. safety backup of the existing targets (tag `pre-restore`). Overridden targets are staged
       under `Absolute`/`WinAbsolute` with their full path. It reindexes once and writes
       `SnapshotMeta` with `roots` for the target roots it covers. Step 12's
       `backup_games(&mut Storage, games, stage_root, &["pre-restore"], progress)` does this
       given a `stage_root` of its own (not the backup mirror's); a game's failure is in its
       `GameBackup`, not in the outer `Result`. Any error → nothing written;
    3. apply files and registry. A staged source is copied only when its `symlink_metadata`
       is a regular file. A target whose final component is an existing symlink is refused
       with "target is a symlink"; symlinked parent directories are allowed (Proton's legacy
       dirs).
- **Accept:** `cargo test -p salvajocs-core restore::run`:
  - `registry_restore_refused_while_wineserver_runs`;
  - `apply_aborts_when_safety_backup_fails` (the repository location is made unreachable;
    targets are unchanged);
  - `list_default_skips_pre_restore`;
  - `default_targets_uses_proton_compatdata`;
  - `default_targets_prefers_custom_wine_prefix`;
  - `default_targets_linux_only_without_prefix`;
  - `safety_backup_covers_planned_targets_in_user_picked_prefix`;
  - `safety_backup_meta_has_roots`;
  - `safety_backup_covers_overridden_target`;
  - `proton_backup_restores_into_windows_layout`;
  - `prefix_registry_restores_into_other_prefix`;
  - `staged_symlink_not_restored`;
  - `symlinked_target_file_refused`.

## 16. GUI shell, jobs, first run, Settings (depends on 8, 9)

- **Goal:** ADR-0002, 0009 UI.
- **Files:**
  - `gui/src/app.rs`, `gui/src/job.rs` (+ `job/tests.rs`, copied from the reference project's
    `Job<T>` with its `ended`/`finished` split).
  - `gui/src/first_run.rs`, `gui/src/widgets/backend.rs`, `gui/src/tabs.rs`,
    `gui/src/tabs/settings.rs`.
  - `gui/tests/common/mod.rs`, `gui/tests/smoke.rs`, `gui/tests/settings.rs`.
- **Content:**
  - Tabs Backup / Restore / Custom games / Settings (placeholders for the first three).
  - Backend widget (shared by first run and Settings): a kind picker from
    `config::backend_kinds()`, a location field (folder picker for local), and a key/value
    option list with a "secret" checkbox per row. The OS caveats are shown inline.
  - First run: backend, compression (one integer field validated by
    `Compression::from_field`), password, remember. The probe runs in a `Job`; `Missing` asks
    "Create a new repository at <location>?" before init.
  - Startup: if `config::missing_secrets` is not empty, a prompt asks for exactly those values
    before the repository is opened.
  - Settings: the backend widget, the repo's compression (read and change), password mode and the
    roots list.
  - Settings: when the repository location changes, offer to delete the old location's keyring
    entries (`secret::delete` with the old `AppConfig::secret_scope`).
  - Manifest load and refresh in a `Job` on start, with a banner on failure.
- **Accept:** `cargo test -p salvajocs-gui`:
  - `smoke_renders_every_tab`;
  - `first_run_modal_shown_without_config`;
  - `missing_repo_asks_before_init`;
  - `backend_option_marked_secret_not_written_to_config`;
  - `startup_prompts_for_missing_secret_option`;
  - `compression_field_shows_error_for_out_of_range_level`;
  - `job_result_survives_second_poll`.

## 17. GUI Backup tab (depends on 12, 16)

- **Goal:** list scanned games, select, preview files, back up with progress.
- **Files:** `gui/src/tabs/backup.rs`, `gui/tests/backup.rs`.
- **Accept:** `cargo test -p salvajocs-gui backup`: with a fixture environment, the scan `Job`
  completes (`Harness::step` loop) and the game rows and file counts are shown.

## 18. GUI Restore tab and mapping dialog (depends on 15, 16)

- **Goal:**
  - games from snapshots, with a snapshot picker (time, host, "pre-restore" label);
  - targets preselected from `restore::default_targets`. On Linux the control always offers
    both "Windows build in prefix" and "native Linux build (cross-build mapping)", whatever
    bases the snapshot has;
  - engine-rule and auto-pair mappings need no dialog; they go straight to the preview;
  - only on `NeedsMapping` (at least one unknown root), the mapping dialog: one row per root of
    the game with source path, file count, proposal, confidence, a candidate dropdown plus free
    text, notes (e.g. Unity PlayerPrefs), and skip. Known roots are prefilled and editable;
    unknown ones start empty. Free text must be a placeholder template; `prepare`'s error is
    shown inline.
  - confirm calls `restore::prepare` again with the choices and saves all of the game's roots via
    `crossbuild::apply_choices`;
  - a plan preview with writes, skips, the cross-build warning and `RestorePlan.notes`.
    Writes to `Absolute`/`WinAbsolute` targets are flagged. Each write's target can be
    overridden for this restore (`RestorePlan::override_target`). Then `restore::run`.
- **Files:** `gui/src/tabs/restore.rs`, `gui/src/tabs/restore/mapping.rs`, `gui/tests/restore.rs`.
- **Accept:** `cargo test -p salvajocs-gui restore`:
  - `preview_lists_writes_and_skips`;
  - `apply_disabled_until_plan_loaded`;
  - `engine_rule_mapping_needs_no_dialog` (a Unity snapshot goes straight to the preview, with
    the cross-build warning);
  - `mapping_dialog_prefills_known_roots_and_blocks_unknown` (dialog shown only with an unknown
    root; known rows are prefilled and editable; Confirm is disabled until every unknown row is
    chosen or skipped);
  - `dialog_confirm_saves_all_roots_as_pairs`;
  - `cross_build_warning_shown`;
  - `prefix_found_but_user_picks_native_linux_mapping`;
  - `typed_template_that_does_not_classify_shows_error`;
  - `preview_per_file_override_changes_one_write`.

## 19. GUI Custom games editor (depends on 16)

- **Goal:** add/edit/remove custom games:
  - name;
  - override/extend;
  - file templates and registry keys;
  - optional `wine_prefix`;
  - `pairs` (Linux ↔ Windows template rows).
  Saved to config.
- **Files:** `gui/src/tabs/custom_games.rs`, `gui/tests/custom_games.rs`.
- **Accept:** `cargo test -p salvajocs-gui custom_games`: adding a game with one pair through the
  UI and saving produces the expected `AppConfig.custom_games` entry.

## Out of scope for v1

- Unity PlayerPrefs conversion (registry ↔ prefs file). Possible follow-up; v1 only detects it
  and explains it in the mapping dialog.
- Engine rules beyond Unity and Godot.
- Auto-discovery of Heroic, Lutris and Bottles prefixes. Steam library folders from
  `libraryfolders.vdf`. Non-Steam shortcuts.
- HKLM registry keys. Translating `<storeUserId>` inside tails.
- macOS.
- Retention (`forget`/`prune`), scheduled or automatic backups, a CLI front-end, release
  packaging.
- Per-prefix wineserver detection (v1 refuses registry writes while any wineserver runs).
- Per-service forms for backends (v1 uses a generic key/value list).
