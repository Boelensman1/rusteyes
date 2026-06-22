# Project Rename

## Summary

- Renamed the project to RustEyes for human-facing text.
- Renamed lowercase build/runtime identifiers to `rusteyes`, including the
  Cargo package, binary paths, config directory, log targets, mDNS sync service
  type, helper binary name, and environment variables.
- Kept uppercase environment variable style as `RUSTEYES_*`.
- Renamed the macOS app bundle path to `target/macos/RustEyes.app` and bundle
  identifier to `dev.rusteyes.RustEyes`.
- Renamed the Swift helper target/source path to `RustEyesMacOSHelper`, while
  keeping the executable product name `rusteyes-macos-helper`.

## Verification

- `make check` passes after the rename.

## Follow-Up

- Generated outputs under `target/` and SwiftPM `.build/` may still contain
  old build artifact names until rebuilt or cleaned.
