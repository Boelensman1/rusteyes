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
  and originally exited cleanly through a no-op backend.
- Completed `backend-trait`: the runtime now uses a crate-internal backend
  boundary with runtime events for activity/control input and backend commands
  for break start, break clear, and local lock requests. Follow-up cleanup made
  scheduler transitions return affected pending breaks so runtime does not
  inspect scheduler pending state to drive backend cleanup, made backend
  command handling default to no-op for backends that do not handle commands,
  and removed the unused test-only no-op backend.
- Completed `x11-activity`: Linux production runs now use a permanent
  crate-internal X11 activity backend backed by XScreenSaver idle time. It emits
  wall-clock ticks and active-time increments into the runtime. This step
  initially used a temporary diagnostic wrapper, which was removed by
  `x11-overlay`.
- Follow-up fix: unsupported non-Linux builds now fail at startup with a clear
  missing-backend message instead of silently using the no-op backend path.
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
- Completed `x11-lock-after-break`: config now has an optional `lock.command`
  argv override, and Linux/X11 production runs start and supervise
  `loginctl lock-session` by default, or the configured command after a break
  requests local locking. Follow-up fix keeps the overlay visible during lock
  handoff by finishing the break and starting the lock command through one
  backend command before destroying the overlay. Later cleanup moved default
  lock behavior to each platform backend.
- Completed `x11-ui-improvements`: X11 break overlays now render remaining
  break time and a lock-after-break control, and runtime tracks current-break
  lock state from configured autolock or that control before requesting the
  local lock command after the break finishes. Follow-up cleanup simplified
  lock-control hit testing and break timer state.
- Completed `logging`: the binary now initializes `tracing` output with a
  warning-level default and `RUST_LOG` override support, X11 backend errors use
  tracing events, high-frequency regular activity samples are logged through a
  backend-agnostic activity module, X11 overlay samples remain available at
  trace level, and top-level startup errors remain visible on stderr. Follow-up
  cleanup switched tracing writes away from the global Rust stderr writer so
  macOS activity traces are not blocked before reaching the terminal.
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
- Completed `macos-helper-scaffold`: a standalone SwiftPM macOS helper package
  builds `resteyes-macos-helper`, with an explicit Make artifact target and
  `macos-helper-build` alias that build on Darwin and skip successfully
  elsewhere.
- Completed `macos-helper-ipc`: macOS production runs now start the Swift
  helper, complete a versioned stdio JSON Lines handshake, define command and
  shutdown framing for later macOS backend work, and shut the helper down
  cleanly.
- Completed `macos-activity`: macOS production runs now keep the helper-backed
  daemon loop alive after handshake, poll CoreGraphics any-input idle time once
  per second through protocol version 2, and emit wall-clock and active-time
  runtime events with the same shared activity interpretation and regular
  activity trace output as X11.
- Completed `macos-overlay`: macOS break commands now create helper-owned black
  AppKit overlay windows on every `NSScreen`, render the first configured break
  message, suppress active-time events while visible, advance break duration
  only on idle helper samples through the shared break timer, and clear
  overlays on finish, clear, shutdown, or helper EOF.
- Completed `macos-input-blocking`: macOS break startup now creates a
  helper-owned Quartz session event tap before showing overlay windows, drops
  normal keyboard and pointer input while the overlay is visible, disables the
  tap on overlay cleanup, and reports a structured helper error instead of
  showing an unblocked overlay when macOS refuses event tap creation.
- Completed `macos-permission-preflight`: macOS helper protocol version 4 now
  has a startup permission preflight that explicitly requests Accessibility
  trust, checks Input Monitoring trust, fails startup before scheduling when
  either permission is missing, acknowledges break/control commands before Rust
  updates backend break state, and keeps the break-time event tap failure path
  as a defense against permissions revoked later.
- Completed `macos-lock-after-break`: macOS production runs now honor
  lock-after-break intent by clearing the helper overlay/input tap before
  either calling the helper-native `SACLockScreenImmediate` default lock path or
  running an explicitly configured no-shell lock command from Rust.
- Completed `macos-ui-improvements`: macOS break overlays now render remaining
  break time and a lock-after-break control through helper protocol version 6,
  and helper click requests are reported back to the runtime before a break can
  finish and request local locking. Follow-up fix explicitly serializes the
  helper command lock flag as `lockAfter` so `updateBreak` and `finishBreak`
  match the Swift protocol parser. Later follow-up fix forces the break overlay
  cursor to the arrow cursor so an insertion cursor from another app does not
  remain visible during an input-blocking break. Later cleanup replaced Swift
  dictionary protocol parsing with typed `Codable` messages, made Rust validate
  helper shutdown completion, aligned macOS overlay event ordering with X11,
  and clarified helper hit-test point conversion.
- Completed `sync-protocol`: crate-internal version 1 sync messages now carry
  transient peer identity, sequence numbers, active-time increments, named
  break starts, disable/enable controls, and lock-after-current-break requests,
  authenticated with HMAC-SHA256 over canonical JSON payloads.
- Completed `lan-discovery`: crate-internal mDNS/DNS-SD discovery now
  advertises a peer-specific Resteyes sync service, authenticates TXT metadata
  with the configured shared secret, and converts resolved authenticated
  services into discovered peer records. Follow-up verification support added a
  temporary `RESTEYES_DISCOVERY_SMOKE=1` path to run discovery without the
  platform backend and log authenticated peers found on the LAN; this temporary
  path was removed when authenticated peer transport started discovery from
  normal runtime code.
