# macos-login-item

## Goal

- Let macOS builds manage RustEyes as a user Login Item from config.

## Changes

- Added `startup.open_at_login` as an optional config key:
  - `true` asks macOS to launch RustEyes at login.
  - `false` unregisters RustEyes from login.
  - `null` or omission leaves the current macOS state unchanged.
- Added a macOS-only `smappservice-rs` integration that uses
  `SMAppService.mainApp` through `AppService::new(ServiceType::MainApp)`.
- Startup registration is best-effort: failures are logged and RustEyes keeps
  running.
- LaunchAgent plist and `KeepAlive` behavior remain out of scope for now.

## Decisions

- Use a Login Item instead of a LaunchAgent because RustEyes is currently a
  menu-bar GUI app with a user-visible quit action, notifications, overlays,
  and user-session privacy permissions.
- Keep the config tri-state so generated default config files do not
  automatically enable or disable a user's existing Login Item choice.
- Use `smappservice-rs` instead of a Swift sidecar or direct Objective-C bridge.

## Commands

- `make check`
- `make -B macos-app-build`
- `plutil -lint target/macos/RustEyes.app/Contents/Info.plist`
- `codesign -dv target/macos/RustEyes.app`

## Follow-up

- Manual Login Item registration verification is pending on macOS.
