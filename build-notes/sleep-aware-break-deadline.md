# sleep-aware-break-deadline

## Goal

Make an active break finish on time when the machine sleeps through its
deadline. Before this fix, closing a MacBook mid-break and reopening it 10+
minutes later showed a 5-minute long break still running.

## Root cause

`BreakDeadline` stored an `Instant`. On macOS `Instant` is backed by
`mach_absolute_time`, which does not advance during system sleep; Linux
`CLOCK_MONOTONIC` likewise excludes suspend, so the X11 backend had the same
bug. Both backends' `ActiveBreak` timers evaluated a frozen deadline on each
500ms overlay tick, so `finished` never became true after a sleep. This is the
same bug class the timed-disable countdown had (fixed in "Fix timed disable
countdown after sleep" by moving to an absolute wall-clock deadline); the break
timers were left on `Instant` when "Use deadlines for break countdowns"
introduced them.

## Decisions

- `BreakDeadline` (`src/activity.rs`) now stores `ends_at: SystemTime` plus the
  break `duration`. `remaining_at` saturates to zero past the deadline and
  clamps to `duration`, so a backwards system-clock jump (manual change, large
  NTP step) can neither panic nor inflate the countdown; a forwards jump at
  worst ends the break early by the jump amount. DST shifts are irrelevant —
  `SystemTime` is UTC-based Unix time.
- Per-tick `elapsed` reporting (`RuntimeEvent::WallClockElapsed`) deliberately
  stays monotonic (`Instant`), reporting only awake time. The runtime already
  handles sleep for timed disables with its own absolute deadline, and a sudden
  10-minute elapsed event on wake could perturb status/pre-break accounting.
  New `ObservedTime { monotonic: Instant, wall: SystemTime }` in `activity.rs`
  carries both readings through the samplers; deadlines read `.wall`, elapsed
  reads `.monotonic`.
- Both backends changed identically: `src/macos_helper.rs` and
  `src/x11_activity.rs` `ActiveBreak::new/replace` anchor the deadline at
  wall-now, and `apply_sample`/`advance` split the two time sources.
  `next_sample_delay_at` uses the same wall clock as `finished`, so the loop
  cannot compute a stale delay against a passed deadline.
- No NSWorkspace sleep/wake observers: the backend actor's `flume` waits are at
  most 500ms of monotonic time, so the loop re-evaluates within ~1s of wake.
  Observers remain possible future work if 1s latency ever matters.
- The deferred-finish path (break expires while the session is locked — the
  typical wake-to-lock-screen case) needed no changes: the first post-wake
  sample computes `finished = true` from the wall clock and defers; the latched
  `DeferredFinish` finishes the break on unlock and cannot be reverted by later
  clock movement. Same for X11's `finish_reported` latch.

## Tests

- `src/activity/tests.rs`: existing `BreakDeadline` tests moved to constructed
  `SystemTime` values; new `break_deadline_finishes_after_wall_clock_sleep_jump`
  and `break_deadline_backwards_clock_jump_clamps_remaining`.
- `src/macos_helper.rs`: `Instant`-injecting tests converted to `ObservedTime`
  via `observed_start`/`observed_after`/`observed_after_sleep` helpers. New:
  `overlay_sample_after_sleep_finishes_break_immediately` (monotonic +1s, wall
  +10min → immediate `FinishBreak`, and `WallClockElapsed(1s)` pins that
  elapsed stays awake-time), `overlay_sample_after_sleep_while_locked_defers_finish_until_unlock`,
  and `backwards_clock_jump_does_not_finish_break_early` (clamped
  `UpdateBreak { remaining_ms: 300000 }`).
- X11 has no `ActiveBreak` unit tests (needs a live X connection); coverage
  comes from the shared `BreakDeadline` tests plus the compiler-enforced type
  change.

## Commands

- `make check` passes (fmt-check, clippy `-D warnings`, 328 tests).

## Follow-up

- Manual macOS verification: start a long break, close the lid past the break
  duration, reopen — the overlay should clear within ~1s of unlock.
