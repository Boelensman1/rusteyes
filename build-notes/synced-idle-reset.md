# synced-idle-reset

## Goal

- After enough idle time, reset the scheduled break counter so returning to the
  computer starts from the first break slot instead of potentially reaching a
  long break immediately.
- Keep connected sync peers aligned when that reset happens.

## Decisions

- Split break-count reset from the existing active-time reset threshold.
- Keep `breaks.reset_after_idle` as the active-time reset knob, defaulting to
  `5m`.
- Add `breaks.reset_count_after_idle` as the break-count reset knob, defaulting
  to `1h` and disabled with `null`.
- Authenticated remote active-time events still reset local idle tracking, so an
  active connected peer prevents both idle reset timers from firing.
- Add a `SchedulerReset` sync event and bump the sync protocol to version 5.
- Broadcast the reset only when the local scheduler position actually changed.
- Do not add reset epochs; disconnected peers still use the existing
  higher-slot scheduler snapshot catch-up behavior after reconnect.
- Follow-up fix: use wall-clock gaps between activity samples as idle time for
  reset tracking, so sleep can trigger the break-count reset after wake.

## Behavior

- Active-time idle reset still clears only accumulated active time.
- Break-count idle reset clears both accumulated active time and the local
  completed scheduled slot counter.
- Long unobserved gaps are treated as idle time before the first post-wake
  active tick is counted; sleep still does not count as active time.
- Inbound `SchedulerReset` applies the same scheduler-position reset without
  rebroadcasting.
- Inbound resets mark both idle reset timers as already handled until the next
  local or synced active-time event, preventing echo resets while the machine
  remains idle.
- Scheduler reset preserves disabled and pending scheduler state and does not
  clear an active break overlay.

## Tests

- Scheduler tests cover slot restart, change reporting, and preserving pending
  and disabled state.
- Runtime tests cover local reset broadcast, post-reset short-break scheduling,
  active-time reset preserving the completed slot counter, inbound reset without
  rebroadcast, and remote active time preventing idle reset.
- Follow-up runtime tests cover sleep-gap resets before the first post-wake
  active tick and disabled break-count reset behavior.
- Config tests cover `reset_count_after_idle` defaults, overrides, `null`, and
  zero-duration validation.
- Protocol tests cover the version 5 wire shape and `schedulerReset` event.

## Commands

- `make test`
- `make check`
- `make test` after making idle reset sleep-aware
