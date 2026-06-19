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
- This increment temporarily layered stderr diagnostics through a wrapper that
  printed activity samples and backend commands; `x11-overlay` removed that
  wrapper after visible break behavior existed.
- Unsupported targets report a clear missing-backend startup error instead of
  silently using the no-op backend path.
- Added unit tests for activity classification, queued event ordering, idle
  behavior, and overlay break countdown helpers without requiring a live X
  server.

## Decisions

- The X11 activity backend is permanent production code; the temporary console
  diagnostics were removed by `x11-overlay`.
- Activity and idle interpretation stay outside the scheduler. Idle means no
  active-time advancement.
- X11 startup errors are surfaced through the existing public application error
  type without exposing X11 types in the crate API.
- Break due handling is provided by the later `x11-overlay` and
  `x11-input-blocking` increments.

## Commands

- `make check`
- `timeout 3s make run` reached the X11 startup path but failed in the sandbox
  with `Operation not permitted` while connecting to X11.

## Follow-up

- Fulfilled by `x11-overlay`: removed temporary console diagnostics after break
  commands gained visible overlay behavior.
- Fulfilled by `x11-input-blocking`: visible overlays now grab keyboard and
  pointer input while a break is active.
