# macos-overlay

## Summary

- macOS break commands now show helper-owned black AppKit overlay windows on
  every `NSScreen`.
- The helper renders the first configured break message centered in white, with
  `Take a break` as a protocol fallback if no usable message is present.
- The Rust macOS backend tracks the active break, suppresses active-time events
  while the overlay is visible, polls helper activity every 500 ms for
  locked-session and overlay-control state, computes remaining break time from a
  monotonic deadline, and queues `BreakFinished` when the break duration has
  elapsed.
- `finishBreak`, `clearBreak`, EOF, and `shutdown` clear helper overlay
  windows.
- AppKit is initialized lazily on the first overlay command so normal daemon
  startup does not emit AppKit diagnostics.
- Follow-up fix made the 500 ms overlay poll a target UI/state cadence rather
  than the countdown source of truth, preventing early or delayed backend
  wakeups from changing visible break length.
- Follow-up fix: overlays now cover every display, not only the primary one.
  `NSWindow(contentRect:...screen:)` interprets `contentRect` relative to the
  given screen's origin, so passing the global `screen.frame` offset each
  non-primary window by its screen origin a second time, leaving it off-display.
  Windows are now created at a zero origin and then pinned with
  `setFrame(screen.frame, ...)` in global coordinates.
- The same fix corrected the Quartz-to-AppKit point flip used for the
  lock-after-break control: it now flips about the primary display's height
  instead of the union of all screen frames, which was wrong when displays have
  different heights or are stacked vertically.

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
- `make check` passed after switching visible-break countdowns to monotonic
  deadlines.
- A helper protocol smoke test for `hello`, `startBreak`, `clearBreak`, and
  `shutdown` returned only `ready` and `shutdownComplete` on stdout.
- A bounded `timeout 3s make run` stayed alive until terminated by `timeout`
  and no longer emitted helper stderr during startup.
- The helper smoke test exercised overlay creation and cleanup on macOS; visual
  inspection is not available from the command output.
- The multi-display fix was made from a Linux container: `make check` passed,
  but the Swift helper was not built and the fix still needs checking on a Mac
  with a secondary display (overlay coverage and the lock control on each
  screen).

## Follow-up

- Implement `macos-input-blocking`.
- If AppKit stderr diagnostics during overlay creation prove noisy in normal
  use, decide whether to filter known system diagnostics in the helper stderr
  mirror.
