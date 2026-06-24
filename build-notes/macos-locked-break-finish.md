# macos-locked-break-finish

## Summary

- Manual reproduction showed a macOS break overlay could remain visible when a
  lock-after-break countdown finished while the session was locked.
- Helper protocol version 9 adds `sessionLocked` to `activitySample` using
  `CGSessionCopyCurrentDictionary`.
- When a macOS break reaches zero while the session is locked, RustEyes now
  keeps the active break in a deferred-finish state and waits for an unlocked
  sample before sending `finishBreak` and queueing `BreakFinished`.
- Deferred finishes suppress a second lock request after unlock because the
  session was already locked when the break completed.

## Decisions

- Keep countdown semantics while locked rather than treating the lock event as
  immediate break completion.
- Do not send a zero-duration update while locked; finishing is held in Rust
  state until unlock so helper cleanup happens in an unlocked UI session.
- Remove the temporary diagnostics, force-clear trigger, and overlay force-exit
  button after confirming the locked-session finish path fixed the issue.

## Verification

- `nix develop --command cargo test --lib macos_helper` passes (30 tests).
- `make check` passes (281 tests).
- `make -B macos-helper-build` passes.

## Follow-up

- Re-test manually by requesting lock-after-break, locking macOS before the
  timer ends, waiting past the end, and unlocking. Expected result: overlay
  clears on the first unlocked poll and RustEyes does not immediately lock the
  session again.
