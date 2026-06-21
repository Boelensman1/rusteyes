# peer-config-compatibility

## Goal

- Reject sync peers whose break behavior configuration differs from the local
  daemon, while still allowing each peer to use its own lock command.

## Changes

- Added a sync compatibility fingerprint derived from validated typed config.
- The fingerprint includes break cadence, idle reset, break type definitions,
  messages, autolock flags, and disable presets.
- The fingerprint excludes lock command, sync enablement, shared secret, peer
  identity, and transport state.
- Sync protocol version 2 `PeerHello` frames now carry only the compatibility
  fingerprint, not raw config values.
- Transport rejects authenticated peers with mismatched fingerprints before
  accepting domain sync events.
- Runtime shows one desktop notification per rejected peer ID per daemon
  session through the existing UI notification path.

## Decisions

- Use HMAC-SHA256 with the sync shared secret and a domain-separation string for
  the compatibility fingerprint.
- Keep raw break messages, timings, and disable presets out of transport hello
  frames because sync frames are authenticated but not encrypted.
- Keep malformed, unauthenticated, unsupported-version, and self-peer failures
  as logs only; only authenticated config incompatibility notifies the user.
- Normalize disable presets by duration before fingerprinting so ordering does
  not affect compatibility.

## Commands

- `cargo check --all-targets --all-features`
- `cargo test --all-targets --all-features sync_protocol`
- `cargo test --all-targets --all-features sync_transport`
- `cargo test --all-targets --all-features runtime`
- `make check`

## Follow-up

- Manual peer mismatch notification verification is pending.
