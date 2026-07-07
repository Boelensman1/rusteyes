# automatic-break-sync

## Goal

- Make scheduled breaks that are triggered by synced active time converge across
  peers the same way manual breaks do.

## Changes

- Runtime now treats active-time propagation and break-start propagation as
  separate decisions.
- Inbound synced active-time events are still not rebroadcast as active time.
- If an inbound active-time event causes the local scheduler to start a
  scheduled break, the resulting `BreakStarted` event is broadcast so peers can
  join the exact break.
- Synced `BreakStarted` application still suppresses rebroadcast, avoiding
  break-start echoes.
- Added a scheduler path for joining an active synced scheduled break at the
  current slot, used when scheduler state has already caught up before the
  active break start arrives. Older scheduled slots remain rejected.

## Tests

- Runtime tests cover synced active time starting and broadcasting a scheduled
  break without echoing the active-time event.
- Runtime tests cover scheduler state for a slot followed by an active
  `BreakStarted` for the same slot.
- Scheduler tests cover current-slot active synced break joins while the
  existing future-only stale scheduled break path remains test-only.

## Commands

- `make test`
- `make check`
