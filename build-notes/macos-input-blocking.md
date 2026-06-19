# macos-input-blocking

## Summary

- macOS break startup now creates a helper-owned Quartz session event tap before
  showing AppKit overlay windows.
- The event tap swallows normal keyboard, flags, mouse button, mouse movement,
  dragging, scrolling, and tablet pointer events while a break overlay is
  active.
- `finishBreak`, `clearBreak`, `shutdown`, and helper EOF clear overlay windows
  and disable the event tap through the existing overlay cleanup path.
- If macOS refuses event tap creation, the helper returns a structured protocol
  error and does not show an unblocked overlay.

## Decisions

- Kept helper protocol version 2 and the Rust backend command boundary
  unchanged.
- Used a `.cgSessionEventTap` inserted at the head of the session event stream
  with `.defaultTap` so events can be dropped instead of only observed.
- Treated event tap unavailability as a break startup failure rather than
  falling back to an unenforced break.
- Left macOS lock-after-break, remaining-time UI, overlay controls, launchd,
  and sync out of this step.

## Verification

- `make macos-helper-build` passed.
- `make check` passed.
- A helper protocol smoke test returned `ready`, a structured event-tap
  permission error for `startBreak`, and `shutdownComplete`, confirming the
  helper does not show an unblocked overlay when required macOS permissions are
  unavailable.

## Follow-up

- Manually verify input blocking on macOS after granting the required
  Accessibility/Input Monitoring permissions.
- Continue with `macos-lock-after-break`.
