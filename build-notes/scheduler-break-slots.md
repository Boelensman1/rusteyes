# scheduler-break-slots

## Goal

- Add deterministic break slot scheduling without runtime or backend wiring.

## Changes

- Added an internal scheduler that consumes active-time `Duration` values rather
  than `Instant` values, raw input events, or backend-specific activity state.
- Added `BreakScheduler::advance_active`, which accumulates active time, advances
  slots using `breaks.after_active`, and starts a configured break when the
  current slot is due.
- Added owned scheduled break snapshots with the selected break name, slot,
  duration, messages, and autolock flag.
- Added pending-break behavior so active time stops advancing once a break is
  due until `finish_break` is called.
- Later cleanup introduced `BreakSchedule` as the validated internal scheduler
  input. It owns sorted break rules and centralizes the due-break selection
  policy.
- Kept runtime behavior unchanged; the app still prints `hello world`.

## Decisions

- The scheduler owns a `BreakSchedule` built from validated `Breaks` config and
  stays crate-internal.
- Activity and idle interpretation stay outside the slot scheduler; future
  runtime code should convert observed activity into active-time durations.
- If multiple break types are due for a slot, the break type with the largest
  interval wins.
- If a large active-time delta crosses several slots, scheduling stops at the
  first due break and any remainder is discarded while the break is pending.
- Later cleanup removed scheduler dead-code allowances after daemon runtime
  wiring started using scheduler state transitions directly.

## Commands

- `make check` failed initially because the new internal scheduler is not yet
  wired into non-test runtime code.
- `make check`
- `make check` after internal schedule cleanup

## Follow-up

- Continue with `scheduler-disable-state`.
