# macos-ui-improvements

## Summary

- macOS break overlays now show the configured break message, remaining break
  time, and a lock-after-break control on every helper-owned overlay window.
- The macOS helper protocol is version 6. It adds `updateBreak` for remaining
  time and lock-control state, and activity samples now report whether the
  overlay control requested lock-after-break.
- The Rust macOS backend keeps helper UI state in sync after each overlay
  activity poll and queues `LockAfterCurrentBreak` before `BreakFinished` when
  a click and break completion happen in the same poll.
- Follow-up fix: `finishBreak` and `updateBreak` now explicitly serialize
  `lockAfter` in the JSON wire format. The helper previously rejected
  `updateBreak` as an invalid protocol message because Rust emitted
  `lock_after`.
- Follow-up fix: the helper now forces the overlay cursor to the arrow cursor
  when a break starts and registers an arrow cursor rect for the full overlay
  view, so a pre-existing insertion cursor from another app does not remain
  visible during an input-blocking break.
- Follow-up cleanup: the Swift helper now decodes daemon commands and encodes
  helper responses through typed `Codable` protocol values instead of
  dictionary/string-key parsing.
- Follow-up cleanup: Rust now validates the helper `shutdownComplete` response
  during shutdown, computes break elapsed once per overlay sample, and queues
  macOS overlay runtime events in the same wall-clock, lock-request,
  break-finished order as X11.
- Follow-up cleanup: helper lock-control hit testing now names the Quartz event
  point conversion path explicitly while preserving the existing raw/flipped
  point fallback.

## Decisions

- Keep remaining-time drawing and lock-control hit testing inside the Swift
  helper because it owns the AppKit windows and Quartz event tap.
- Keep `updateBreak` as a helper protocol command instead of adding a shared
  backend command because this is macOS-helper UI synchronization, not a
  cross-platform runtime concept.
- Detect lock-control clicks from the event tap and continue dropping normal
  input, so the overlay remains input-blocking while still accepting this
  specific control.

## Verification

- `make test` passed after adding protocol version 6 framing, activity-sample
  lock-request decoding, active-break state, and runtime event ordering tests.
- `make macos-helper-build` passed after adding the AppKit overlay UI updates.
- `make check` passed before completing the step.
- `make test` passed after adding raw JSON assertions for `lockAfter` command
  fields.
- `make check` passed after fixing `lockAfter` command field serialization.
- `make macos-helper-build` passed after fixing the overlay cursor.
- `make check` passed after fixing the overlay cursor.
- `make macos-helper-build` passed after the typed helper protocol cleanup.
- `make check` passed after the Rust shutdown/event-order cleanup.

## Follow-up

- Manual macOS overlay UI verification with Accessibility/Input Monitoring
  permissions granted is still pending.
- Continue with `sync-protocol`.
