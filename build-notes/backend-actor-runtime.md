# backend-actor-runtime

## Goal

- Make platform backends actor-like so platform state stays on the backend
  thread while runtime selects over backend and sync transport events.

## Changes

- Replaced the synchronous backend trait with `BackendActor`, command-sender,
  runtime-event receiver, and actor startup/join ownership.
- X11 and macOS backend constructors now spawn backend actor threads and build
  X11 connections or macOS helper sessions inside those threads.
- X11 and macOS sampling loops now use `flume::Selector::wait_timeout` to wait
  for either a backend command or the next sample timeout.
- Runtime now selects over backend runtime events and sync transport events
  through one internal input path.
- Sync transport exposes a cloned event receiver for runtime selection.

## Decisions

- Prioritized a clean channel-based API over preserving the old backend trait.
- Sync transport events are selected and logged, but sync domain behavior is
  still deferred to the `active-time-sync` increment.
- Kept unrelated blocking waits, such as X11 lock handoff and macOS helper
  shutdown polling, unchanged.

## Commands

- `cargo check --all-targets --all-features`
- `cargo test --all-targets --all-features runtime`
- `cargo test --all-targets --all-features backend`
- `make fmt-check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `make check`

## Follow-up

- Continue with `active-time-sync`, mapping selected authenticated sync domain
  events into runtime behavior.
