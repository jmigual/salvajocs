# CLAUDE.md

## Scratch space

Tests, experiments and extra builds go in the repo's git-ignored `tmp/` folder (extra cargo
dirs as `CARGO_TARGET_DIR=<repo>/tmp/<name>`). A worktree's normal build stays in its own
`target/`. Never put builds or large files in the `/tmp` scratchpad: it is RAM. Tell every agent
this in its prompt.

## GUI tests (crates/salvajocs-gui)

- Drive the harness with `run_until`, `run_settled` or `harness.step()` (helpers in
  `tests/common/mod.rs`). Never call plain `Harness::run` while a job is in flight: a running job
  keeps asking for redraws, so `run` panics or flakes.
- `State::save_config` is the only code that writes the config file. It runs in the background
  and queues a second save if one is running. Do not call `AppConfig::save` from the GUI.
- Tests never hit the network or the real system: inject through the `Env` seam (`src/env.rs`;
  `env_with` in `tests/common`).
