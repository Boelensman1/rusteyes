# Progress

## Current State

- Initialized the repository and created the initial Rust binary scaffold.
- Completed `project-rename`: the project now uses RustEyes for human-facing
  text, `rusteyes` for lowercase package/binary/config/service identifiers, and
  `RUSTEYES_*` for environment variables.
- Completed `core-layout`: the binary entry point now calls into the internal
  library/runtime layout.
- Completed `config-schema`: typed configuration defaults and validation now
  exist without changing runtime behavior.
- Completed `yaml-config-loading`: YAML config files are loaded from
  `RUSTEYES_CONFIG` or the XDG config path, partially overlaid onto defaults,
  parsed with string-only human-readable durations, and validated. Follow-up
  cleanup made path resolution more explicit and simplified empty YAML handling.
- Completed `default-config-file`: missing implicit XDG or home config files
  are created on startup from the typed default config, while explicit
  `RUSTEYES_CONFIG` paths remain strict and missing explicit files still fail.
- Completed `macos-login-item`: macOS builds can optionally register or
  unregister RustEyes as the main-app Login Item through
  `startup.open_at_login`, using `smappservice-rs` and leaving startup state
  unmanaged when the config key is omitted or null.
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
  overlays are cleared. Follow-up fix treats transient X11 grab contention as
  a bounded break-start retry and skips the unstarted break without shutting
  down the daemon if the grab remains unavailable.
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
  macOS activity traces are not blocked before reaching the terminal. Later
  follow-up cleanup added an info-level startup event after logging
  initialization so explicit `RUST_LOG` runs show when the daemon starts.
- Completed `service-log-delivery`: the binary logger now writes tracing events
  through a dup of the inherited stderr (fd 2) instead of reopening
  `/dev/stderr`, so logs reach whatever a service manager attaches — the
  journald socket under systemd (previously dropped with `ENXIO`) and the
  launchd `StandardErrorPath` file (previously overwritten at offset 0) — while
  still bypassing the global Rust stderr lock. `logLevel`/`RUST_LOG` now has a
  visible effect under both service managers.
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
  builds `rusteyes-macos-helper`, with an explicit Make artifact target and
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
  advertises a peer-specific RustEyes sync service, authenticates TXT metadata
  with the configured shared secret, and converts resolved authenticated
  services into discovered peer records. Follow-up verification support added a
  temporary `RUSTEYES_DISCOVERY_SMOKE=1` path to run discovery without the
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
  Later cleanup moved transport framing and sequence state behind a private
  session helper, renamed nonblocking inbound polling to `try_recv_event`,
  added a facade event drain helper, made connection authentication/removal
  outcomes explicit, and centralized failed endpoint closure in the worker.
  Later cleanup removed transport worker timeout polling with `message-io` wake
  signals and replaced discovery timeout polling with a selected mDNS/shutdown
  event path. Later cleanup moved remaining transport command, reply, and
  facade event channels from `std::sync::mpsc` to `flume`, including bounded
  one-shot reply channels.
- Completed `backend-actor-runtime`: platform backends now run as actor
  threads that own X11 or macOS helper state, receive `BackendCommand` values
  over flume, and send `RuntimeEvent` values over flume. X11 and macOS sampling
  loops now use `flume::Selector::wait_timeout` so backend commands can
  interrupt the next sample delay. Runtime now selects over backend events and
  sync transport events through one internal input path, while sync domain
  behavior remains deferred to `active-time-sync`.
- Completed `active-time-sync`: local backend-originated active-time events now
  broadcast authenticated `ActiveTimeElapsed` sync events, inbound authenticated
  active-time events advance the same scheduler path without rebroadcasting, and
  existing scheduler disabled/pending states suppress remote active-time
  increments.
- Completed `break-disable-sync`: local scheduled/manual break starts and local
  disable controls now broadcast authenticated sync events, inbound
  authenticated break-start and disable/enable events apply locally without
  rebroadcasting, timed local disables re-enable from wall-clock elapsed time
  without a separate enable event, and synced lock-after-current-break remains
  deferred.
  Follow-up cleanup collapsed duplicated local/synced runtime helper paths,
  removed thin backend actor channel wrappers, and made sync runtime tests drive
  ordered inputs without sleep-delayed event sources. Later cleanup removed the
  stale test-only local runtime enable event and the resulting broad dead-code
  allowances on shared backend events.
