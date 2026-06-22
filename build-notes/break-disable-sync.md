# break-disable-sync

## Goal

- Broadcast and apply authenticated break-start and disable/enable sync events
  across peers.

## Decisions

- Reuse the existing version 1 sync event variants; no protocol changes are
  needed.
- Treat inbound `BreakStarted` as a configured named break start using the
  manual-break scheduler path. This resets accumulated active time, does not
  advance break slots, and preserves disabled-state resume behavior.
- Ignore unknown inbound break names.
- Apply inbound timed disables from the moment they are received.
- Keep synced `LockAfterCurrentBreak` behavior deferred to the later
  `synced-lock-after-break` step.

## Behavior

- Local active-time due breaks and local manual break starts broadcast
  `BreakStarted`.
- Local disable-for and disable-until-restart actions broadcast their matching
  sync events.
- Local timed disables re-enable from wall-clock elapsed time without sending a
  separate enable event; authenticated inbound enable events are still accepted.
- Authenticated inbound break-start and disable/enable events apply locally and
  are not rebroadcast.
- Inbound disable events clear a pending local break through the same backend
  cleanup path as local disable actions.

## Cleanup

- Collapsed separate local and synced runtime helper paths behind a small sync
  propagation flag so scheduler behavior stays shared while rebroadcast policy
  remains explicit.
- Removed thin backend actor channel wrapper types; `BackendActor` now stores
  the flume channels directly.
- Marked directed sync transport send/poll helpers as test-only where runtime
  now uses the event receiver API.
- Reworked runtime sync tests to drive ordered runtime inputs directly instead
  of using sleep-delayed backend and sync event sources.
- Removed the stale test-only local runtime enable event so the shared runtime
  event enum no longer needs a broad dead-code allowance.

## Commands

- `cargo test --all-targets --all-features runtime`
- `make check`
- `cargo check --all-targets --all-features`
- `make fmt-check`
