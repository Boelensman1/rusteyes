# manual-break-control

## Goal

- Add runtime control flow for starting configured named breaks on demand while
  reusing the existing backend break overlay path.

## Changes

- Added `RuntimeEvent::StartManualBreak` as the crate-internal input for future
  tray, sync, or control sources.
- Added `BreakOrigin` so break payloads distinguish scheduled slot breaks from
  manual breaks without giving manual breaks a fake slot.
- Scheduler can start a configured manual break by name from active or disabled
  state.
- Manual break starts reset accumulated active time. Later
  `manual-break-cadence-reset` made manual starts also satisfy the selected
  break type's cadence, plus more frequent break types, at the current slot.
- Manual breaks started while disabled return to disabled scheduling after they
  finish, unless the disable period expires during the break.
- Runtime starts manual breaks through the existing `StartBreak` command and
  finishes them through the existing `FinishBreak { lock_after }` command.
- Follow-up cleanup removed the scheduler's cloned pending break payload; the
  scheduler now reports whether finish/disable affected a pending break, and
  runtime owns the named current-break lock state.

## Decisions

- Unknown manual break names are ignored.
- No separate pending-break override behavior is needed because scheduled breaks
  are handed to the backend immediately.
- No tray/menu UI, sync input, platform-specific control source, or production
  dependency was added in this increment.

## Commands

- `make test`
- `make check` initially failed on Clippy `similar_names` in new tests; renamed
  local variables.
- `make check`
- `make test` after follow-up cleanup.
- `make check` after follow-up cleanup.

## Follow-up

- Continue with sync configuration/authentication before adding actual remote
  control inputs.