- Completed `notification-tray-ui`: Linux and macOS production runs now start a
  main-thread tray/menu-bar UI with controls to start configured break types,
  disable for configured presets, disable until restart, and quit. Runtime now
  sends one pre-break notification when active time enters the fixed
  `min(30s, breaks.after_active / 2)` notice window for the next scheduled
  break, and UI-originated controls reuse the existing local runtime paths so
  manual breaks and disable actions can sync to authenticated peers. Follow-up
  fix configures macOS as an accessory app and hides Dock visibility so RustEyes
  is menu-bar only. Later follow-up refinement shows accumulated scheduler
  active time in the tray/menu-bar dropdown and resets it with scheduler state.
  Later follow-up refinement orders manual break controls by scheduled cadence
  from shortest to longest. Later follow-up fix adds a local
  `target/macos/RustEyes.app` bundle with bundle id `dev.rusteyes.RustEyes`,
  configures `notify-rust` with that bundle id before runtime startup, and
  launches the app bundle from `make run` on macOS so notifications no longer
  trigger the `use_default` application lookup.
  Later follow-up refinement adds a logo-derived Linux/macOS tray icon and a
  macOS app bundle icon copied into `RustEyes.app`.
- Completed `synced-lock-after-break`: local lock-after-current-break requests
  now broadcast authenticated sync events, inbound authenticated lock-after
  requests mark the active local break for locking without rebroadcasting, and
  X11/macOS active overlays update their lock-control state through private
  backend paths before the existing platform-local lock hook runs at break
  finish.
- Completed `idle-reset-behavior`: config now has optional
  `breaks.reset_after_idle`, defaulting to 5 minutes and disabled with `null`;
  normal activity polling emits idle-duration runtime events; runtime resets
  accumulated scheduler active time after enough combined idle time, clears
  pre-break notification state, updates the active-time tray row, and treats
  authenticated remote active-time events as combined activity without changing
  the sync protocol. Follow-up cleanup removed the now-unneeded
  platform-specific dead-code allowance from the shared break timer remaining
  accessor.
- Completed `peer-config-compatibility`: sync protocol version 2 peer hellos
  now carry a keyed compatibility fingerprint derived from synced break
  behavior settings, excluding lock command and raw sync secrets; transport
  rejects authenticated peers with mismatched fingerprints before domain sync
  events can flow, and runtime sends one desktop notification per rejected peer
  ID through the existing UI notification path.
- Completed `rust-1-96-toolchain`: Cargo now requires Rust 1.96, the Nix
  development shell and package build use the exact Rust 1.96.0 toolchain from
  `oxalica/rust-overlay`, and Rust 1.96 Clippy compatibility fixes have been
  applied.
- Completed `macos-user-notifications-api`: macOS builds now enable
  `notify-rust`'s `UNUserNotificationCenter` backend, request notification
  authorization during UI startup, keep desktop notifications at normal
  priority, and ad-hoc sign the generated local app bundle before
  LaunchServices registration.
- Completed `pre-break-notification-countdown`: pre-break desktop
  notifications now update in place at countdown boundaries, preserve the
  half-interval notice lead for short schedules, and are explicitly cleared
  when the break starts.
- Completed `idle-activity-grace`: normal activity samples now use an internal
  10 second idle threshold so slow ongoing input still counts as active, while
  local and synced active-time signals share one wall-clock budget so synced
  peers do not multiply active-time accumulation.
- Completed `linux-packaging`: the flake now exposes `nix run` app output,
  NixOS and Home Manager modules, a GTK-wrapped Linux package, generated
  service YAML through `RUSTEYES_CONFIG`, secret-safe sync configuration through
  `RUSTEYES_SYNC_SHARED_SECRET_FILE`, and a graphical-session systemd user
  service. A follow-up fix wraps the Linux binary's `LD_LIBRARY_PATH` with
  `libappindicator-gtk3` so the runtime `dlopen` of the appindicator tray
  library succeeds instead of panicking at startup.
- Completed `macos-packaging`: Darwin flake defaults now build a
  `RustEyes.app` bundle containing the Rust binary, Swift helper, icon, plist,
  and a `bin/rusteyes` wrapper; the raw Rust package remains available as
  `packages.<system>.rusteyes`; Darwin Home Manager installs the app and writes
  generated config to `~/.config/rusteyes/config.yaml` without creating a
  LaunchAgent.
- Completed `macos-launchagent`: the Darwin Home Manager module gained an opt-in
  `launchAgent.enable` option that manages a `launchd.agents.rusteyes`
  LaunchAgent running the app at login with `common.serviceEnvironment`
  injected, unlocking `configFile`, `syncSharedSecretFile`, `logLevel`, and
  `extraEnvironment` on Darwin (this is how the sync shared secret reaches the
  app on macOS). It asserts that `settings.startup.open_at_login` is not also
  set to avoid launching the app twice, and the default-path config write is now
  skipped when an external `configFile` is used.
- Completed `macos-tray-template-icon`: the macOS menu-bar tray icon is now an
  NSImage template image via `TrayIconBuilder::with_icon_as_template(true)`
  (gated to `cfg(target_os = "macos")`), so the system tints the existing icon's
  alpha silhouette to match the menu bar (white on dark, black on light) instead
  of showing the full-colour gear. The embedded `rusteyes-tray.rgba` asset is
  reused unchanged (templates ignore RGB), and the Linux system tray keeps the
  full-colour icon.
