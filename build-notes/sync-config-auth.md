# sync-config-auth

## Goal

- Add config-only sync authentication settings before introducing sync protocol,
  discovery, transport, or runtime behavior.

## Changes

- Added `sync.enabled`, defaulting to `false`.
- Added `sync.shared_secret`, required only when sync is enabled.
- Shared secrets are validated when present: no blank values, no surrounding
  whitespace, and at least 32 characters.
- Shared secrets use a wrapper with redacted `Debug` output.
- Project planning docs now defer peer identity to the sync protocol step as a
  transient per-start sender identity rather than a YAML setting.

## Decisions

- No `peer_id` config key is accepted.
- Disabled sync may omit the shared secret.
- Sync remains config-only in this step; no protocol, discovery, transport,
  runtime wiring, or production dependency was added.

## Commands

- `make check` initially failed because a runtime test fixture constructed
  `Config` without the new `sync` field.
- `make check`

## Follow-up

- Defer `sync-protocol`, including authenticated event shape and the transient
  sender identity, until after macOS backend parity.
