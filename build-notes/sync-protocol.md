# sync-protocol

## Goal

- Define authenticated sync message framing before adding LAN discovery,
  transport, or runtime behavior.

## Changes

- Added a crate-internal `sync_protocol` module with protocol version 1 JSON
  messages.
- Added transient `PeerId` generation and parsing as 128-bit lowercase hex.
- Added sync events for active-time increments, named break starts, finite
  disables, disable-until-restart, enable, and lock-after-current-break.
- Added HMAC-SHA256 authentication over the canonical JSON message payload,
  with the MAC encoded as lowercase hex beside the payload.
- Moved `serde_json` to a normal dependency and added direct `getrandom`,
  `hmac`, and `sha2` dependencies for protocol support.
- Exposed configured shared-secret bytes crate-internally while preserving
  redacted `Debug` output.

## Decisions

- Keep this step protocol-only: no peer discovery, sockets, runtime wiring,
  duplicate suppression, scheduler changes, or synced break behavior.
- Carry sequence numbers in the message now, but defer assignment and
  deduplication policy to transport/runtime work.
- Send break starts by configured break name only; receiving peers will resolve
  local duration, messages, and autolock settings in a later step.
- Represent durations as unsigned milliseconds and reject zero durations.

## Verification

- `make test`
- `make check`

## Follow-up

- Continue with `lan-discovery`.
