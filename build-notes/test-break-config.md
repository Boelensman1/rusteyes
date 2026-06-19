# test-break-config

## Goal

- Add a short local config for manually exercising scheduled breaks.

## Changes

- Added `test-configs/ten-second-break.yaml`, which starts a 10 second `test`
  break after 10 seconds of active time.

## Decisions

- Kept the config to one break type with `interval: 1` so the first active-time
  slot starts the break.
- Left `autolock` omitted so it defaults to false.

## Commands

- `make check`
- `RESTEYES_CONFIG=test-configs/ten-second-break.yaml make run` manually
  exercises the short break cycle in a real X11 session.
