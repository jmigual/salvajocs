# CLAUDE.md

## Scratch space

Tests, experiments and extra builds go in the repo's git-ignored `tmp/` folder (extra cargo
dirs as `CARGO_TARGET_DIR=<repo>/tmp/<name>`). A worktree's normal build stays in its own
`target/`. Never put builds or large files in the `/tmp` scratchpad: it is RAM. Tell every agent
this in its prompt.
