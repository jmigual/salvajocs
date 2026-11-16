# ADR-0002: GUI with egui/eframe, headless tests with egui_kittest

**Status:** Accepted
**Date:** 2026-11-16

## Context

Candidates (research.md §7): iced 0.14.0 (what ludusavi uses), egui/eframe 0.37.0 (2026-10-15),
slint 1.19.0. The UI is a few lists with checkboxes, forms and progress — no custom rendering. The
user already maintains an egui app (`modular-scheduler`'s `fms-scheduler-gui`) with a background
`Job<T>` pattern and `egui_kittest` headless tests.

## Decision

`eframe`/`egui` 0.37 for the window, `egui_kittest` 0.37 as a dev-dependency for headless widget
tests, `rfd` for native folder pickers. Copy the reference project's conventions:

- The GUI is thin: anything that touches disk, network or the repository runs in a `Job<T>` (worker thread
  + channel + `request_repaint`). A blocking call in a `ui` method is a bug.
- Tests that wait on a job loop with `Harness::step`, never `Harness::run`; shared helpers in
  `tests/common/mod.rs`.
- Logic sits in core as pure functions and is unit-tested there; widget tests only prove the tree
  renders and wires up.

## Alternatives considered

- **iced:** proven for exactly this app (ludusavi), but Elm-style message plumbing for every form
  field costs more code, and the user has no existing iced test harness.
- **slint:** markup language plus a separate toolchain step; licensing terms to check; no gain for
  plain forms and lists.

## Consequences

- Immediate-mode redraws mean background work must wake the UI explicitly (the `Job` does it).
- Same headless test approach as the user's other project; no display needed for `cargo test`.
