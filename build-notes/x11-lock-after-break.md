# x11-lock-after-break

## Goal

- Invoke the local Linux lock command after configured autolock break types
  finish.

## Changes

- Added a `lock.command` YAML setting that is parsed as an argv list.
- The default lock command is `["loginctl", "lock-session"]`.
- Config validation now rejects an empty lock command and a blank lock command
  program.
- Linux/X11 startup passes the validated lock command into the activity
  backend.
- The X11 backend handles `BackendCommand::RequestLock` by starting the
  configured command with `std::process::Command` and no shell.
- Started lock commands are supervised on a background thread so foreground
  lockers can keep running without blocking X11 activity polling.
- Lock command stdout is logged at trace level, stderr is mirrored to
  Resteyes' stderr and trace logging, and the final exit status is logged at
  trace level.
- Lock command startup failures are treated as backend errors because the lock
  request definitely did not start.
- Added tests for default and YAML lock command config, invalid lock config,
  X11 backend argv command construction, and lock command spawn failures.

## Decisions

- Use `loginctl lock-session` as the default because v1 targets NixOS/systemd.
- Keep the command configurable now so non-systemd desktops can provide their
  own locker without changing code.
- Do not add a timeout around the lock command because valid foreground lockers
  can stay running until the screen is unlocked.
- Do not add production dependencies.
- Do not invoke a real locker from tests.

## Commands

- `make test`
- `make check`

## Follow-up

- Manual autolock verification on a real X11 session is still pending because
  this environment does not provide usable X server access.
