# Progress

## Current State

- Initialized the repository and created the initial Rust binary scaffold.
- The app currently has a single entry point in `src/main.rs` and prints
  `hello world`.
- Cargo is the Rust build system; `make` is the project task runner.
- Nix provides the reproducible development shell and package build.
- Codex project hooks are configured to run Rust formatting after Codex edits.

## Verified

- `make run` prints `hello world`.
- `make check` passes formatting, Clippy, and tests.
- `make build` passes.
- `nix build` passes.
- `.codex/hooks/rustfmt.sh` runs successfully.

## Notes

- Build work should proceed one step at a time.
- Step-specific notes belong in `build-notes/$step.md`.
- Update this file whenever a step is completed or the project state changes.
