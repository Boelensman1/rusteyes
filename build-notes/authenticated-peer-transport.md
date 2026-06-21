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
- Follow-up cleanup split the transport facade, worker commands, worker loop,
  and connection tracking into smaller modules while preserving the
  crate-internal transport API.
- Added protocol-level replay protection for inbound domain events: the
  transport now tracks the highest accepted event sequence per authenticated
  peer and drops stale or repeated sequences before forwarding events.
- Follow-up API cleanup made disabled sync an inert `SyncTransport`, moved
  inbound event polling onto the transport facade, returned a named transport IO
  binding from listener setup, split connection success/failure into explicit IO
  events, and moved sender/replay validation behind the connection tracker.
- Follow-up API cleanup unified peer authentication, disconnection, and
  authenticated domain messages behind one `SyncTransportEvent` facade stream,
  renamed outbound facade calls to `broadcast_event` and `send_event`, hid wire
  sequence numbers from runtime-facing events, and simplified connection binding
  results to a peer result plus endpoints to disconnect.
- Follow-up API cleanup moved transport frame encoding, hello framing, inbound
  decoding, and outbound sequence allocation behind a private transport session
  helper, renamed nonblocking inbound polling to `try_recv_event`, added a
  facade event drain helper, made connection authentication/removal outcomes
  explicit, and centralized failed endpoint closure in the worker.
- Removed the temporary `RESTEYES_DISCOVERY_SMOKE` path now that discovery is
  started by normal sync-enabled runtime startup.
- Follow-up cleanup removed transport polling: the worker now blocks on
  `message-io` events and uses node signals to wake for facade commands or
  shutdown, while discovery uses `flume::Selector` to wait for mDNS events or
  an explicit shutdown signal.
- Follow-up cleanup moved remaining transport facade command, reply, and event
  channels from `std::sync::mpsc` to `flume`; one-shot reply channels now use
  `flume::bounded(1)`.

## Decisions

- Bind the production transport listener to `0.0.0.0:0` and advertise the
  assigned port through mDNS instead of adding sync port config.
- Keep this step transport-only: authenticated domain sync events are exposed
  through the transport facade but are not applied to runtime scheduling yet.
- Treat listener and discovery startup failures as fatal when sync is enabled;
  later connection/authentication failures are logged and the daemon continues.
- Reserve sequence `0` for transport hello and start local outbound domain
  event sequences at `1`.
- Add direct `flume` dependency with only the `select` feature because
  discovery needs to select between the `mdns-sd` service receiver and shutdown
  without reintroducing timeout polling.

## Verification

- `cargo test --all-targets --all-features --no-run`
- `cargo test --all-targets --all-features`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `make check`
- `cargo test --all-targets --all-features sync_transport`
- Re-ran `cargo test --all-targets --all-features sync_transport` and
  `make check` after the unified event facade cleanup.
- Re-ran `cargo test --all-targets --all-features sync_transport` after the
  transport session and connection outcome API cleanup.
- `make check` passes after the transport session and connection outcome API
  cleanup.
- `cargo check --all-targets --all-features` passes after the wake-driven
  worker and discovery cleanup.
- `make check` passes after the wake-driven worker and discovery cleanup.
- `cargo test --all-targets --all-features sync_transport` passes after the
  transport channel cleanup.
- `make check` passes after the transport channel cleanup.

## Follow-up

- Continue with `active-time-sync`: broadcast local active-time increments and
  apply authenticated remote increments through the scheduler.
