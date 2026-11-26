# macos-helper-restart

## Goal

A failed macOS helper (read timeout, broken pipe, protocol error) used to
emit `RuntimeEvent::Shutdown`, which ends the whole RustEyes runtime. The
user's log shows this happened on 2026-09-16 and 2026-10-01: the helper
answered more than 5s late, and RustEyes quit silently. Restart the helper
instead, and keep the break the runtime believes is active.

## Decisions

- `MacOSHelperCore` records the first helper failure (`record_failure`)
  instead of queueing `Shutdown`. After each command or sample,
  `MacOSHelperBackend::run_actor` calls `restart_helper` if a failure is
  recorded.
- `restart_helper` kills the old helper outright, with no clean shutdown
  attempt: it is already suspect, and killing it releases its input event tap
  immediately. It then starts a new one through `start_helper` (spawn,
  stderr mirror, handshake, permission preflight; shared with `connect`).
- Restart budget: 3 starts within 10 minutes, retried 1s apart. Past that
  the backend flushes queued events and sends `Shutdown` as before. This way
  a deterministic failure such as missing permissions cannot loop forever.
- Active break across a restart (`resume_after_restart_at`): if the break
  still has time left and is not deferred, the new helper gets `startBreak`
  with the remaining time, the original name, origin and latest message
  (`active_break_spec`, updated on replace), and the current lock-after flag.
  The backend's wall-clock deadline is unchanged. A break that already ended,
  or is waiting for unlock, is not shown again; the next sample finishes it.
- Gaps closed because failures no longer end the process:
  - `start_break` records the active break before sending, so a failed
    `startBreak` is shown on the restarted helper. Before, the runtime would
    have waited for a break the backend never tracked.
  - `clear_break` forgets the break even if the helper command fails, so a
    restart does not show a cleared break.
  - Overlay samples queue `WallClockElapsed`/`BreakFinished` before
    returning a failed `finishBreak`/`updateBreak` result, so a finished
    break still reaches the runtime.
- No runtime or protocol changes; the restart is invisible to the runtime.

## Verification

- `src/macos_helper.rs` is `#[cfg(target_os = "macos")]`, so `make check` on
  Linux does not compile it. That includes the INFO logs added in
  `overlay-stuck-recovery`, which were unverified until this step. Verified
  here by:
  - `cargo +1.96.0 clippy --target aarch64-apple-darwin --all-targets -- -D warnings`
    through rustup in the scratchpad. A stub `CC_aarch64_apple_darwin` lets
    `mac-notification-sys`'s Objective-C build script pass, since nothing is
    linked. Clean.
  - Temporarily un-gating the module for `cfg(test)` on Linux and running
    `cargo test --lib macos_helper`: 40 passed (5 new). Reverted afterwards.
  - `make check` passes (319 tests, Linux).
- New tests: `failed_start_break_is_recorded_and_shown_again_after_restart`,
  `restart_resume_keeps_replaced_message_and_lock_request`,
  `restart_does_not_show_an_ended_or_deferred_break`,
  `failed_helper_finish_still_reports_break_finished`,
  `failed_clear_break_still_forgets_the_break`.
- Process-level restart (kill, respawn, handshake) is not unit-tested and
  needs checking on a Mac: during a long break, `kill -STOP` the helper. After
  ~5s the log should show `macOS helper failed; restarting it`, then
  `restarted macOS helper`. Input should return briefly, then the overlay
  reappear with the remaining time, and RustEyes keep running.

## Follow-up

- Consider making the macOS-target clippy run and the un-gated test run a
  Make target, so macOS Rust code is checked from Linux.