- Completed `break-finished-beep`: finishing a break now plays a short beep to
  signal that work can resume. The X11 backend rings the X server bell via
  `xproto::ConnectionExt::bell` (best-effort, default volume, only when a break
  was active, before any lock handoff), and the macOS helper calls
  `NSSound.beep()` on the main thread in `handleFinishBreak`. No config toggle
  was added. Audible verification on a real X session and on macOS is pending.
- Completed `random-break-message`: a break type with several configured
  messages now shows a randomly chosen one each break instead of always the
  first. Selection is centralized on the Rust side via
  `ScheduledBreak::random_message()` (backed by `getrandom`), the macOS wire
  protocol carries a single chosen `message` to the Swift helper, and the
  Linux-only `RuntimeEvent::BreakStartFailed` variant gained a
  non-Linux `allow(dead_code)` so `-D warnings` builds cleanly on all hosts.
- Completed `overlay-stuck-recovery`: defensive checks so the macOS break
  overlay (and its input-blocking event tap) can never outlive a break and
  force a reboot. `runtime::finish_break` now clears the overlay whenever a break
  was active (decoupled from the scheduler's return value, with a duplicate-event
  guard); the Swift helper gained two main-queue watchdogs (zero-timer 5s and
  heartbeat 10s, overridable via `RUSTEYES_HELPER_WATCHDOG_MS`) that self-clear
  the overlay if dismissal never arrives; and `HelperSession` reads are now
  bounded by a 5s `recv_timeout` via a reader thread so a wedged helper is killed
  (the OS then removes the tap) instead of blocking the backend forever. Manual
  macOS verification of the watchdogs and the wedge path is pending.
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
- `cargo test --all-targets --all-features sync_transport` passes after the
  transport session and connection outcome API cleanup.
- `make check` passes after the transport session and connection outcome API
  cleanup.
- `make check` passes after adding the backend actor runtime.
- `cargo check --all-targets --all-features` passes after adding active-time
  sync.
- `cargo test --all-targets --all-features runtime` passes after adding
  active-time sync.
- `make check` passes after adding active-time sync.
- `cargo test --all-targets --all-features runtime` passes after adding
  break/disable sync behavior.
- `make check` passes after adding break/disable sync behavior.
- `make check` passes after the break/disable sync runtime cleanup.
- `make check` passes after adding notification tray UI behavior.
- `make check` passes after adding the tray/menu-bar active-time status row.
- `make -B macos-app-build` passes with approved SwiftPM cache and
  LaunchServices registration access after adding the macOS app bundle.
- `plutil -lint target/macos/RustEyes.app/Contents/Info.plist` passes.
- A bounded `timeout 3s make run` on macOS reaches the bundled app path and
  exits at the existing missing Accessibility/Input Monitoring preflight in
  this environment.
- `make check` passes after adding the macOS app bundle notification identity
  fix.
- `cargo test --all-targets --all-features runtime` passes after adding synced
  lock-after-break behavior.
- `make check` passes after adding synced lock-after-break behavior.
- `make check` passes after adding idle reset behavior.
- `cargo check --all-targets --all-features` passes after adding sync peer
  compatibility fingerprints.
- `cargo test --all-targets --all-features sync_protocol` passes after adding
  sync peer compatibility fingerprints.
- `cargo test --all-targets --all-features sync_transport` passes after adding
  sync peer compatibility fingerprints.
- `cargo test --all-targets --all-features runtime` passes after adding sync
  peer rejection notifications.
- `make check` passes after adding sync peer compatibility rejection and
  notifications.
- A bounded `timeout 3s make run` on macOS stays alive until terminated by
  `timeout` and no longer emits helper stderr during startup after AppKit setup
  was made lazy.
- `nix build` passes.
- `nix develop --command rustc --version` reports Rust 1.96.0 after the exact
  Nix toolchain pin.
- `nix develop --command cargo --version` reports Cargo 1.96.0 after the exact
  Nix toolchain pin.
- `nix develop --command make check` passes after switching to Rust 1.96.0.
- `nix build` passes after switching to Rust 1.96.0.
- `.codex/hooks/rustfmt.sh` runs successfully.
- `make check` passes after moving macOS notifications to the
  `UNUserNotificationCenter` backend.
- `make -B macos-app-build` passes after adding explicit ad-hoc signing for the
  generated local macOS app bundle.
- `plutil -lint target/macos/RustEyes.app/Contents/Info.plist` passes after
  moving macOS notifications to the `UNUserNotificationCenter` backend.
- `codesign -dv target/macos/RustEyes.app` reports ad-hoc signature identity
  `dev.rusteyes.RustEyes` after the signing step.
