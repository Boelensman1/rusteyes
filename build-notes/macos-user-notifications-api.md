# macos-user-notifications-api

## Goal

- Move macOS desktop notifications to `notify-rust`'s
  `UNUserNotificationCenter` backend while keeping local development builds
  launchable with ad-hoc signing.

## Decisions

- Enable `notify-rust`'s `preview-macos-un` feature only for macOS so the Rust
  UI uses `UNUserNotificationCenter` there while Linux keeps the existing
  notification path.
- Request macOS notification authorization during UI startup and log denied or
  failed permission requests without failing RustEyes startup.
- Keep time-sensitive delivery out of scope for now. The required entitlement is
  restricted, and macOS kills ad-hoc signed binaries that embed it.
- Explicitly ad-hoc sign the generated local `RustEyes.app` bundle after copying
  the Rust binary and helper so the `UNUserNotificationCenter` path has a
  bundled, signed app identity.

## Behavior

- Pre-break notifications and generic desktop notifications use the same normal
  notification priority.
- Non-macOS notification behavior is unchanged.
- macOS notification permissions are requested before the runtime thread starts,
  preserving the existing ordering where notification setup happens before any
  pre-break notification can be emitted.

## Commands

- `make check`
- `make -B macos-app-build`
- `plutil -lint target/macos/RustEyes.app/Contents/Info.plist`
- `codesign -dv target/macos/RustEyes.app`
- `codesign -d --entitlements - target/macos/RustEyes.app`
- `timeout 3s make run`

## Follow-up

- Users who need notifications during Focus/Do Not Disturb should add RustEyes
  to the relevant macOS Focus allowed-apps list.
