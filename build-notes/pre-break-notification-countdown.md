# pre-break-notification-countdown

## Goal

- Replace the one-shot pre-break desktop notification with an updating
  countdown notification that is removed when the break starts.

## Decisions

- Preserve the existing pre-break notice lead of
  `min(30s, breaks.after_active / 2)`.
- Use the actual remaining time for the first notification in the notice
  window, then update only when lower 5-second boundaries are crossed.
- Keep countdown state in the runtime so UI commands remain deterministic and
  testable.
- Store the active pre-break `notify-rust` notification handle in the UI loop,
  update it in place for later countdown commands, and close it on clear.
- Follow-up fix: render countdown notification durations at whole-second
  precision in the UI, rounding fractional values up so desktop notifications
  do not expose millisecond, microsecond, or nanosecond units.

## Behavior

- A normal 60-second active interval shows `30s`, `25s`, `20s`, `15s`, `10s`,
  and `5s` countdown notifications before clearing the notification as the
  break starts.
- Short active intervals keep the half-interval lead: a 10-second interval
  shows `5s`, and a 20-second interval shows `10s` and `5s`.
- Fractional countdown commands keep precise runtime durations internally, but
  notification text is human-sized; for example `24.633210875s` renders as
  `25s`.
- Generic desktop notifications are unchanged.
- The first appearance of the pre-break notification plays a sound so the
  upcoming break is noticeable without looking at the screen. The 5-second
  countdown updates stay silent (the sound is set only on the initial `.show()`,
  not on the in-place `.update()` path).
- If a user manually dismisses a countdown notification, a later countdown
  update may show it again because dismissal callbacks are not tracked.

## Notification sound

- Implemented via `notify-rust`'s `Notification::sound_name`, set only in the
  first-show branch of `show_pre_break_notification` in `src/ui.rs`; the shared
  `build_notification` helper is untouched so other notifications stay silent.
- The name vocabularies differ per platform, so `PRE_BREAK_NOTIFICATION_SOUND`
  is `cfg`-conditional: Linux/XDG uses `"message"` (freedesktop sound naming
  spec), macOS uses `"Ping"` (a `/System/Library/Sounds` system sound).
- The sound is played by the desktop's notification server (XDG `SoundName`
  hint on Linux, `NSUserNotification`/`UN` sound on macOS), so minimal or
  headless setups that ignore sound hints will not play it.
- macOS audible verification is pending a macOS session (same as the
  break-finished beep).

## Commands

- `cargo test pre_break_notification --lib`
- `make check`
- `make -B macos-app-build`
- `make check` passes after rounding countdown notification text to whole
  seconds.
- Checks were skipped when amending the countdown cadence from 10 seconds to 5
  seconds at user request.
