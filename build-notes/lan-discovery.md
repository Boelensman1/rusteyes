# lan-discovery

## Goal

- Discover authenticated Resteyes peers on the local network before adding
  peer transport or runtime sync behavior.

## Changes

- Added crate-internal mDNS/DNS-SD discovery using the `mdns-sd` crate.
- Added a Resteyes sync service type, `_resteyes-sync._tcp.local.`, with
  peer-specific instance and host names derived from the transient `PeerId`.
- Added authenticated TXT metadata for discovery protocol version, transient
  peer ID, advertised transport port, and HMAC-SHA256 over that metadata using
  the configured sync shared secret.
- Added conversion from resolved mDNS services into crate-internal discovered
  peer records with peer ID, socket address, and observation time.
- Initially added a temporary discovery smoke mode for manual verification:
  `RESTEYES_DISCOVERY_SMOKE=1 RUST_LOG=info RESTEYES_CONFIG=test-configs/sync-discovery.yaml make run`.
  It starts discovery without the platform backend, logs that it is searching,
  and logs each authenticated peer it finds.
- Added trace logging inside discovery startup and peer discovery so the same
  visibility remains useful when the module is wired into runtime sync later.
- Follow-up cleanup in `authenticated-peer-transport` removed the temporary
  smoke environment path. A later transport cleanup replaced timeout-based
  discovery polling with a selected mDNS/shutdown event path.

## Decisions

- Use DNS-SD over mDNS instead of custom IPv4 multicast so discovery works with
  standard local service discovery semantics and supports IPv4 and IPv6 through
  the library.
- Keep discovery as an internal capability only; no daemon runtime wiring,
  active-time transport, scheduler changes, or synced control behavior was
  added in this step.
- Treat discovery authentication as a filter only. Future peer transport must
  still authenticate every sync message because mDNS records can be replayed.

## Verification

- `make check`
- A bounded local two-process smoke run printed `started Resteyes LAN discovery
  smoke test` from both instances and `found authenticated Resteyes peer` for
  both advertised peers.

## Follow-up

- Continue with runtime sync behavior over authenticated peer transport.
