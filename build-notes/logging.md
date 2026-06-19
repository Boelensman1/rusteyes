# logging

## Goal

- Add structured runtime logging after `x11-lock-after-break`.

## Implemented Changes

- Add `tracing` as the logging facade and `tracing-subscriber` as the binary
  logger setup.
- Initialize logging once from the binary entry point before calling
  `resteyes::run()`.
- Use `warn` as the default filter so normal runs stay quiet.
- Support the standard `RUST_LOG` override, such as
  `RUST_LOG=resteyes=trace make run`.
- Keep fatal startup errors visible on stderr even after logging is initialized.
- Replace internal backend diagnostic `eprintln!` calls with tracing events.
- Add trace-level activity diagnostics for high-frequency polling:
  - X11 idle duration and active/idle classification for each activity sample.
  - Queued runtime events, including wall-clock and active-time events.
  - Overlay-period samples and whether idle break time advanced.

## Decisions

- Use `tracing` instead of `log` because the daemon will benefit from
  structured fields and future span-based diagnostics.
- Keep activity diagnostics at `trace` level because they run on every poll and
  would be too noisy for default or info-level output.

## Commands

- `make check` passed.

## Follow-up

- Manual trace-output verification with `RUST_LOG=resteyes=trace make run`
  still needs a usable X11 session.
- Continue with `x11-ui-improvements`.
