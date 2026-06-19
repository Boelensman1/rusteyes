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
- The X11 backend handles `BackendCommand::RequestLock` by spawning the
  configured command with `std::process::Command` and no shell.
- Lock command startup or non-zero exit failures are treated as backend errors:
  the error is printed and the daemon queues shutdown.
- Added tests for default and YAML lock command config, invalid lock config,
  and X11 backend argv command construction.

## Decisions

- Use `loginctl lock-session` as the default because v1 targets NixOS/systemd.
- Keep the command configurable now so non-systemd desktops can provide their
  own locker without changing code.
- Do not add production dependencies.
- Do not invoke a real locker from tests.

## Commands

- `make test`
- `make check`

## Follow-up

- Manual autolock verification on a real X11 session is still pending because
  this environment does not provide usable X server access.
