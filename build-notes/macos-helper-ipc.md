# macos-helper-ipc

## Summary

- Added a crate-internal macOS helper backend that starts
  `rusteyes-macos-helper`, performs a versioned JSON Lines handshake over
  stdio, and shuts the helper down when the backend is dropped.
- The Rust daemon now uses the helper backend on macOS instead of reporting an
  unsupported platform; Linux/X11 behavior is unchanged.
- Added version 1 daemon-to-helper messages for `hello`, `startBreak`,
  `finishBreak`, `clearBreak`, and `shutdown`.
- Added version 1 helper-to-daemon messages for `ready`, `shutdownComplete`,
  and `error`; future activity/control events remain for `macos-activity`.
- The Swift helper now reserves stdout for JSON Lines protocol messages,
  validates the initial `hello` message, sends `ready`, accepts no-op break
  commands, exits after `shutdown`, and prints a human-facing stderr message
  when launched without the daemon's initial `hello`.
- `make run` now depends on the helper artifact on Darwin so local macOS runs
  rebuild the helper first.
- Follow-up cleanup validates the break schedule before starting any platform
  backend, always attempts to reap the helper during shutdown, trims unused
  future helper events from the Rust IPC model, and removes unused Swift scaffold
  imports/context.
- Follow-up cleanup now waits for explicit helper `commandComplete`
  acknowledgements for break/control commands before updating Rust backend
  break state, and shares one helper shutdown wait/kill/reap path for normal
  drop and startup failures.

## Decisions

- Use stdio JSON Lines for the local daemon/helper protocol because it keeps
  process supervision simple and gives structured escaping without custom
  framing.
- Add `serde_json` only for macOS targets.
- Treat `backend`, `config`, `scheduler`, and the daemon runtime as portable
  core modules; keep cfg gates only around concrete platform backends and
  platform-specific entry points.
- Keep real macOS activity, overlay, input blocking, and lock behavior out of
  this increment.
- Keep helper stdout protocol-only; human diagnostics belong on stderr.
- Treat terminal launches, empty stdin, invalid first input, and non-`hello`
  first messages as direct helper invocation. These cases print
  `rusteyes-macos-helper is an internal RustEyes helper. Start RustEyes with
  the main rusteyes binary; do not run this helper directly.` to stderr and
  exit with status 2.
- Keep incompatible `hello` versions on the JSON protocol path so the daemon
  can report version mismatches as structured helper errors.
- Default helper lookup uses
  `helpers/macos-helper/.build/debug/rusteyes-macos-helper`, with
  `RUSTEYES_MACOS_HELPER` available as an override for tests and future
  packaging.

## Verification

- `make macos-helper-build` initially failed in the sandbox because SwiftPM
  could not write user Swift/Clang caches.
- `make macos-helper-build` passed with approved cache access.
- `make check` passed.
- `make run` initially failed in the sandbox because the Nix daemon socket was
  unavailable.
- `make run` passed with approved Nix daemon access and exited cleanly through
  the macOS helper handshake path.
- `make check` passed after adding the direct-invocation guard.
- `make macos-helper-build` reported the helper target up to date; forced
  `make -B macos-helper-build` initially failed in the sandbox because SwiftPM
  and Clang could not write user caches, then passed with approved cache
  access.
- Direct helper terminal launch, empty stdin, invalid first input, and a
  non-`hello` first message each printed the direct-invocation message and
  exited with status 2.
- A valid `hello` followed by `shutdown` returned `ready` and
  `shutdownComplete` JSON messages.
- An incompatible `hello` version returned a JSON `error` message.
- `make run` still starts the helper and exits cleanly through the macOS helper
  handshake path.
- `make check` passed after the follow-up cleanup.
- `make macos-helper-build` rebuilt the simplified Swift helper successfully.
- `make run` initially failed in the sandbox because the Nix daemon socket was
  unavailable, then passed with approved Nix daemon access.
- `make check` passed after adding command acknowledgements and consolidating
  helper shutdown cleanup.
- `make macos-helper-build` passed after adding command acknowledgements.

## Follow-up

- Implement macOS keyboard/mouse activity and wall-clock event reporting in
  `macos-activity`.
