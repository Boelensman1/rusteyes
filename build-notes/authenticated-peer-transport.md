# authenticated-peer-transport

## Goal

- Start authenticated peer transport for sync-enabled daemons before applying
  any remote runtime behavior.

## Changes

- Added `message-io` with framed TCP support for peer communication.
- Added crate-internal sync transport startup that listens on an OS-assigned
  TCP port, advertises that port through LAN discovery, connects to discovered
  authenticated peers, and keeps the transport alive for the daemon lifetime.
- Added authenticated `PeerHello` sync messages using sequence `0`; every
  transport connection must send a valid HMAC-framed hello before it is tracked
  as a peer.
- Follow-up refactor split transport control frames from domain sync events:
  `PeerHello` now travels as authenticated transport control, while scheduler
  sync events remain domain payloads.
- Wrapped `message-io` behind a small private transport adapter so peer
  authentication, send/broadcast, and inbound event forwarding do not depend on
  `message-io` types outside the adapter.
- Added peer connection tracking that rejects self-connections, drops failed or
  unauthenticated endpoints, and collapses duplicate peer connections with a
  deterministic rule: the lower peer ID keeps its outgoing connection and the
  higher peer ID keeps its incoming connection.
- Added a crate-internal transport API for broadcasting to all authenticated
  peers, sending to one authenticated peer, and receiving authenticated inbound
  domain events with sender and sequence metadata.
- Removed the temporary `RESTEYES_DISCOVERY_SMOKE` path now that discovery is
  started by normal sync-enabled runtime startup.

## Decisions

- Bind the production transport listener to `0.0.0.0:0` and advertise the
  assigned port through mDNS instead of adding sync port config.
- Keep this step transport-only: authenticated domain sync events are exposed
  through the inbound receiver but are not applied to runtime scheduling yet.
- Treat listener and discovery startup failures as fatal when sync is enabled;
  later connection/authentication failures are logged and the daemon continues.
- Reserve sequence `0` for transport hello and start local outbound domain
  event sequences at `1`.

## Verification

- `cargo test --all-targets --all-features --no-run`
- `cargo test --all-targets --all-features`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `make check`

## Follow-up

- Continue with `active-time-sync`: broadcast local active-time increments and
  apply authenticated remote increments through the scheduler.
