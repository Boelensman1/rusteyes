# macos-overlay

## Summary

- macOS break commands now show helper-owned black AppKit overlay windows on
  every `NSScreen`.
- The helper renders the first configured break message centered in white, with
  `Take a break` as a protocol fallback if no usable message is present.
- The Rust macOS backend tracks the active break, suppresses active-time events
  while the overlay is visible, polls helper activity every 500 ms for
  locked-session and overlay-control state, advances break time on each overlay
  tick, and queues `BreakFinished` when the break duration has elapsed.
- `finishBreak`, `clearBreak`, EOF, and `shutdown` clear helper overlay
  windows.
- AppKit is initialized lazily on the first overlay command so normal daemon
  startup does not emit AppKit diagnostics.

## Decisions

- Kept helper protocol version 2 and the existing break command wire shapes.
- Used AppKit directly in the existing Swift helper instead of adding Rust or
  Swift package dependencies.
- Kept input blocking, lock-after-break, remaining-time UI, and overlay controls
  out of this step.
- Helper stdout remains JSON-protocol-only; AppKit can still emit system
  diagnostics to stderr when the overlay path is exercised.

## Verification

- `make macos-helper-build` passed.
- `make check` passed.
- A helper protocol smoke test for `hello`, `startBreak`, `clearBreak`, and
  `shutdown` returned only `ready` and `shutdownComplete` on stdout.
- A bounded `timeout 3s make run` stayed alive until terminated by `timeout`
  and no longer emitted helper stderr during startup.
- The helper smoke test exercised overlay creation and cleanup on macOS; visual
  inspection is not available from the command output.

## Follow-up

- Implement `macos-input-blocking`.
- If AppKit stderr diagnostics during overlay creation prove noisy in normal
  use, decide whether to filter known system diagnostics in the helper stderr
  mirror.
