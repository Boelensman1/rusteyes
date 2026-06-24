# log-stream-routing

## Problem

Under launchd (and systemd), every tracing event landed in `err.log` and
`out.log` stayed empty, and the files contained raw ANSI color escapes
(`\e[2m`, `\e[32m`, …).

Both came from `init_logging()` in `src/main.rs`:

- The custom writer routed *all* levels to a dup of fd 2 (stderr). The fd-dup
  itself exists to dodge a macOS reentrant-lock deadlock when writing the first
  activity trace event through `std::io::stderr()`; stderr was never the intended
  destination for INFO.
- `tracing_subscriber::fmt()` defaults `with_ansi(true)` and does not auto-detect
  a TTY, so color codes were written literally into the log files.

## Change

Reworked `init_logging()` into a layered `tracing_subscriber::registry()` with
two filtered `fmt` layers:

- INFO/DEBUG/TRACE -> stdout (launchd `StandardOutPath` / journald).
- WARN/ERROR -> stderr, keeping `err.log` a clean "something is wrong" record.
- ANSI enabled only when the stream `is_terminal()`, so files/sockets stay plain
  while dev terminal runs keep colors.

The old `DevStderr`/`DevStderrWriter` pair was generalized into a single
`StdStream { Stdout, Stderr }` `MakeWriter` plus `StdStreamWriter`, dup-ing fd 1
or fd 2. The dup rationale comment (reentrant-lock avoidance; correct behavior
on journald sockets / append files / ttys / pipes; why reopening `/dev/std*` by
path is wrong) is preserved.

The global `EnvFilter` (default `warn`, `info` in production via `RUST_LOG`)
still governs overall verbosity; the per-layer `filter_fn`s only route what it
lets through, so default-level behavior is unchanged.

## Decisions / gotchas

- **Level ordering**: tracing's `Level` orders TRACE > DEBUG > INFO > WARN >
  ERROR. So stdout uses `*level >= Level::INFO` and stderr uses
  `*level <= Level::WARN` — disjoint and complete, no event dropped or
  duplicated.
- **No dependency change**: `tracing-subscriber`'s default features
  (`registry`, `fmt`, `ansi`) plus the existing `env-filter` already provide the
  layered API and per-layer filters.
- **No nix/plist change**: `StandardOutPath`/`StandardErrorPath` already exist;
  `out.log` simply starts receiving INFO once the binary is rebuilt and
  redeployed.

## Verification

- `make check` passes (fmt-check, lint, 275 tests, build).
- Redirect run (mirrors launchd):
  `RUST_LOG=info RUSTEYES_SYNC_SHARED_SECRET_FILE=<file> rusteyes >out 2>err`
  -> `out` has the two startup INFO lines, plain; `err` has the macOS
  notification-permission WARN, plain. No ANSI in either.
- pty run (`script`) -> stdout retains ANSI colors.

## Follow-up

- Redeploy the rebuilt binary under launchd to confirm live: new INFO lines in
  `out.log`, `err.log` collecting only warnings/errors.
