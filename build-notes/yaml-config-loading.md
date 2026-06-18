# yaml-config-loading

## Goal

- Load YAML configuration from `RESTEYES_CONFIG` or the XDG config path.

## Changes

- Added YAML config loading that overlays partial file values onto typed
  defaults and validates the resulting config.
- Added `serde`, `serde-saphyr`, and `humantime` for typed YAML parsing and
  human-readable duration strings.
- Accepted duration values as quoted `humantime` strings such as `20s`, `20m`,
  `1h`, and `1h 30m`; bare integer seconds are rejected.
- Changed break text from a single `message` to `messages`, allowing multiple
  messages for future random selection.
- Moved `autolock` onto each break type to make automatic locking part of the
  generic break definition.
- Wired runtime startup through config loading while preserving the no-config
  `hello world` output.
- Later cleanup made config path resolution more explicit, replaced indexed
  path suffixes with named constants, and simplified empty YAML handling by
  applying a default partial config.
- Replaced fixed break keys with `breaks.after_active` plus a `breaks.types`
  map keyed by arbitrary break type names. Each break type defines `interval`,
  `duration`, `messages`, and optional `autolock`.
- When `breaks.types` is present in YAML, it replaces the default break type
  map so users can define the exact break type set they want.

## Decisions

- Missing implicit XDG config files fall back to defaults.
- A non-empty `RESTEYES_CONFIG` is explicit and must point to a readable, valid
  config file.
- Unknown YAML fields are rejected.
- Duration values are string-only to keep the YAML shape simple and explicit.
- Break cadence is slot-based rather than separate per-break timers: one shared
  active duration defines slots, and each break type has an integer interval.
- Random message selection is deferred to a later runtime/scheduler step.

## Commands

- `make check`
- `make run`
- `make check` after config cleanup
- `make check` after generic break type correction

## Follow-up

- Continue with `scheduler-break-slots`.
