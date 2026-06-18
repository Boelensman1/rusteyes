# scheduler-disable-state

## Goal

- Add explicit local disable and enable state to the deterministic scheduler.

## Changes

- Added disabled state to `BreakScheduler`.
- Added `disable`, `enable`, and `is_disabled` scheduler controls for later
  runtime wiring.
- Disabled schedulers ignore active-time increments and never start breaks.
- Disabling clears any pending break and resets accumulated active time while
  preserving the completed slot count.
- Kept runtime behavior unchanged; the app still prints `hello world`.

## Decisions

- Disable resets accumulated active time.
- Disable does not rewind completed slots. If a pending break is cleared by
  disabling, the next enabled break advances from the next slot.
- Runtime conversion of finite disable presets into monotonic deadlines is
  deferred to a later daemon runtime step.
- Disable-until-restart remains daemon state outside the scheduler.

## Commands

- `make check`

## Follow-up

- Continue with `daemon-runtime-noop`.
