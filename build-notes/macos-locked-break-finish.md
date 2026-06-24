# macos-locked-break-finish

## Summary

- Reproduced diagnostics showed `finishBreak` was acknowledged and
  `SACLockScreenImmediate` returned, but RustEyes continued polling with helper
  overlay state `active=false windows=0` while the user still saw a stale
  lock-screen overlay.
- Helper protocol version 9 adds `sessionLocked` to `activitySample` using
  `CGSessionCopyCurrentDictionary`.
- When a macOS break reaches zero while the session is locked, RustEyes now
  keeps the active break in a deferred-finish state and waits for an unlocked
  sample before sending `finishBreak` and queueing `BreakFinished`.
- Deferred finishes suppress a second lock request after unlock because the
  session was already locked when the break completed.
- Diagnostics force-clear handling now polls lock state first, defers clear
  while locked, and can still send `clearBreak` even if RustEyes no longer has
  an active break.
- The helper tags overlay windows and clears both tracked windows and any
  remaining tagged app windows. The diagnostic `Force exit` button also has an
  AppKit mouse-down fallback in addition to the event-tap path.

## Decisions

- Keep countdown semantics while locked rather than treating the lock event as
  immediate break completion.
- Do not send a zero-duration update while locked; finishing is held in Rust
  state until unlock so helper cleanup happens in an unlocked UI session.
- Keep `/tmp/rusteyes-force-clear` temporary and diagnostics-gated.

## Verification

- `nix develop --command cargo test --lib macos_helper` passes (36 tests).
- `make check` passes (287 tests).
- `make -B macos-helper-build` passes.

## Follow-up

- Re-test manually by requesting lock-after-break, locking macOS before the
  timer ends, waiting past the end, and unlocking. Expected result: overlay
  clears on the first unlocked poll and RustEyes does not immediately lock the
  session again.
