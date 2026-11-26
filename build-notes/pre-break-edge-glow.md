# pre-break-edge-glow

## Goal

- Make an upcoming break impossible to miss when desktop notifications are
  muted (macOS silences them during calls), without interrupting the user:
  a pulsing glow around the edges of every display during the existing
  pre-break notice window.

## Decisions

- The glow is a backend display surface, not a UI-thread notification. The
  runtime sends `BackendCommand::ShowPreBreakWarning { starts_after }` next to
  every `UiCommand::ShowPreBreakNotification`, and
  `BackendCommand::ClearPreBreakWarning` next to every
  `UiCommand::ClearPreBreakNotification`. This reuses the notification's
  cadence: the initial notice at `min(30s, after_active / 2)` and the final
  5-second notice. Clears already happen before `StartBreak`, and on disable,
  idle reset, and scheduler reset.
- Backends pick the pulse speed from `starts_after`: slow above 5 seconds,
  fast at or below 5 seconds. The final 5-second notice is what switches the
  glow to the fast pulse.
- Backends also drop the warning on `StartBreak`/`ClearBreak`/shutdown and
  ignore a show request while a break is active or pending. If no clear
  arrives, a safety expiry at deadline + 5 seconds removes it.
- The warning is cosmetic, so failures to show, pulse, or clear it are logged
  and never shut the backend down.
- It is always on alongside the notification. There is no config toggle yet;
  add one if the glow proves annoying.
- The pulsing edge glow was chosen over a gradual dim or a cursor halo because
  it is noticeable from the corner of the eye and covers no content.

## macOS

- Helper protocol version 9 -> 10, with new `showPreBreakWarning
  {startsAfterMs}` and `clearPreBreakWarning` commands, each answered with
  `commandComplete`.
- `PreBreakWarningController` in the Swift helper creates one borderless,
  transparent window per `NSScreen`, at `.screenSaver` level, with
  `ignoresMouseEvents = true` and no key/main activation
  (`orderFrontRegardless`). Windows use the same zero-origin + `setFrame`
  placement as the break overlay, so they cover secondary displays.
- `sharingType = .none` keeps the glow out of screen capture, so call
  participants don't see it during screen sharing. Unverified: newer
  ScreenCaptureKit-based capture may not honor this on every macOS version.
- The glow is a `CALayer` border (`systemOrange`, 10pt) whose shadow spreads
  inward. A repeating Core Animation opacity animation (1.6s period, 0.5s in
  the final 5s) runs it, so the helper needs no ticks from Rust.
- `BreakOverlayController.clear()` also clears the warning, so `startBreak`,
  `finishBreak`, `clearBreak`, `shutdown`, and the watchdogs all remove it.

## X11

- Enabled x11rb's `shape` feature. This is the same crate, not a new
  dependency.
- `X11PreBreakWarning` (`src/x11_overlay/pre_break_warning.rs`) creates four
  12px override-redirect edge strips per monitor. Each strip's input shape is
  set to an empty region, so clicks pass through. Strips never grab input.
- The pulse alternates the background pixel between bright and dim amber
  (TrueColor pixel values), repaints, and re-raises the strips: every 500 ms,
  or every 250 ms in the final 5s. This needs no compositor.
- The backend loop wakes for pulses while the warning is visible but still
  takes activity samples only at the 1s poll interval, because each sample
  reports a full poll interval of activity.

## Commands

- `make check` passes (323 tests).
- `cargo test --lib pre_break_warning` covers the edge-strip geometry, the
  pulse cadence, and clearing on disable. The pre-break notification runtime
  tests assert the warning command sequence.
- The Swift helper and the macOS-only Rust module (`src/macos_helper.rs`,
  including the new framing test) were not compiled: this step was done from
  a Linux container without the Darwin target or Swift.

## Pending verification

- macOS: run `make macos-helper-build`, then `make run` with
  `test-configs/forty-second-break.yaml`. Check that the glow appears on every
  display at about 20s and speeds up at 5s. Check that it disappears when the
  overlay starts, lets clicks through, doesn't take focus from a call app, and
  is absent from a screen-share preview.
- X11: check that the strips appear, pulse, are click-through, stay on top,
  and clear on break start.
