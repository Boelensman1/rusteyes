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
- Runtime now tracks whether the current break should lock afterward as
  break-local state initialized from configured autolock, ignores stale
  requests outside a break, clears the request after finish or disable, and
  requests the local lock when that state is set.
- Breaks already configured with autolock show the lock control in the
  requested state from the start.

## Decisions

- The lock control is one-way for the current break and does not modify YAML
  config.
- The overlay continues using core X11 drawing and the existing `x11rb`
  dependency; richer fonts or toolkit UI remain out of scope.
- Lock requests are queued before `BreakFinished` when both happen in the same
  overlay polling tick, so a final-tick click applies to that break.
- Follow-up fix: lock-control clicks use root coordinates across all overlay
  windows instead of the grabbed event window's local coordinates, and accepted
  clicks redraw immediately so the requested state is visible.
- Follow-up cleanup: overlay windows now select only the consumed exposure and
  button-press events, and pure overlay layout/text helpers are grouped under a
  private layout module.
- Follow-up cleanup: lock-control hit testing now depends only on the active
  control bounds, and the break timer stores remaining duration without a
  separate finished flag.

## Commands

- `make test`
- `make check`
- `make test` after the lock-control hit-test fix.
- `make check` after the lock-control hit-test fix.
- `make test` after follow-up cleanup.
- `make check` after follow-up cleanup.

## Follow-up

- Run manual X11 verification for the updated overlay in a real X session.
