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
- Completed `scheduler-disable-state`: the scheduler can be explicitly disabled
  and enabled; disabled state suppresses active-time accumulation, resets
  accumulated active time and pending breaks on disable, and preserves completed
  slot progression.
- Completed internal API cleanup before daemon wiring: crate public API now
  exposes `run` and an application error wrapper, config remains crate-internal,
  and scheduler construction uses a validated `BreakSchedule` instead of raw
  config storage.
- Generalized break config to one shared `breaks.after_active` duration plus an
  arbitrary `breaks.types` map. Each break type has an integer slot interval,
  duration, messages, and per-type autolock flag.
- Completed `daemon-runtime-noop`: the runtime now loads config, initializes a
  scheduler from the validated break schedule, runs a private daemon event loop,
  and exits cleanly through a no-op backend.
- Completed `backend-trait`: the no-op runtime now uses a crate-internal backend
  boundary with runtime events for activity/control input and backend commands
  for break start, break clear, and local lock requests. Follow-up cleanup made
  scheduler transitions return affected pending breaks so runtime does not
  inspect scheduler pending state to drive backend cleanup.
- Cargo is the Rust build system; `make` is the project task runner.
- Nix provides the reproducible development shell and package build.
- Codex project hooks are configured to run Rust formatting after Codex edits.

## Verified

- `make run` starts the no-op daemon and exits successfully.
- `make check` passes formatting, Clippy, and tests.
- `make build` passes.
- `nix build` passes.
- `.codex/hooks/rustfmt.sh` runs successfully.

## Notes

- Build work should proceed one step at a time.
- The next increment should be `x11-activity`.
- Step-specific notes belong in `build-notes/$step.md`.
- Update this file whenever a step is completed or the project state changes.
