# backend-trait

## Goal

- Define the crate-internal backend boundary the daemon runtime uses before
  adding platform-specific implementations.

## Changes

- Added `src/backend.rs` with the internal `Backend` trait, `RuntimeEvent`
  input enum, backend command enum, and initial `NoopBackend` implementation.
- Moved runtime input events out of `runtime.rs` and into the backend module.
- Kept `rusteyes::run()` wired to `NoopBackend`, which still shuts down
  immediately.
- Added backend commands to start a break, clear a break, and request local
  lock through one command handler.
- Runtime now clears backend break state when a pending break finishes or is
  cleared by disable.
- Runtime requests local lock after an autolock break finishes.
- Follow-up fix removed the no-op backend from production unsupported-target
  startup. Unsupported targets now return a clear missing-backend error, and the
  no-op backend became test-only.
- Later cleanup grouped finite and until-restart disable inputs under one
  `DisableRequest` event payload.
- Later cleanup made scheduler break completion and disable transitions return
  the affected pending break, so runtime no longer peeks at scheduler pending
  state before clearing backend break state.
- Later cleanup gave `Backend::handle_command` a default no-op implementation
  so backends only override it when they handle commands.
- Updated runtime tests to use a scripted backend that records ordered backend
  commands.
- Later cleanup removed the unused test-only `NoopBackend` after scripted
  runtime tests and platform backends made it redundant.

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
- `make check` after backend default-command cleanup
- `make check` after unsupported-platform startup fix
- `make run` after unsupported-platform startup fix (expected non-zero exit)

## Follow-up

- Continue with `x11-activity`.
