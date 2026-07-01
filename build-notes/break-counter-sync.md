# break-counter-sync

## Goal

Keep synced peers on the same scheduled break counter, and let a peer that
connects during an active break join the remaining break immediately.

## Decisions

- Bumped the sync protocol to version 4. Backward compatibility is intentionally
  out of scope because this is a single-user deployment.
- `BreakStarted` now carries its origin. Later
  `manual-break-cadence-reset` added a scheduler position snapshot so manual
  break cadence resets converge across peers.
- Added a directed `SchedulerState` sync event sent to each newly authenticated
  peer. It carries the current slot, active elapsed time, and optional active
  break metadata.
- Scheduler state only moves forward: lower remote slots do not rewind the
  global slot, higher remote slots replace local slot and active elapsed, equal
  slots keep the greater active elapsed, and later per-break satisfied slots are
  merged by maximum value.
- Active break snapshots include name, message, origin, start timestamp, and the
  effective lock-after state. Receivers compute remaining time from their local
  clock and the peer timestamp.
- If a received active break has already expired locally, RustEyes skips showing
  it but still advances the scheduled slot.
- Did not add a separate "break that ends last wins" priority rule. Newer
  scheduled slots replace older scheduled slots; same-slot/manual collisions keep
  the existing timestamp and peer-id ordering.

## Implementation

- Runtime reacts to `PeerAuthenticated` by sending a directed scheduler snapshot
  through the existing transport, now with production directed sends enabled.
- Runtime stores active break metadata in `CurrentBreakState` so snapshots can
  describe the visible break.
- Synced scheduled break starts use a scheduler path that validates the named
  break and adopts the supplied slot. Later per-break cadence state is carried
  by the synced scheduler position.
- Joining a mid-break snapshot starts the backend with only the remaining
  duration while preserving the original start timestamp in runtime state.
- Live break replacement now carries the effective lock-after state alongside
  message and remaining time so backend overlay state matches the adopted break.

## Tests

- Scheduler tests cover monotonic slot merging, equal-slot active elapsed merge,
  synced scheduled starts, and stale scheduled starts.
- Protocol tests cover version 4 wire shape, break origin, scheduler snapshots,
  and invalid scheduled slot validation.
- Runtime tests cover directed state send on peer auth, counter catch-up,
  scheduled break counter advancement, manual break cadence reset sync,
  mid-break join, and expired snapshot handling.

## Commands

- `make test` passes (281 tests).
- `make check` passes.
