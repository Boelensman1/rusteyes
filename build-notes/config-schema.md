# config-schema

## Goal

- Add typed configuration defaults and validation without loading config files.

## Changes

- Added a public `config` module with typed defaults for break scheduling,
  break duration, break messages, disable presets, and autolock flags.
- Added validation for zero active durations, zero break durations, empty break
  message lists, blank break messages, empty disable presets, zero disable
  presets, and duplicate disable presets.
- Kept startup behavior unchanged; the runtime still prints `hello world`.

## Decisions

- No production dependencies were added.
- YAML loading is deferred to `yaml-config-loading`.
- Autolock settings are typed as per-break booleans for now;
  platform-specific lock commands remain out of scope.
- Break configs include an `after_active` duration so the scheduler can use the
  same schema in the next step.

## Commands

- `make check`
- `make run`

## Follow-up

- Continue with `scheduler-short-breaks`.
