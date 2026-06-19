# macos-helper-ipc

## Summary

- Added a crate-internal macOS helper backend that starts
  `resteyes-macos-helper`, performs a versioned JSON Lines handshake over
  stdio, and shuts the helper down when the backend is dropped.
- The Rust daemon now uses the helper backend on macOS instead of reporting an
  unsupported platform; Linux/X11 behavior is unchanged.
- Added version 1 daemon-to-helper messages for `hello`, `startBreak`,
  `finishBreak`, `clearBreak`, and `shutdown`.
- Added version 1 helper-to-daemon messages for `ready`, future runtime events,
  `shutdownComplete`, and `error`.
- The Swift helper now reserves stdout for JSON Lines protocol messages,
  validates the initial `hello` message, sends `ready`, accepts no-op break
  commands, and exits after `shutdown`.
- `make run` now depends on the helper artifact on Darwin so local macOS runs
  rebuild the helper first.

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
- Default helper lookup uses
  `helpers/macos-helper/.build/debug/resteyes-macos-helper`, with
  `RESTEYES_MACOS_HELPER` available as an override for tests and future
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

## Follow-up

- Implement macOS keyboard/mouse activity and wall-clock event reporting in
  `macos-activity`.