- Completed `authenticated-peer-transport`: sync-enabled runtime startup now
  creates a transient peer ID, starts a framed TCP transport listener on an
  OS-assigned port, advertises that port through authenticated LAN discovery,
  connects to discovered peers, authenticates each connection with a HMAC-framed
  transport-control `PeerHello`, rejects unauthenticated/self endpoints, and
  collapses duplicate peer connections deterministically. Follow-up transport
  cleanup split control frames from domain sync events, wrapped `message-io`
  behind a private adapter, and added crate-internal broadcast, directed send,
  and authenticated inbound domain-event receiver APIs before runtime sync
  behavior is added. Later follow-up cleanup split the transport facade,
  worker commands, worker loop, and connection tracking into smaller modules
  and added per-peer inbound event sequence replay protection. Later API
  cleanup made disabled sync an inert transport value, moved inbound polling
  onto the transport facade, named the transport IO listener binding, split
  connection success/failure events, and moved sender/replay acceptance behind
  the connection tracker. Later cleanup unified peer/auth/domain output behind
  one `SyncTransportEvent` facade stream, hid wire sequence numbers from
  runtime-facing domain events, and simplified connection binding results.
- Cargo is the Rust build system; `make` is the project task runner.
- Nix provides the reproducible development shell and package build.
- Codex project hooks are configured to run Rust formatting after Codex edits.

## Verified

- A sandboxed `timeout 3s make run` reaches the X11 startup path but cannot
  connect to X11 from this environment.
- `make check` passes formatting, Clippy, and tests.
- `make build` passes.
- `make macos-helper-build` passes on macOS with SwiftPM available.
- `make run` on macOS starts the helper, completes the IPC handshake, and exits
  cleanly in older helper-IPC verification before activity polling existed.
- A bounded `timeout 3s make run` on macOS now stays alive until terminated by
  `timeout`, confirming the helper-backed activity polling loop is running.
- A bounded `RUST_LOG=trace make run` on macOS prints shared `sampled activity`
  and `queued runtime event` traces.
- macOS helper protocol smoke tests returned `ready`, `activitySample`, and
  `shutdownComplete` for a valid version 2 session, returned a structured error
  for an unknown message, and returned a structured error for an incompatible
  version.
- A macOS helper overlay smoke test returned only `ready` and
  `shutdownComplete` on stdout for `hello`, `startBreak`, `clearBreak`, and
  `shutdown`; AppKit emitted system diagnostics to stderr when overlay creation
  was exercised.
- A macOS helper input-blocking smoke test returned `ready`, a structured
  event-tap permission error for `startBreak`, and `shutdownComplete`,
  confirming the helper does not show an unblocked overlay when required macOS
  permissions are unavailable.
- macOS helper protocol version 4 tests cover permission preflight framing,
  `preflightResult` decoding, command acknowledgements, command error handling,
  and startup error text for missing Accessibility, missing Input Monitoring,
  and both permissions missing.
- A macOS helper protocol version 3 smoke test returned `ready` and
  `shutdownComplete` for `hello` followed by `shutdown`; current Rust unit
  tests cover protocol version 4 framing.
- `make macos-helper-build` passes after adding the permission preflight.
- `make macos-helper-build` passes after adding protocol version 4 command
  acknowledgements.
- A temporary Swift smoke test loaded and invoked `SACLockScreenImmediate` from
  login.framework successfully.
- `make test` passes after adding macOS lock-after-break behavior.
- `make check` passes after adding macOS lock-after-break behavior.
- `make macos-helper-build` passes after adding macOS lock-after-break
  behavior.
- `make test` passes after adding macOS overlay UI protocol and backend state.
- `make macos-helper-build` passes after adding macOS overlay UI controls.
- `make check` passes after adding macOS overlay UI controls.
- `make test` passes after fixing macOS helper `lockAfter` command field
  serialization.
- `make check` passes after fixing macOS helper `lockAfter` command field
  serialization.
- `make macos-helper-build` passes after fixing the macOS overlay cursor.
- `make check` passes after fixing the macOS overlay cursor.
- `make macos-helper-build` passes after the typed macOS helper protocol
  cleanup.
- `make check` passes after the macOS helper shutdown/event-order cleanup.
- `make check` passes after adding authenticated sync protocol framing.
- `make check` passes after adding mDNS/DNS-SD LAN discovery.
- `make check` passes after adding authenticated peer transport.
- `make check` passes after splitting sync transport control/domain frames and
  adding the send/broadcast/inbound receiver transport API.
- `cargo test --all-targets --all-features sync_transport` passes after
  splitting sync transport internals and adding replay protection.
- `make check` passes after splitting sync transport internals and adding
  replay protection.
- `cargo test --all-targets --all-features sync_transport` passes after
  unifying the sync transport event facade.
- `make check` passes after unifying the sync transport event facade.
- A bounded `timeout 3s make run` on macOS stays alive until terminated by
  `timeout` and no longer emits helper stderr during startup after AppKit setup
  was made lazy.
- `nix build` passes.
- `.codex/hooks/rustfmt.sh` runs successfully.
- On unsupported targets, `make run` prints
  `resteyes: no backend is available for <platform> yet` and exits non-zero.
- Manual X11 overlay, input-blocking, overlay UI, and trace-output verification
  is still pending because this environment does not provide usable X server
  access.
- Manual macOS input-blocking verification with Accessibility/Input Monitoring
  permissions granted is still pending.

## Notes

- Build work should proceed one step at a time.
- The next planned increment is `active-time-sync`.
- The later build order now brings macOS backend parity before sync protocol,
  then separates sync protocol, LAN discovery, authenticated peer transport,
  active-time sync, synced break/disable behavior, tray UI, and synced
  lock-after-break behavior.
- Step-specific notes belong in `build-notes/$step.md`.
- Update this file whenever a step is completed or the project state changes.
