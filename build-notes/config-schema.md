# config-schema

## Goal

- Add typed configuration defaults and validation without loading config files.

## Changes

- Added a public `config` module with typed defaults for break scheduling,
  break duration, break messages, disable presets, and autolock flags.
- Added validation for zero active durations, zero break durations, empty break
  message lists, blank break messages, empty disable presets, zero disable
  presets, and duplicate disable presets.
- Later corrected long-break scheduling config so long breaks are expressed as
  `after_short_breaks`, can be disabled with `null`, and do not have an
  independent `after_active` duration.
- Long breaks now autolock by default; short breaks do not.
- Kept startup behavior unchanged; the runtime still prints `hello world`.

## Decisions

- No production dependencies were added.
- YAML loading is deferred to `yaml-config-loading`.
- Autolock settings remain typed as per-short/long booleans for now;
  platform-specific lock commands remain out of scope.
- Short breaks define the active-time cadence. Long breaks replace a short-break
  slot after the configured number of completed short breaks, avoiding separate
  timer/coalescing policy.

## Commands

- `make check`
- `make run`
- `cargo test` after long-break config correction
- `make check` after long-break config correction

## Follow-up

- Continue with `scheduler-short-breaks`.
