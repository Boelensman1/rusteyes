# backend-trait

## Goal

- Define the crate-internal backend boundary the daemon runtime uses before
  adding platform-specific implementations.

## Changes

- Added `src/backend.rs` with the internal `Backend` trait, `BackendEvent`
  input enum, and `NoopBackend` implementation.
- Moved runtime input events out of `runtime.rs` and into the backend module.
- Kept `resteyes::run()` wired to `NoopBackend`, which still shuts down
  immediately.
- Added backend operations to start a break, clear a break, and request local
  lock.
- Runtime now clears backend break state when a pending break finishes or is
  cleared by disable.
- Runtime requests local lock after an autolock break finishes.
- Updated runtime tests to use a scripted backend that records start, clear,
  and lock requests.

## Decisions

- No production dependencies were added.
- The backend boundary stays crate-internal while platform interfaces are still
  evolving.
- Notifications, tray/menu actions, manual breaks, sync, and X11 behavior
  remain out of scope for this increment.
- Disabling while a break is pending clears backend break state without
  requesting local lock.

## Commands

- `make test`
- `make check`

## Follow-up

- Continue with `x11-activity`.
