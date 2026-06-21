# notification-tray-ui

## Goal

- Add minimal tray/menu-bar controls and pre-break notifications.

## Decisions

- Use `tray-icon` with a `tao` event loop for the tray/menu-bar surface.
- Keep the UI event loop on the main thread so macOS tray creation follows
  `tray-icon`'s event-loop requirements.
- Move the existing daemon runtime loop to a named runtime thread once UI is
  enabled.
- Route tray/menu controls into the runtime as local `RuntimeEvent` values so
  existing manual-break and disable sync broadcasting stays shared.
- Use a fixed pre-break notification lead of `min(30s, breaks.after_active / 2)`
  for this increment instead of adding new config.
- Use `notify-rust` for passive desktop notifications.
- On macOS, configure the `tao` event loop as an accessory app and hide Dock
  visibility before the loop runs so Resteyes is menu-bar/tray only.
- Follow-up refinement: show the scheduler's accumulated active time as a
  disabled status row in the tray/menu-bar dropdown.
- Follow-up fix: local macOS runs now build and register
  `target/macos/Resteyes.app` with bundle id `dev.resteyes.Resteyes`, and
  `make run` launches the bundled binary so notification permissions and
  attribution use Resteyes instead of `notify-rust`'s default app lookup.
- The Rust UI configures `notify-rust` with the Resteyes bundle id before the
  runtime thread starts, so no pre-break notification can race ahead of macOS
  notification app setup.
- The macOS helper path keeps `RESTEYES_MACOS_HELPER` as the highest-precedence
  override, then prefers the helper copied into the app bundle, then falls back
  to the existing SwiftPM debug helper.

## Behavior

- The tray/menu-bar has controls to start each configured break type, disable
  for each configured preset, disable until restart, and quit.
- Runtime sends one pre-break notification command when active time enters the
  notification window for the next scheduled break.
- Pre-break notifications are not emitted while scheduling is disabled or while
  a break is pending.
- UI-originated manual breaks and disable controls use the same local runtime
  paths as backend-originated controls and can be synced to authenticated peers.
- The active-time row starts at `0s`, updates when local or synced active-time
  increments change scheduler accumulation, and resets when breaks or disable
  controls reset scheduler active time.
- Manual break controls are ordered by scheduled cadence from shortest to
  longest, using each break type's slot interval.
- On macOS, pre-break notifications use the Resteyes app bundle identity rather
  than the `use_default` placeholder lookup from the default
  `mac-notification-sys` path.

## Commands

- `make check`
- `make -B macos-app-build`
- `plutil -lint target/macos/Resteyes.app/Contents/Info.plist`
- `timeout 3s make run` reached the bundled app path and stopped at the
  existing macOS privacy preflight because Accessibility/Input Monitoring are
  not granted in this environment.

## Follow-up

- Manual Linux tray verification is pending.
- Manual macOS notification verification is pending after granting
  Accessibility/Input Monitoring to Resteyes.
