# Progress

## Current State

- Initialized the repository and created the initial Rust binary scaffold.
- Completed `core-layout`: the binary entry point now calls into the internal
  library/runtime layout.
- Completed `config-schema`: typed configuration defaults and validation now
  exist without changing runtime behavior.
- Completed `yaml-config-loading`: YAML config files are loaded from
  `RESTEYES_CONFIG` or the XDG config path, partially overlaid onto defaults,
  parsed with string-only human-readable durations, and validated. Follow-up
  cleanup made path resolution more explicit and simplified empty YAML handling.
- Completed `scheduler-break-slots`: an internal deterministic scheduler now
  consumes active-time durations, advances break slots, picks the due break type
  with the largest interval, and holds a pending break until it is finished.
- Generalized break config to one shared `breaks.after_active` duration plus an
  arbitrary `breaks.types` map. Each break type has an integer slot interval,
  duration, messages, and per-type autolock flag.
- The app currently prints `hello world` when no config error is present.
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
- The next scheduler increment should be `scheduler-idle-disable`.
- Step-specific notes belong in `build-notes/$step.md`.
- Update this file whenever a step is completed or the project state changes.
