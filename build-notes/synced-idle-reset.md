# synced-idle-reset

## Goal

- After enough idle time, reset the scheduled break counter so returning to the
  computer starts from the first break slot instead of potentially reaching a
  long break immediately.
- Keep connected sync peers aligned when that reset happens.

## Decisions

- Reuse the existing `breaks.reset_after_idle` threshold and config shape.
- Authenticated remote active-time events still reset local idle tracking, so an
  active connected peer prevents idle reset from firing.
- Add a `SchedulerReset` sync event and bump the sync protocol to version 5.
- Broadcast the reset only when the local scheduler position actually changed.
- Do not add reset epochs; disconnected peers still use the existing
  higher-slot scheduler snapshot catch-up behavior after reconnect.

## Behavior

- Idle reset now clears both accumulated active time and the local completed
  scheduled slot counter.
- Inbound `SchedulerReset` applies the same scheduler-position reset without
  rebroadcasting.
- Inbound resets mark idle reset as already handled until the next local or
  synced active-time event, preventing echo resets while the machine remains
  idle.
- Scheduler reset preserves disabled and pending scheduler state and does not
  clear an active break overlay.

## Tests

- Scheduler tests cover slot restart, change reporting, and preserving pending
  and disabled state.
- Runtime tests cover local reset broadcast, post-reset short-break scheduling,
  inbound reset without rebroadcast, and remote active time preventing idle
  reset.
- Protocol tests cover the version 5 wire shape and `schedulerReset` event.

## Commands

- `make test`
- `make check`
