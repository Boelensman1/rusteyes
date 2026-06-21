# synced-lock-after-break

## Goal

- Apply lock-after-current-break decisions across authenticated synced peers.

## Decisions

- Reuse the existing version 1 `LockAfterCurrentBreak` sync event; no protocol
  or config changes are needed.
- Treat lock-after-current-break as break-local state. Requests outside an
  active break are ignored.
- De-duplicate repeated local or remote requests once the current break is
  already marked for locking.
- Add an internal backend command to update active overlay lock state without
  changing the helper wire protocol.

## Behavior

- Local lock-after-current-break requests mark the current break for locking,
  update the active overlay state, and broadcast `LockAfterCurrentBreak`.
- Inbound authenticated `LockAfterCurrentBreak` events apply locally without
  rebroadcasting.
- X11 redraws the active overlay lock control when a synced request applies.
- macOS reuses the existing helper `updateBreak` command to refresh the active
  overlay lock-control state.
- Break finish still uses each platform's existing local lock path.

## Commands

- `cargo test --all-targets --all-features runtime`
- `make check`
