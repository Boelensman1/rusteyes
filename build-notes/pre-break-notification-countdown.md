# pre-break-notification-countdown

## Goal

- Replace the one-shot pre-break desktop notification with an updating
  countdown notification that is removed when the break starts.

## Decisions

- Preserve the existing pre-break notice lead of
  `min(30s, breaks.after_active / 2)`.
- Use the actual remaining time for the first notification in the notice
  window, then update only when lower 5-second boundaries are crossed.
- Keep countdown state in the runtime so UI commands remain deterministic and
  testable.
- Store the active pre-break `notify-rust` notification handle in the UI loop,
  update it in place for later countdown commands, and close it on clear.

## Behavior

- A normal 60-second active interval shows `30s`, `25s`, `20s`, `15s`, `10s`,
  and `5s` countdown notifications before clearing the notification as the
  break starts.
- Short active intervals keep the half-interval lead: a 10-second interval
  shows `5s`, and a 20-second interval shows `10s` and `5s`.
- Generic desktop notifications are unchanged.
- If a user manually dismisses a countdown notification, a later countdown
  update may show it again because dismissal callbacks are not tracked.

## Commands

- `cargo test pre_break_notification --lib`
- `make check`
- `make -B macos-app-build`
- Checks were skipped when amending the countdown cadence from 10 seconds to 5
  seconds at user request.
