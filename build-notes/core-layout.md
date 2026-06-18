# core-layout

## Goal

- Introduce an internal library/module layout while keeping the program runnable.

## Changes

- Added `src/lib.rs` as the crate-level application entry.
- Added `src/runtime.rs` to own current startup behavior.
- Kept `src/main.rs` minimal by calling `resteyes::run()`.

## Decisions

- No new dependencies were added.
- No config, scheduler, backend, or platform-specific behavior was introduced.
- The observable startup output remains `hello world`.

## Commands

- `make check`
- `make run`

## Follow-up

- Continue with `config-schema` as the next build step.
