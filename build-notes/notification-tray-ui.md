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

## Behavior

- The tray/menu-bar has controls to start each configured break type, disable
  for each configured preset, disable until restart, and quit.
- Runtime sends one pre-break notification command when active time enters the
  notification window for the next scheduled break.
- Pre-break notifications are not emitted while scheduling is disabled or while
  a break is pending.
- UI-originated manual breaks and disable controls use the same local runtime
  paths as backend-originated controls and can be synced to authenticated peers.

## Commands

- `make check`

## Follow-up

- Manual Linux/macOS tray and notification verification is pending.
