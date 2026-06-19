# config-schema

## Goal

- Add typed configuration defaults and validation without loading config files.

## Changes

- Added a crate-internal `config` module with typed defaults for shared break
  scheduling, named break types, break durations, break messages, disable
  presets, and per-break-type autolock flags.
- Added validation for zero active durations, empty break type maps, empty break
  type names, whitespace-padded break type names, zero break intervals,
  duplicate break intervals, zero break durations, empty break message lists,
  blank break messages, empty disable presets, zero disable presets, and
  duplicate disable presets.
- Represented breaks as `breaks.after_active` plus arbitrary named
  `breaks.types`. Default `short` and `long` break types preserve the previous
  defaults.
- The default `long` break type autolocks; the default `short` break type does
  not.
- Kept startup behavior unchanged; the runtime still prints `hello world`.

## Decisions

- No production dependencies were added.
- YAML loading is deferred to `yaml-config-loading`.
- Autolock is stored per break type; platform-specific lock commands remain out
  of scope.
- A shared active-time duration defines break slots. Each break type has an
  integer interval in slots, and the due break with the largest interval wins.
- Duplicate intervals are rejected so future scheduling has one unambiguous
  winner per slot.

## Commands

- `make check`
- `make run`
- `make check` after generic break type correction
- `make check` after config visibility cleanup

## Follow-up

- Continue with `scheduler-break-slots`.
