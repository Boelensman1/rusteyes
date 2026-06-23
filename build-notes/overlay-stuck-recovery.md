# overlay-stuck-recovery

## Goal

Make it impossible for the macOS break overlay (and its input-blocking
`CGEventTap`) to outlive a break. A user hit a state where the countdown showed
`0:00` but the overlay never disappeared; because the overlay swallows all
keyboard/mouse input, the machine was fully locked and had to be rebooted. Add
layered defensive checks so no single failure can leave input permanently
blocked.

## Root causes investigated

Two independent ways the overlay can get stuck at `0:00`:

1. **Runtime could drop a break without clearing the overlay.**
   `src/runtime.rs::finish_break()` always `take()`s `current_break` but only
   sent `BackendCommand::FinishBreak` when `scheduler.finish_break()` returned
   `true`. `src/scheduler.rs::finish_break()` returns `true` only in the
   `Pending` state. If the scheduler ever left `Pending` while the overlay was
   still up, the runtime forgot the break and never told the helper to clear it.
   (Note: I could not construct a sequence through the public event API where
   `current_break == Some` while the scheduler is not `Pending` — the two are set
   and cleared together today — so this is invariant-hardening, not a proven
   reproduction.)
2. **Wedged IPC with no recovery (the more likely real culprit).**
   `HelperSession::receive()` did a blocking `read_line` with no timeout. A
   wedged/slow helper blocked the backend actor thread forever, so
   `MacOSHelperBackend::drop()` (which shuts down / kills the helper, and the OS
   then removes the tap) never ran. Nothing else could remove the tap.

There was also **no watchdog** anywhere: nothing independently guaranteed the
input tap was ever released once a break started.

## Decisions

- **No emergency keyboard escape hatch** (user decision): breaks stay
  unskippable; recovery is automatic only.
- The **Swift watchdogs are the real guarantee** that input is never permanently
  blocked. The runtime fix is sound hardening but may not be the literal trigger
  of the incident.
- Reader-thread + `recv_timeout` chosen for the Rust read timeout over
  `poll(2)`/raw-fd: cross-platform and no new dependency (`flume` already used).

## Changes

- **`src/runtime.rs::finish_break()`** — gate the overlay teardown on
  `current_break` being `Some`, not on `scheduler.finish_break()`. Early-return
  when `current_break` is `None` so a duplicate `BreakFinished` cannot trigger a
  second finish (which would spuriously lock/beep). `scheduler.finish_break()` is
  still called (to reset scheduler state) but now only gates the active-time
  display update. `break_start_failed()` is intentionally left unchanged (the
  helper break never started there).
- **`helpers/macos-helper/.../main.swift`** — two `DispatchSourceTimer`
  watchdogs on `DispatchQueue.main`, owned by `BreakOverlayController` and only
  touched on the main thread. They live on the main run loop (which
  `RunLoop.main.run()` services) so they fire even when the protocol thread is
  blocked on a read, and their handlers call `overlay.clear()` directly (no
  `DispatchQueue.main.sync`, no re-entrancy).
  - **Watchdog A (zero-timer, 5s):** armed once on the first `updateBreak` with
    `remainingMs == 0`; not re-armed on the subsequent stream of zero updates
    (which would prevent it ever firing). Disarmed on finish/clear/new-break or a
    non-zero update. Targets the reported symptom directly.
  - **Watchdog B (heartbeat, 10s):** armed while an overlay is active, reset on
    every inbound message (`updateBreak` and the 500ms `pollActivity`). Catches
    "RustEyes alive but backend thread wedged in `receive()`, pipe open, no
    data," which Watchdog A would not (no zero update arrives). Does not falsely
    fire on a paused break because polling continues every 500ms.
  - `RUSTEYES_HELPER_WATCHDOG_MS` overrides both timeouts for manual testing.
    A watchdog firing logs to stderr (mirrored by the Rust side).
- **`src/macos_helper.rs` (`HelperSession`)** — line reads moved onto a
  dedicated reader thread that forwards each `Result<String, _>` over a `flume`
  channel; `receive()` uses `recv_timeout(HELPER_READ_TIMEOUT = 5s)`. A timeout
  returns an "unresponsive" error that propagates to `run_actor`, which emits
  `RuntimeEvent::Shutdown` and returns → `drop` → `shutdown_helper_process` →
  kill after the existing 2s wait. Killing the helper closes the pipe and the OS
  removes the tap. The struct is now generic only over the writer
  (`HelperSession<W>`); the reader is owned by the thread. Cursor-based unit
  tests are unaffected (lines flow through the same channel with the default
  timeout).

## Coverage of the failure modes

- Runtime forgets the break → fixed at the source (change 1).
- Helper wedged but Rust alive, no zero update → Watchdog B clears.
- Stuck at `0:00`, healthy helper, finish never sent → Watchdog A clears.
- Helper wedged so the Rust read blocks forever → read timeout kills the helper;
  OS removes the tap. This is the only layer that also covers a wedged Swift
  **main** thread (where the watchdogs themselves cannot fire).

## Tests

- `src/runtime/tests.rs::duplicate_break_finished_finishes_overlay_exactly_once`
  — two `BreakFinished` events yield exactly one `FinishBreak`.
- `src/macos_helper.rs::tests::receive_reports_timeout_when_helper_is_silent` —
  a reader that blocks (pipe open, no data) makes `receive()` return the
  "did not respond" timeout error within a short deadline.
- Existing `break_start_failure_skips_pending_break_without_finishing_backend_break`
  still passes (no `FinishBreak` on a failed start).

## Verification

- `make check` passes (fmt-check, clippy `-D warnings`, 268 tests).
- Swift helper builds (`swift build`).
- Manual macOS verification pending on a machine with Accessibility/Input
  Monitoring granted:
  - Normal short break: overlay shows, counts to `0:00`, disappears with the
    finish beep, no early watchdog firing (check stderr).
  - Stuck-at-zero: with `RUSTEYES_HELPER_WATCHDOG_MS` short, force the finish to
    be skipped and confirm Watchdog A clears the overlay and restores input.
  - Wedge: `kill -STOP <helper pid>` mid-break and confirm the Rust read timeout
    fires, the helper is killed, and input returns within the deadline + 2s.
  - Paused break: stay active during a break and confirm neither watchdog fires.

## Follow-up

- Consider surfacing watchdog firings as a user-visible notification so a
  recovered stuck overlay is diagnosable without reading stderr.
