# service-log-delivery

## Goal

- Make RustEyes' `tracing` log output actually reach the destination a service
  manager attaches to stderr, so the `logLevel`/`RUST_LOG` option has a visible
  effect when running under systemd (Linux) and launchd (macOS), not only in an
  interactive terminal.

## Problem

- The binary logger wrote events by reopening `/dev/stderr` per event
  (`DevStderrWriter::new`), chosen to bypass the global Rust stderr lock that
  blocked the macOS runtime while formatting the first activity trace event
  (see `logging`). When the open failed, the writer silently discarded output
  (`file: None` -> `write` returns `Ok(len)`).
- Reopening `/dev/stderr` (i.e. `/proc/self/fd/2`) only behaves when fd 2 is a
  tty or pipe. Under a service manager it breaks:
  - systemd wires stderr to the journald **socket**; opening it fails with
    `ENXIO`, so every `tracing` event was silently dropped. Only the fatal
    `eprintln!` on exit (which uses the inherited fd 2 directly) reached the
    journal. `logLevel`/`RUST_LOG` had no observable effect.
  - launchd (with `StandardErrorPath`) and any plain redirect point fd 2 at a
    **regular file**; reopening it starts a fresh open description at offset 0,
    so each event overwrote the previous line — garbled, lossy logs.
- Verified empirically: `open("/dev/stderr", O_WRONLY)` returns `ENXIO`
  (errno 6) under `systemd-run --user -p StandardError=journal`, while a direct
  write to fd 2 reaches the journal.

## Changes

- `src/main.rs`: `DevStderrWriter::new` now writes through a **dup of the
  inherited stderr (fd 2)** instead of reopening `/dev/stderr`:
  `std::io::stderr().as_fd().try_clone_to_owned().ok().map(File::from)`.
- Dropped the now-unused `OpenOptions` import; added `std::os::fd::AsFd`.

## Decisions

- Dup of fd 2 over reopening `/dev/stderr`: a dup shares fd 2's open file
  description (offset and append flag), so it writes correctly to every
  destination a service manager attaches — journald socket, append file
  (launchd `StandardErrorPath`), tty, or pipe. It still bypasses the global
  Rust stderr lock (the original macOS-deadlock reason), because writes go
  through our own `File`, not `std::io::stderr()`'s locked handle.
- Pure std (`BorrowedFd::try_clone_to_owned`) instead of `libc::dup`, to avoid
  adding a production dependency. `as_fd()` only borrows the descriptor; it does
  not take the stderr lock.
- Keep the per-event handle creation (a dup syscall per event) — same cost shape
  as the previous per-event `/dev/stderr` open, and it preserves the lock-free
  property.
- macOS launchd still needs `StandardErrorPath` set (done in
  `macos-launchagent`) so fd 2 points at a real file rather than `/dev/null`;
  the dup fix then writes to it correctly. The two changes are complementary.

## Commands

- `make build` — compiles.
- `cargo clippy --bin rusteyes` — no findings in `main.rs`. (`make lint` fails on
  pre-existing `src/x11_overlay` lints unrelated to this change.)
- Empirical verification of the three destination classes:
  - regular file: startup `INFO` and a later `WARN` both present, in order
    (no offset-0 overwrite);
  - pipe (non-reopenable stream, like the socket): both lines delivered;
  - `systemd-run --user -p StandardError=journal -p Environment=RUST_LOG=info`:
    `starting RustEyes version="0.1.0"` now appears in
    `journalctl --user`, where it was previously dropped entirely.

## Follow-up

- Manual macOS verification still pending (shared with `macos-launchagent`):
  confirm logs land in `~/Library/Logs/rusteyes.err.log` under the LaunchAgent.
