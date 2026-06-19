# x11-ui-improvements

## Goal

- Improve the X11 break overlay with remaining break time and a way to request
  local locking after the current break finishes.

## Changes

- X11 overlays now draw the configured break message, the remaining break time,
  and a centered `Lock after break` control on each monitor.
- Remaining time is rendered from the backend break timer and still counts down
  only while overlay samples are idle.
- Clicking the lock control queues a runtime event that applies only to the
  currently active break.
- Runtime now tracks lock-after-current-break as break-local state, ignores
  stale requests outside a break, clears the request after finish or disable,
  and requests the local lock when either the break is configured for autolock
  or the current-break request was made.
- Breaks already configured with autolock show the lock control in the
  requested state from the start.

## Decisions

- The lock control is one-way for the current break and does not modify YAML
  config.
- The overlay continues using core X11 drawing and the existing `x11rb`
  dependency; richer fonts or toolkit UI remain out of scope.
- Lock requests are queued before `BreakFinished` when both happen in the same
  overlay polling tick, so a final-tick click applies to that break.

## Commands

- `make test`
- `make check`

## Follow-up

- Run manual X11 verification for the updated overlay in a real X session.
