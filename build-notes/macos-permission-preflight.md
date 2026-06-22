# macos-permission-preflight

## Summary

- macOS helper IPC now uses protocol version 4. Version 3 introduced permission
  preflight; the later protocol bump added helper command acknowledgements.
- Added a `preflightPermissions` daemon-to-helper message and a
  `preflightResult` helper-to-daemon response with Accessibility and Input
  Monitoring trust booleans.
- macOS startup now runs permission preflight immediately after the helper
  handshake and before daemon scheduling can start.
- The helper requests Accessibility trust with
  `AXIsProcessTrustedWithOptions` and `kAXTrustedCheckOptionPrompt`, checks
  Input Monitoring with `CGPreflightListenEventAccess`, and keeps stdout
  JSON-protocol-only.
- Missing permissions fail startup with an error naming the missing permission,
  the System Settings Privacy & Security area, the development helper
  executable name, and the need to restart RustEyes after granting access.
- The existing break-time event tap failure path remains in place for
  permissions revoked after startup.

## Decisions

- Run the preflight in the Swift helper because macOS privacy trust is tied to
  the executable requesting access.
- Treat missing permissions as a fatal startup error instead of allowing a
  break to fail later.
- Ask for Accessibility explicitly, but only report missing Input Monitoring;
  the user must grant Input Monitoring in System Settings.
- Keep this step out of config and avoid new production dependencies.

## Verification

- `make test` passed.
- `make macos-helper-build` initially failed because this SDK imports
  `kAXTrustedCheckOptionPrompt` as `Unmanaged<CFString>`; using
  `takeUnretainedValue()` fixed the bridge.
- `make macos-helper-build` passed.
- `make check` initially found Clippy issues in the new tests; after replacing
  `expect_err` and removing unnecessary parentheses, `make check` passed.
- A helper smoke test for protocol version 3 `hello` followed by `shutdown`
  returned `ready` and `shutdownComplete`.
- A helper smoke test for `preflightPermissions` was not run because it would
  intentionally trigger the macOS Accessibility prompt.
- `make check` passed after clarifying the missing-permission guidance and
  updating protocol tests for version 4.
- `make macos-helper-build` passed after the protocol version 4 update.

## Follow-up

- Manual macOS verification should run `make run` and confirm startup prompts
  for Accessibility, reports missing Input Monitoring before any break, and
  starts normally after both permissions are granted.
