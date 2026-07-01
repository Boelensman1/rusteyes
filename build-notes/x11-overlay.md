# x11-overlay

## Goal

- Show visible X11 break overlays without window-manager involvement.

## Changes

- Added a Linux-only X11 overlay helper backed by the existing `x11rb`
  dependency with the `randr` feature enabled.
- The overlay helper discovers connected RandR outputs through output and CRTC
  info, de-duplicates monitor geometries, sorts them by position, and falls
  back to the root screen geometry when RandR monitor discovery is unavailable.
- Break overlays create one black `override_redirect` input/output window per
  monitor, so the window manager should not decorate, move, resize, or manage
  them.
- Overlay windows are mapped above other windows, periodically raised while a
  break is active, and redrawn on expose events.
- The first configured break message is rendered in white with X11 core text.
- Linux runtime now uses the production X11 activity backend directly; the
  temporary diagnostic wrapper and stderr command printing were removed.
- The Linux backend now handles break commands: `StartBreak` shows overlays and
  suppresses active-time polling while a break is visible, wall-clock time
  continues, the break countdown is computed from a monotonic deadline,
  `BreakFinished` is emitted after the configured break duration, and
  `ClearBreak` destroys overlay resources.
- Follow-up cleanup removed unused monitor names from overlay geometry, avoided
  CRTC queries for outputs that cannot become monitors, and dropped a redundant
  per-window raise during overlay creation.
- Follow-up refinement removed the overlay-period idle check so break duration is
  wall-clock time while the overlay is visible.
- Follow-up fix changed visible-break countdowns from fixed 500 ms decrements to
  a monotonic deadline. Overlay samples still run for UI refresh, event
  handling, and keeping windows raised, but sample cadence no longer defines
  break length.

## Decisions

- This increment intentionally did not grab or block input; `x11-input-blocking`
  completed that follow-up.
- One deterministic message is shown: the first configured message for the due
  break type. Message randomization or cycling remains deferred.
- Break duration is measured against a monotonic deadline while the overlay is
  visible; normal keyboard and pointer activity is blocked by the later
  `x11-input-blocking` step.
- Overlay failures during backend command handling are logged and cause the
  backend to shut down, because backend commands are currently fire-and-forget.
- X11 connection teardown still provides a fallback cleanup path for server-side
  resources, and the backend also explicitly destroys overlays on clear/drop.

## Commands

- `make test`
- `make lint`
- `make check`
- `make check` after switching break countdown to wall-clock overlay ticks.
- `make check` after switching visible-break countdowns to monotonic deadlines.

## Follow-up

- Fulfilled by `x11-input-blocking`.
- Manual X11 overlay verification still needs to be run in a real X session;
  this environment does not provide usable X server access.
