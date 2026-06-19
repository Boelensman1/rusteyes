# macos-activity

## Summary

- macOS helper IPC now uses protocol version 2.
- Added a `pollActivity` daemon-to-helper message and an `activitySample`
  helper-to-daemon response carrying idle time in milliseconds.
- The Swift helper samples keyboard, mouse, and tablet idle time through
  CoreGraphics `CGEventSource.secondsSinceLastEventType` with the documented
  any-input event type.
- The Rust macOS backend now stays in the daemon loop after handshake, polls
  activity once per second, emits `WallClockElapsed` every poll, and emits
  `ActiveTimeElapsed` when helper idle time is less than or equal to the poll
  interval.
- The Rust activity sample interpretation and regular activity trace output now
  use the same shared activity module as X11.
- Follow-up cleanup moved break timer logic into the shared activity module so
  macOS and X11 use the same countdown behavior.
- Helper protocol or activity sampling failures are logged and shut the daemon
  loop down cleanly.

## Decisions

- Keep activity interpretation in shared Rust code so the macOS backend matches
  the X11 event model and trace output.
- Use a one-second activity poll interval for parity with X11.
- Treat idle as the absence of active-time advancement; no scheduler-level idle
  input was added.
- Keep macOS break overlay, input blocking, lock-after-break, and UI controls
  out of this step.
- Bump the helper protocol version because the daemon now requires helper
  support for activity polling.

## Verification

- `cargo test --all-targets --all-features` passed.
- `make macos-helper-build` passed.
- Helper protocol smoke tests passed for version 2 hello plus activity poll,
  unknown message error handling, incompatible version error handling, and
  shutdown.
- `timeout 3s make run` stayed alive until `timeout` terminated it, confirming
  macOS no longer exits immediately after the helper handshake.
- A bounded `RUST_LOG=trace make run` smoke run prints shared `sampled
  activity` and `queued runtime event` traces on macOS.
- `make check` passed.
- `make check` passed after moving break timer logic into the shared activity
  module.

## Follow-up

- Implement macOS break overlay rendering in `macos-overlay`.
