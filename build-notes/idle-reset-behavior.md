# idle-reset-behavior

## Goal

- Reset accumulated active time after enough combined idle time.

## Decisions

- Add `breaks.reset_after_idle` as an optional duration under `breaks`.
- Default idle reset to `5m`.
- Treat `breaks.reset_after_idle: null` as disabled.
- Keep sync protocol unchanged: local idle reset is not broadcast.
- Treat authenticated remote active-time events as combined activity so one
  active synced peer prevents another idle peer from resetting.

## Behavior

- Normal activity polling emits idle-duration runtime events in addition to
  wall-clock events.
- Runtime tracks continuous idle time since the last local or synced active-time
  event.
- When idle time reaches `breaks.reset_after_idle`, runtime resets scheduler
  accumulated active time, clears any pre-break notification state, and updates
  the tray/menu-bar active-time row.
- Scheduler reset preserves completed break slots, pending breaks, and disabled
  state.
- Overlay break polling remains unchanged and does not emit idle reset events.

## Cleanup

- Removed the platform-specific dead-code allowance from the shared break timer
  remaining-time accessor after macOS overlay code started using it.

## Commands

- `cargo check --all-targets --all-features` failed locally because `cargo` is
  not on `PATH`; use the Make/Nix fallback.
- `make check`
- `make check` after removing stale dead-code allowances

## Follow-up

- Manual synced multi-machine timing verification is pending.
