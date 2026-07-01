# active-time-sync

## Goal

- Broadcast and apply authenticated active-time increments across synced peers.

## Decisions

- Use additive active-time deltas for v1.
- Do not deduplicate overlapping real-world active seconds across peers.
- Apply remote active-time increments through the same scheduler path as local
  active-time increments.
- Simultaneous activity on multiple peers may make breaks arrive faster.

## Behavior

- A peer broadcasts active-time elapsed while it is locally active.
- A receiving peer treats authenticated remote active-time elapsed as active
  time for the shared break cadence.
- Remote active-time increments are ignored while local scheduling is disabled
  or while a break is pending, matching existing scheduler behavior.
- Local or synced break starts reset accumulated active time.
- Manual breaks also reset accumulated active time. Later
  `manual-break-cadence-reset` made them satisfy the selected break cadence,
  plus more frequent cadences, at the current slot.

## Changes

- Runtime now keeps a sync broadcaster next to the existing selected sync event
  receiver.
- Local backend-originated active-time events broadcast
  `ActiveTimeElapsed { elapsed }` before advancing the local scheduler.
- Authenticated remote active-time transport events advance the scheduler
  through the same path as local active time.
- Remote active-time events are not rebroadcast.
- Peer lifecycle events and non-active-time domain events remain logged/ignored
  until later sync increments.
- Sync broadcast failures are logged as degraded sync and do not stop local
  scheduling.

## Protocol Notes

- Active-time messages should carry an elapsed active duration.
- Sender identity, sequencing, resend behavior, and duplicate handling remain
  deferred to the sync protocol and transport steps.

## Tests

- Remote active-time increments can trigger scheduled breaks.
- Disabled and pending-break scheduler states suppress remote active-time
  increments.
- Simultaneous peer activity adds together.
- A synced break start resets accumulated active time.

## Commands

- `cargo check --all-targets --all-features`
- `cargo test --all-targets --all-features runtime`
- `make check`
