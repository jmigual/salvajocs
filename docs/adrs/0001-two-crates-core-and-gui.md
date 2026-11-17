# ADR-0001: Two crates, `salvajocs-core` and `salvajocs`

**Status:** Accepted
**Date:** 2026-11-16

## Context

The tool has one front-end (a desktop GUI) and a body of logic that must be unit-testable without a
window: manifest parsing, path translation, registry parsing, repository access. The Rust
project-setup guidance found (beagle `rust-project-setup`; matklad "Large Rust Workspaces") says a
single crate unless there are several targets, otherwise a virtual workspace with flat `crates/*`,
resolver 3, edition 2024 and `[workspace.package]`/`[workspace.dependencies]`/`[workspace.lints]`.
The reference project (`modular-scheduler`) splits core / tools / cli / gui.

## Decision

A virtual workspace with two crates:

- `crates/salvajocs-core` (lib): everything that is not drawing — manifest, layout, registry,
  prefix discovery, scan, storage (rustic_core), cross-build mapping, backup, restore, config. Each module owns a `thiserror`
  error type; no `anyhow` in core.
- `crates/salvajocs-gui` (package `salvajocs`, lib + bin `salvajocs`): egui front-end only (ADR-0002). Uses `anyhow`
  at its edge. Depends on core; core never depends on gui.

Workspace `Cargo.toml`: `resolver = "3"`, `edition = "2024"` in `[workspace.package]`, every
dependency pinned once in `[workspace.dependencies]`, `[workspace.lints.rust] unsafe_code =
"forbid"`. Unit tests in `<module>/tests.rs` beside the code; integration tests in `tests/`.

## Alternatives considered

- **One crate:** gui deps (eframe, wgpu for tests) would compile for every core test; a CLI added
  later would drag them in too.
- **core / tools / cli / gui like the reference:** a `tools` layer and a CLI have no caller in v1.
  A CLI can be added later as a thin crate over core without moving code.

## Consequences

- `cargo test -p salvajocs-core` runs the domain tests without building any GUI code.
- If a CLI is added later it calls the same core functions the GUI calls.
