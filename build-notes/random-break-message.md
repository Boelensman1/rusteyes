# random-break-message

## Goal

When a break type is configured with several messages, show a randomly chosen
one each break instead of always the first.

## Problem

Both backends hardcoded "first message" selection:

- X11 `selected_break_message()` returned `messages.first()`.
- The macOS path shipped the whole `messages` list to the Swift helper, which
  picked the first non-empty entry.

## Decision

Centralize selection on the Rust side, since `ScheduledBreak` is the value both
backends consume and the crate already depends on `getrandom`.

- Added `ScheduledBreak::random_message()` plus a pure, testable
  `ScheduledBreak::message_at(index)` in `src/scheduler.rs`, alongside a shared
  `DEFAULT_BREAK_MESSAGE` constant (moved out of `src/x11_overlay.rs`).
- `random_message()` draws 8 bytes via `getrandom::fill`, reduces them modulo
  the message count, and falls back to index 0 / `DEFAULT_BREAK_MESSAGE` if the
  RNG fails or the list is empty. Modulo bias is negligible for a handful of
  messages.
- The message is picked once per break (X11 `show()`; macOS at command
  framing), not on every redraw.

## Wire protocol change

The macOS helper protocol now carries a single chosen `message: String` instead
of a `messages` list:

- `WireBreak` in `src/macos_helper.rs` (`messages` -> `message`, set from
  `random_message()`).
- Swift `BreakInfo` decodes `message`; `breakOverlayState(from:)` uses it
  directly, falling back to `defaultBreakMessage` when empty/whitespace.

The protocol is internal (app and helper ship together), so no compatibility
shim was needed.

## Bonus fix

`RuntimeEvent::BreakStartFailed` is only constructed by the X11 backend, so it
tripped `-D warnings` dead-code on non-Linux hosts. Annotated the variant with
`#[cfg_attr(not(target_os = "linux"), allow(dead_code))]`; it remains part of
the cross-platform `RuntimeEvent` contract (matched in `runtime.rs`).

## Tests

- Deterministic `message_at` tests in `src/scheduler/tests.rs` (in-range,
  wrap-around, empty -> default) plus a `random_message` membership test.
- Removed the obsolete `first_configured_break_message_is_selected` X11 test.
- Updated the macOS framing test to expect the single `message` field.

## Verification

- `make check` passes (fmt, clippy `-D warnings`, 266 tests) on macOS.
- `swift build` of `helpers/macos-helper` compiles.
- Manual: configure multiple messages, trigger several breaks, confirm variety
  (still pending in this environment).
