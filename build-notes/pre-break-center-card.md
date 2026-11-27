# pre-break-center-card

## Goal

- The pre-break desktop notification is still missed during calls (macOS
  mutes notifications then). Replace it with a large countdown card in the
  center of every display, shown with the existing edge glow.

## Decisions

- The card is part of the backend pre-break warning, not the UI thread.
  `BackendCommand::ShowPreBreakWarning` now carries `break_name` next to
  `starts_after`, so the existing cadence, clears, and safety expiry apply to
  the card too.
- The native pre-break notification is gone: `UiCommand::ShowPreBreakNotification`,
  `UiCommand::ClearPreBreakNotification`, `PreBreakNotification`, and the
  notification handle and sound in `src/ui.rs` were removed. Other desktop
  notifications (sync peer rejected) still use `notify-rust`.
- The notification's sound moved to the backends and still plays only on the
  first appearance: `NSSound` "Glass" on macOS, the X server bell on X11.
- The card is click-through, takes no focus, and is shown on every display.
  It counts down locally between runtime updates, so it needs no extra
  commands.
- Card text: "<Name> break in", a large `m:ss` countdown, and "Wrap up what
  you are doing".
- The runtime tests now assert the card's break name through the backend
  warning commands instead of UI notification commands.

## macOS

- Helper protocol version 10 -> 11: `showPreBreakWarning` gains `breakName`.
- `PreBreakWarningController` adds one 520x220 borderless window per screen,
  centered, at `.screenSaver` level, `ignoresMouseEvents`,
  `sharingType = .none` (kept out of screen shares, like the glow).
- `PreBreakCardView` is a dark rounded panel with an orange border and
  `NSTextField` labels; the countdown uses a 92pt monospaced-digit font. A
  0.25s main run loop `Timer` updates it from a wall-clock deadline that each
  `show` re-anchors.

## X11

- `src/x11_overlay/pre_break_card.rs` creates one 480x230 override-redirect
  window per monitor, centered (border included), with an empty input shape.
- Core X fonts are small and large ones are often not installed, so the
  countdown is drawn as seven-segment digits from filled rectangles. The title
  and hint use the first available core font from a short fallback list
  ending in `fixed`.
- The card is repainted and raised on every glow pulse (250-500 ms), which
  also updates the countdown and repairs exposed areas without event
  handling.

## Commands

- `make check` passes (334 tests).
- `nix shell nixpkgs#swift -c swiftc -parse` on the helper passes. That is a
  syntax check only: AppKit isn't available on Linux, so the helper and
  `src/macos_helper.rs` were not compiled.

## Pending verification

- macOS: `make macos-helper-build`, then `make run` with
  `test-configs/forty-second-break.yaml`. Check that the card appears centered
  on every display, counts down, plays the sound once, doesn't take focus
  from a call app, lets clicks through, is absent from a screen-share
  preview, and disappears when the break starts.
- X11: check the card's placement, fonts, countdown, click-through, the bell,
  and that it clears on break start.
