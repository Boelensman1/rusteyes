# x11-activity

## Goal

- Add permanent X11 keyboard/mouse activity polling for the daemon runtime.

## Changes

- Added Linux-only `x11rb` dependency with the `screensaver` feature.
- Added a private X11 activity backend that connects to X11, verifies the
  XScreenSaver extension, polls `ms_since_user_input`, and converts samples into
  runtime events.
- Linux production `resteyes::run()` now uses the X11 activity backend.
- The backend emits `WallClockElapsed` every one-second poll and
  `ActiveTimeElapsed` when the observed X11 idle time is less than or equal to
  that poll interval.
- Temporary stderr diagnostics are layered through a diagnostic wrapper that
  prints activity samples and backend commands so `make run` can be manually
  inspected before overlay/input behavior exists.
- Unsupported targets keep the no-op backend path.
- Added unit tests for activity classification, queued event ordering, idle
  behavior, and diagnostic line formatting without requiring a live X server.

## Decisions

- The X11 activity backend is permanent production code; only the console
  diagnostics are temporary.
- Activity and idle interpretation stay outside the scheduler. Idle means no
  active-time advancement.
- X11 startup errors are surfaced through the existing public application error
  type without exposing X11 types in the crate API.
- If a break becomes due during this diagnostic phase, the backend prints the
  command instead of attempting overlay, input blocking, or break completion.

## Commands

- `make check`
- `timeout 3s make run` reached the X11 startup path but failed in the sandbox
  with `Operation not permitted` while connecting to X11.

## Follow-up

- Remove temporary console diagnostics after X11 overlay/input integration gives
  visible behavior.
- Continue with `x11-overlay`.
