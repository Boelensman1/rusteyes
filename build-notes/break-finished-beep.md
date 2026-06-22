# break-finished-beep

## Goal

Play a short beep when a break finishes, signalling that it is safe to resume
work.

## Decisions

- The beep is a backend responsibility, triggered on `BackendCommand::FinishBreak`,
  so it fires for both timer-completed and lock-after-break finishes and stays
  consistent with how breaks are already cleared.
- Linux/X11 (`src/x11_activity.rs`): ring the X server bell via
  `xproto::ConnectionExt::bell` on the existing connection. No new dependency,
  no audio assets. Volume is the server default (`BREAK_FINISHED_BELL_PERCENT =
  0`). The bell is best-effort: send/check failures are logged at `trace` and do
  not abort the finish path. It only rings when a break was actually active, and
  fires before the optional lock handoff so it is heard before the screen locks.
- macOS helper (`helpers/macos-helper/.../main.swift`): call `NSSound.beep()`
  (AppKit, already imported) from `handleFinishBreak`, dispatched on the main
  thread via the existing `runOnMain` helper, after clearing the overlay and
  before any lock.
- No configuration knob was added; "a little beep" is unconditional. A
  config toggle can be introduced later if needed.

## Verification

- `make check` passes (fmt-check, clippy `-D warnings`, 258 tests).
- The X11 bell was not audibly verified because this environment has no usable X
  server; manual audible verification on a real X session is pending.
- The macOS `NSSound.beep()` change was not built or run here (no macOS
  toolchain); manual verification on macOS is pending.

## Follow-up

- Optional `sound`/`beep` config toggle if users want to silence it.
- Consider whether the beep should be suppressed when `lock_after` is set.
