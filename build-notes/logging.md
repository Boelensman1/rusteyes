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
  - Backend-agnostic idle duration and active/idle classification for each
    regular activity sample.
  - Queued runtime events, including wall-clock and active-time events.
  - X11 overlay-period samples and whether idle break time advanced.
- Shared regular activity sample diagnostics now live in the crate-internal
  activity module so X11 and macOS emit the same `sampled activity` trace event.
- The binary logger writes tracing events through fresh `/dev/stderr` handles
  instead of the global Rust stderr writer; this avoids the macOS runtime
  blocking while formatting the first activity trace event.

## Decisions

- Use `tracing` instead of `log` because the daemon will benefit from
  structured fields and future span-based diagnostics.
- Keep activity diagnostics at `trace` level because they run on every poll and
  would be too noisy for default or info-level output.
- Keep platform names out of regular activity sample log messages. Backend
  details belong in backend-specific traces such as X11 overlay diagnostics.

## Commands

- `make check` passed.
- `RUST_LOG=trace` macOS smoke runs now print shared activity traces.

## Follow-up

- Manual X11 trace-output verification still needs a usable X11 session.
- Continue with `x11-ui-improvements`.
