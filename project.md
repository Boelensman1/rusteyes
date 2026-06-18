RestEyes is an as-simple-as-possible SafeEyes replacement focused on reliable break enforcement across the computers someone is actively using.

## Goal and MVP

- v1 targets NixOS running X11.
- macOS Tahoe support comes after the X11 implementation.
- Wayland support is deferred until the X11 and macOS designs are stable.
- The project should stay small: a core daemon, platform backends, local configuration, minimal UI, and authenticated peer sync.

## Core behavior

- Blank all connected displays during a break and show one configured break
  message.
- Strictly block normal keyboard and mouse input during a break.
- Release display overlays and input grabs if the daemon or backend exits unexpectedly.
- Support short and long breaks.
- Configure break duration and message text lists separately for short and long
  breaks.
- Track keyboard and mouse activity to decide when a break is due.
- Treat idle time as rest: it delays or satisfies break accumulation rather than forcing wall-clock breaks.
- Optionally autolock screens after a break.

## Network sync

- Use LAN discovery to find peers.
- Authenticate peer messages with a configured shared secret.
- Allow any authenticated peer to broadcast that a break is due.
- Apply break starts, disable/snooze periods, and lock-after-break decisions across authenticated peers.
- Each machine runs its own local lock mechanism when a synced lock-after-break decision applies.

## Architecture

- Rust daemon owns configuration, scheduling, network sync, and shared runtime state.
- Most platform backends are written in Rust.
- X11 backend is implemented first because it gives the core blanking, input capture, and activity behavior quickly.
- macOS uses a small Swift/AppKit/CoreGraphics helper for macOS-specific APIs, controlled by the Rust daemon over local IPC.
- Tray and notification UI should try a cross-platform Rust crate first, with platform-specific fallback if needed.
- Run as a per-user service: systemd user service on Linux and launchd agent on macOS.

## Configuration and UI

- Settings live in a YAML config file.
- UI is limited to:
  - break overlay with a configured break message
  - pre-break notification
  - system tray/menu-bar icon showing that the daemon is running
- Tray/menu actions:
  - quit
  - start a short break now
  - start a long break now
  - disable breaks for 30 minutes
  - disable breaks for 1 hour
  - disable breaks for 2 hours
  - disable breaks for 3 hours
  - disable breaks until restart
- Disable actions apply across authenticated synced peers.

## Build order

Initial implementation should proceed through the MVP path in small,
reviewable steps. Each step should leave the program runnable and have its own
step note when implementation begins.

1. `core-layout`: introduce internal modules/library structure while keeping
   `make run` working.
2. `config-schema`: add typed config defaults and validation for break
   durations, messages, disable presets, and autolock settings.
3. `yaml-config-loading`: load YAML from `RESTEYES_CONFIG` or the XDG config
   path, with clear parse and validation errors.
4. `scheduler-short-breaks`: implement deterministic short-break scheduling
   with injected time and activity inputs.
5. `scheduler-long-idle-disable`: add long breaks, idle-as-rest behavior, and
   local disable-until handling.
6. `daemon-runtime-noop`: wire config and scheduler into a daemon loop using a
   no-op backend so behavior is testable before X11.
7. `backend-trait`: define the internal platform interface for activity, break
   overlay, input blocking, notifications, tray actions, and local lock.
8. `x11-activity`: implement X11 keyboard/mouse activity and idle tracking.
9. `x11-overlay`: blank all connected displays and show the configured break
   message.
10. `x11-input-blocking`: grab/block normal input during breaks and release
    cleanly on break end or backend exit.
11. `x11-break-integration`: connect scheduler and X11 backend for scheduled
    and manual short/long breaks.
12. `sync-protocol`: define authenticated sync events for break start, disable
    periods, and lock-after-break decisions.
13. `lan-discovery`: discover authenticated peers on the LAN.
14. `break-disable-sync`: broadcast and apply break starts and disable periods
    across peers.
15. `notification-tray-ui`: add pre-break notifications and tray/menu actions.
16. `synced-lock-after-break`: apply synced lock-after-break decisions using
    the local Linux/X11 lock hook.

Deferred later work:

1. macOS Swift helper and launchd integration.
2. Wayland investigation.
