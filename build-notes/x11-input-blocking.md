# x11-input-blocking

## Goal

- Block keyboard and pointer input from reaching other X11 clients while a
  break overlay is visible.

## Changes

- Overlay windows now select grabbed keyboard, button, and pointer-motion
  events in addition to expose and structure events.
- `X11Overlay::show` acquires active core X11 pointer and keyboard grabs after
  overlay windows are mapped, drawn, and raised.
- Pointer grabs use async pointer/keyboard modes, no pointer confinement, and no
  cursor override, so the pointer can still move while underlying clients do not
  receive pointer events.
- Keyboard grabs also use async modes and route key events to the overlay
  client.
- Overlay cleanup releases keyboard and pointer grabs before destroying overlay
  windows and graphics contexts.
- Overlay setup failures after window creation now destroy any already-created
  overlay resources before returning the error.
- Follow-up fix: `ALREADY_GRABBED` and `FROZEN` pointer/keyboard grab replies
  are now treated as transient contention. Break startup retries for a bounded
  window, keeps the daemon alive if contention persists, and never shows an
  unblocked overlay.

## Decisions

- Use core X11 grabs for this increment; XInput2 and Wayland support remain out
  of scope.
- Treat permanent grab failures as overlay startup failures instead of showing
  an unblocked break; transient grab contention is retried and then skips that
  break if it does not clear.
- Keep the runtime/backend command boundary unchanged.

## Commands

- `make check`
- `make test` after adding bounded retry handling for transient grab contention.
- `make check` after adding bounded retry handling for transient grab
  contention.

## Follow-up

- Manual X11 verification is still pending because this environment does not
  provide usable X server access.
