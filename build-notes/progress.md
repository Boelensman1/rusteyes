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
  Follow-up cleanup removed scheduler-only inspection helpers from production
  builds and simplified rule ordering.
- Completed `scheduler-disable-state`: the scheduler can be explicitly disabled
  and enabled; disabled state suppresses active-time accumulation, resets
  accumulated active time and pending breaks on disable, and preserves completed
  slot progression.
- Completed internal API cleanup before daemon wiring: crate public API now
  exposes `run` and an application error wrapper, config remains crate-internal,
  and scheduler construction uses a validated `BreakSchedule` instead of raw
  config storage. Follow-up cleanup tightened config type visibility to the
  crate boundary.
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
  inspect scheduler pending state to drive backend cleanup, and made backend
  command handling default to no-op for backends that do not handle commands.
- Completed `x11-activity`: Linux production runs now use a permanent
  crate-internal X11 activity backend backed by XScreenSaver idle time. It emits
  wall-clock ticks and active-time increments into the runtime. This step
  initially used a temporary diagnostic wrapper, which was removed by
  `x11-overlay`.
- Completed `x11-overlay`: Linux/X11 break commands now create unmanaged
  `override_redirect` overlay windows over connected monitor geometries, render
  the first configured break message, keep overlays raised/redrawn during the
  break, suppress active-time polling while the break is visible, count down
  break duration only on idle overlay samples, and clear the overlays when the
  runtime finishes or clears the break.
- Completed `x11-input-blocking`: X11 break overlays now acquire active core
  pointer and keyboard grabs while visible, keep pointer movement unconstrained,
  prevent grabbed input from reaching other X11 clients, and release grabs when
  overlays are cleared.
- Completed `x11-lock-after-break`: config now has a `lock.command` argv list
  defaulting to `loginctl lock-session`, and Linux/X11 production runs start
  and supervise the configured command after a break requests local locking.
  Follow-up fix keeps the overlay visible during lock handoff by finishing the
  break and starting the lock command through one backend command before
  destroying the overlay.
- Completed `x11-ui-improvements`: X11 break overlays now render remaining
  break time and a lock-after-break control, and runtime tracks current-break
  lock state from configured autolock or that control before requesting the
  local lock command after the break finishes. Follow-up cleanup simplified
  lock-control hit testing and break timer state.
- Completed `logging`: the binary now initializes `tracing` output with a
  warning-level default and `RUST_LOG` override support, X11 backend errors use
  tracing events, high-frequency X11 activity and overlay samples are available
  at trace level, and top-level startup errors remain visible on stderr.
- Added `test-break-config`: `test-configs/ten-second-break.yaml` starts a 10
  second test break after 10 seconds of active time for manual testing.
- Completed `manual-break-control`: runtime events can now start configured
  named breaks on demand, manual breaks are marked separately from scheduled
  slot breaks, and manual starts work while local scheduling is disabled.
  Follow-up cleanup simplified scheduler pending state and runtime
  current-break lock state.
- Completed `sync-config-auth`: config now has disabled-by-default sync
  settings with a validated 32-character minimum shared secret for enabled
  sync, redacted shared-secret debug output, and no configured peer ID.
- Cargo is the Rust build system; `make` is the project task runner.
- Nix provides the reproducible development shell and package build.
- Codex project hooks are configured to run Rust formatting after Codex edits.

## Verified

- A sandboxed `timeout 3s make run` reaches the X11 startup path but cannot
  connect to X11 from this environment.
- `make check` passes formatting, Clippy, and tests.
- `make build` passes.
- `nix build` passes.
- `.codex/hooks/rustfmt.sh` runs successfully.
- Manual X11 overlay, input-blocking, overlay UI, and trace-output verification
  is still pending because this environment does not provide usable X server
  access.

## Notes

- Build work should proceed one step at a time.
- The next planned increment is `macos-helper-scaffold`.
- The later build order now brings macOS backend parity before sync protocol,
  then separates sync protocol, LAN discovery, authenticated peer transport,
  synced break/disable behavior, tray UI, and synced lock-after-break behavior.
- Step-specific notes belong in `build-notes/$step.md`.
- Update this file whenever a step is completed or the project state changes.
