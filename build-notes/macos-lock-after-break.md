# macos-lock-after-break

## Summary

- macOS production runs now honor lock-after-break intent after autolock or
  runtime lock requests.
- The default macOS lock path is helper-native: the Swift helper clears the
  break overlay and input event tap, then calls `SACLockScreenImmediate` from
  `/System/Library/PrivateFrameworks/login.framework/Versions/Current/login`
  through `dlopen`/`dlsym`.
- `lock.command` is now an optional override. Omitted or null config uses the
  platform default; an explicit argv list uses the shared no-shell command
  runner.
- Linux/X11 now owns its own default `loginctl lock-session` command when no
  override is configured.

## Decisions

- Use the private `SACLockScreenImmediate` API for the macOS platform default
  because a local smoke test resolved and invoked it successfully.
- Treat missing private framework or symbol lookup failure as a structured
  helper command error because the requested lock did not happen.
- Do not fall back to `CGSession -suspend`; users can opt into that with an
  explicit `lock.command` override if they want that behavior.
- Keep macOS remaining-time display and lock-after-break overlay controls in
  the later `macos-ui-improvements` step.

## Verification

- A temporary Swift smoke test loaded `SACLockScreenImmediate` from
  `login.framework` and successfully locked the session when invoked.
- `make test` passed after changing config shape, shared command locking, and
  macOS lock selection.
- `make macos-helper-build` passed after adding helper-native locking.
- `make check` passed before completing the step.

## Follow-up

- Manual end-to-end autolock verification through a full RustEyes break cycle
  is still pending.
- Continue with `macos-ui-improvements`.
