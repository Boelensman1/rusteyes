# core-layout

## Goal

- Introduce an internal library/module layout while keeping the program runnable.

## Changes

- Added `src/lib.rs` as the crate-level application entry.
- Added `src/runtime.rs` to own current startup behavior.
- Kept `src/main.rs` minimal by calling `resteyes::run()`.
- Later cleanup narrowed the public Rust API to `resteyes::run()` plus an
  application-level error wrapper; config and runtime internals remain
  crate-internal.

## Decisions

- No new dependencies were added.
- No config, scheduler, backend, or platform-specific behavior was introduced.
- The observable startup output remains `hello world`.
- Public callers should not depend on config module details while the internal
  config and scheduler shapes are still evolving.

## Commands

- `make check`
- `make run`
- `make check` after internal API cleanup

## Follow-up

- Continue with `config-schema` as the next build step.
