# manual-break-cadence-reset

## Goal

- A manual long break should satisfy the long-break cadence so a scheduled long
  break does not immediately follow it.
- Keep the behavior generic for arbitrary break types, including configurations
  with a less frequent break beyond `long`.

## Decisions

- Replace pure modulo scheduling with per-break last-satisfied slot state.
- A manual break satisfies its selected break type and any more frequent break
  types at the current slot.
- A manual break does not satisfy less frequent break types, so an even-longer
  cadence is not postponed by a manual long break.
- Keep manual break origins slotless; sync carries the scheduler position
  separately instead of inventing a scheduled slot for manual breaks.

## Changes

- Scheduler due checks now compare each break type's interval against its own
  last-satisfied slot.
- Scheduled breaks satisfy all currently due break types that the selected break
  supersedes.
- Scheduler positions now include per-break last-satisfied slots.
- Sync protocol version 6 adds scheduler position snapshots to `BreakStarted`
  and per-break last-satisfied slots to `SchedulerState`.
- Runtime broadcasts and applies those snapshots for local, remote, and
  reconnect break state.

## Tests

- Scheduler tests cover manual cadence satisfaction, preserving less frequent
  cadences, and per-break synced position merging.
- Runtime tests cover local and remote manual long breaks delaying the next
  scheduled long break without rebroadcasting inbound manual breaks.
- Protocol tests cover the version 6 wire shape and invalid scheduler position
  validation.

## Commands

- `make test`
- `make check`
