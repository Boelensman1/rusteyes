# daemon-runtime-noop

## Goal

- Wire config and scheduler into a daemon-shaped runtime before adding
  platform-specific backends.

## Changes

- Replaced the placeholder `hello world` runtime with a private daemon loop.
- The runtime loads config, builds a validated `BreakSchedule`, and owns a
  `BreakScheduler`.
- Added internal runtime events for active time, wall-clock elapsed time, break
  completion, finite disable, disable-until-restart, explicit enable, and
  shutdown.
- Added a no-op backend for the current executable path. It emits shutdown
  immediately so startup wiring is checked without platform behavior.
- Added scripted runtime tests covering scheduled breaks, break completion,
  timed disable, disable-until-restart, shutdown, and scheduler setup errors.
- The public API remains `resteyes::run()` with an application error wrapper.

## Decisions

- No production dependencies were added.
- Platform activity, overlays, input blocking, tray actions, and local lock
  remain out of scope for this increment.
- Runtime finite disable state is tracked outside the scheduler as remaining
  wall-clock duration.
- The no-op backend intentionally produces no app output.

## Commands

- `make run`
- `make check`

## Follow-up

- Continue with `backend-trait`.