- `codesign -d --entitlements - target/macos/RustEyes.app` emits no entitlement
  dictionary after keeping macOS notifications at normal priority.
- `timeout 3s make run` launches the macOS app bundle until the timeout sends
  SIGTERM, avoiding the earlier AMFI `Killed: 9` restricted-entitlement failure.
- `cargo test pre_break_notification --lib` passes after adding countdown
  update and clear behavior.
- `make check` passes after adding pre-break notification countdown updates.
- `make check` passes after adding idle activity grace and capped synced
  active-time accumulation.
- `make check` passes after removing stale dead-code allowances.
- `make check` passes after the project rename to RustEyes.
- `make -B macos-app-build` passes after adding pre-break notification
  countdown updates.
- `nix develop --command cargo test --lib config::tests` passes after adding
  default config file creation.
- `make check` passes after adding default config file creation.
- `make check` passes after adding macOS Login Item integration.
- `make -B macos-app-build` passes after adding macOS Login Item integration.
- `plutil -lint target/macos/RustEyes.app/Contents/Info.plist` passes after
  adding macOS Login Item integration.
- `codesign -dv target/macos/RustEyes.app` reports ad-hoc signature identity
  `dev.rusteyes.RustEyes` after adding macOS Login Item integration.
- `make test` passes after adding Linux packaging config-secret support.
- `nix flake show --json` exposes app, package, NixOS module, and Home Manager
  module outputs after adding Linux packaging.
- NixOS module eval confirms the generated user service has
  `graphical-session.target` ordering, generated `RUSTEYES_CONFIG`,
  `RUSTEYES_SYNC_SHARED_SECRET_FILE`, and configured `RUST_LOG`.
- Home Manager module eval confirms package installation and the generated
  graphical-session systemd user service environment.
- `make check` passes after adding Linux packaging.
- `nix build` passes after adding Linux packaging.
- Attempted `nix build .#packages.x86_64-linux.default --no-link` from this
  `aarch64-darwin` host; it failed with a platform mismatch because no
  `x86_64-linux` builder was available.
- `nix build .#rusteyes` on an x86_64-linux host builds the package, and the
  wrapped `bin/rusteyes` embeds
  `--prefix LD_LIBRARY_PATH : .../libappindicator-gtk3-.../lib`, confirming the
  appindicator tray library is reachable at runtime after the `LD_LIBRARY_PATH`
  wrapper fix.
- Darwin package eval confirms `default`, `macos-app`, `macos-helper`, and
  `rusteyes` package outputs after adding macOS packaging.
- `nix build .#packages.aarch64-darwin.default --no-link --print-out-paths`
  builds the Nix `RustEyes.app` bundle after adding macOS packaging.
- `plutil -lint` passes for the Nix-built
  `RustEyes.app/Contents/Info.plist`.
- `codesign -dv` reports ad-hoc signatures for the Nix-built app executable
  and bundled helper. The Nix build signs Mach-O files with `darwin.sigtool`;
  it does not sign the `.app` directory itself.
- Home Manager module eval confirms Darwin installs RustEyes, writes generated
  `xdg.configFile."rusteyes/config.yaml"`, and does not create a systemd user
  service.
- Home Manager module eval confirms Darwin rejects unsupported
  `configFile` and `syncSharedSecretFile` settings.
- Linux Home Manager module eval still confirms package installation,
  graphical-session systemd user service wiring, generated `RUSTEYES_CONFIG`,
  `RUSTEYES_SYNC_SHARED_SECRET_FILE`, and configured `RUST_LOG` after adding
  macOS packaging.
- `nix flake show --json` exposes the macOS packaging outputs.
- `make check` passes after adding macOS packaging.
- On unsupported targets, `make run` prints
  `rusteyes: no backend is available for <platform> yet` and exits non-zero.
- Manual X11 overlay, input-blocking, overlay UI, and trace-output verification
  is still pending because this environment does not provide usable X server
  access.
- Manual Linux tray and notification verification is pending.
- Manual macOS notification verification is pending after granting
  Accessibility/Input Monitoring to RustEyes.
- Manual macOS Focus/Do Not Disturb notification verification is pending after
  granting notification permission to RustEyes.
- Manual macOS input-blocking verification with Accessibility/Input Monitoring
  permissions granted is still pending.
- Manual launch verification from a Nix-installed or Home Manager-copied
  `RustEyes.app` is pending after granting the required macOS permissions.

## Notes

- Build work should proceed one step at a time.
- No next MVP increment is currently selected.
- The later build order now brings macOS backend parity before sync protocol,
  then separates sync protocol, LAN discovery, authenticated peer transport,
  active-time sync, synced break/disable behavior, tray UI, and synced
  lock-after-break behavior.
- Step-specific notes belong in `build-notes/$step.md`.
- Update this file whenever a step is completed or the project state changes.
