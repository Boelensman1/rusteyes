# macos-break-diagnostics

## Summary

- Added temporary macOS break diagnostics behind `RUSTEYES_BREAK_DIAGNOSTICS=1`
  and Nix `services.rusteyes.breakDiagnostics.enable`.
- Diagnostics log the macOS break tail path from Rust and the Swift helper:
  overlay samples near the end, helper command send/ack, overlay clear,
  watchdogs, force-exit requests, and helper-native lock start/return.
- The helper protocol is version 8. `activitySample` now carries optional
  force-exit fields and an overlay-state string for diagnostic logs.
- With diagnostics enabled, the helper shows a temporary `Force exit` overlay
  button. Clicking it clears the overlay immediately and reports the request to
  Rust so scheduler state can finish without lock-after-break.
- With diagnostics enabled, RustEyes creates `/tmp/rusteyes-force-clear` as a
  world-writable regular file (`0666`). Appending to that file from another
  user during an active break asks Rust to send `clearBreak`, finish the break
  state without locking, and truncate the trigger file.
- Helper-native macOS autolock now runs asynchronously after `finishBreak`
  command completion, so lock execution cannot block helper acknowledgement of
  overlay cleanup.

## Decisions

- Keep this explicitly temporary and opt-in; normal breaks remain unskippable.
- Use a machine-global `/tmp` file because the diagnostic recovery path must
  work after switching to another macOS user.
- Keep the file path configurable with `RUSTEYES_FORCE_CLEAR_PATH`, but have the
  Nix option use `/tmp/rusteyes-force-clear`.
- Require `launchAgent.enable` for Darwin Home Manager diagnostics because
  Login Item launches cannot receive module-managed environment variables.

## Usage

```nix
services.rusteyes = {
  enable = true;
  launchAgent.enable = true;
  breakDiagnostics.enable = true;
  logLevel = "rusteyes=trace";
};
```

During a stuck break, switch users and run:

```sh
printf force >> /tmp/rusteyes-force-clear
```

macOS LaunchAgent logs are written to
`~/Library/Logs/rusteyes.out.log` and `~/Library/Logs/rusteyes.err.log`.

## Verification

- `nix develop --command cargo test --lib macos_helper` passes (31 tests).
- `make check` passes.
- `make -B macos-helper-build` passes.

## Follow-up

- Reproduce the long-break lock/unlock issue with diagnostics enabled and use
  the log sequence to decide whether the permanent fix belongs in activity
  sampling, helper command handling, watchdog behavior, or macOS autolock.
- Remove the temporary force-exit UI and shared trigger when the root cause is
  fixed.
