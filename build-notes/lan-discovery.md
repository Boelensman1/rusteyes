# lan-discovery

## Goal

- Discover authenticated Resteyes peers on the local network before adding
  peer transport or runtime sync behavior.

## Changes

- Added crate-internal mDNS/DNS-SD discovery using the `mdns-sd` crate.
- Added a Resteyes sync service type, `_resteyes-sync._udp.local.`, with
  peer-specific instance and host names derived from the transient `PeerId`.
- Added authenticated TXT metadata for discovery protocol version, transient
  peer ID, advertised transport port, and HMAC-SHA256 over that metadata using
  the configured sync shared secret.
- Added conversion from resolved mDNS services into crate-internal discovered
  peer records with peer ID, socket address, and observation time.

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

## Follow-up

- Continue with `authenticated-peer-transport`.
