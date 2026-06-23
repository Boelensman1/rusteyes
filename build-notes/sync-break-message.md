# sync-break-message

## Goal

When two synced machines start the same break at nearly the same time, make them
show one identical break: the same randomly chosen message and the same
remaining time. The break that started earlier wins and replaces the other.

## Problem

After `random-break-message`, the message was chosen independently in each
backend (`WireBreak::from` and `x11_overlay`), and the synced `BreakStarted`
event carried only the break `name`. Because active time is itself synced, both
machines cross a scheduled break threshold within the same sample tick and each
starts its own break locally; the inbound `BreakStarted` was then dropped
because the scheduler was already `Pending`. So two machines usually showed
different messages, with no way to converge.

## Decision

Choose the message once, at break construction, and propagate the choice plus a
start timestamp through the sync protocol so every peer can agree on one break.

- Message selection moved off `ScheduledBreak` onto `BreakRule`
  (`BreakRule::random_message`); `ScheduledBreak` now carries a single resolved
  `message: String` instead of a `messages` list. Backends read
  `scheduled_break.message` directly (no more per-backend rolling).
- `SyncEvent::BreakStarted` gained `message: String` and `started_at_ms: u64`
  (Unix-epoch millis stamped on the originating machine). Sync
  `PROTOCOL_VERSION` 2 -> 3.
- The runtime resolves "earlier" with a fixed total order: smaller
  `started_at_ms`, then smaller peer id on an exact tie. This makes the order
  consistent on both machines, so the loser replaces once and both converge with
  no oscillation. The local peer id is now threaded from `SyncTransport`
  (`local_peer_id()`) through `RuntimeSync` into the runtime.

### Inbound break-start handling (`runtime.rs`)

- Not in a break yet (`Ready`): join the peer's break, adopting its message and
  starting from the peer's timestamp.
- Already in a break (`Pending`): if the peer's break is strictly earlier, the
  local overlay is *replaced* — message and remaining time
  (`duration - (now - started_at_ms)`); otherwise the inbound start is ignored.
  Inbound starts never rebroadcast.

A test clock seam (`Clock::System` in production, `Clock::Fixed` in tests) makes
broadcast timestamps and replacement remaining times deterministic.

> Assumption: the two machines have roughly NTP-synced wall clocks (fair for a
> user's own LAN devices). "Earlier" and the recomputed remaining time rely on
> it; near-ties fall back to the peer-id tiebreak either way.

## Replacing a live overlay

Neither overlay could change its message mid-break, so a new backend path was
added (routed specially, like the lock-after-break update — not through the
generic command framing):

- `BackendCommand::ReplaceActiveBreak { message, remaining }`.
- X11: `X11Overlay::update_message`; the backend resets the active break's
  `BreakTimer` to `remaining`, then redraws message + remaining. A pending
  (grab-retry) break instead updates its `ScheduledBreak` message/duration.
- macOS: `DaemonMessage::UpdateBreak` gained an optional `message` (omitted for
  the existing remaining/lock updates); helper `PROTOCOL_VERSION` 6 -> 7. Swift
  `UpdateBreakCommand` decodes the optional `message` and
  `BreakOverlayController.update` applies it when present.

## Tests

- `sync_protocol`: `BreakStarted` round-trip and wire-format test for `message`
  and `startedAtMs`; version assertions bumped to 3.
- `scheduler`: `message_at`/`random_message` moved to `BreakRule`; a `to_break`
  test confirms a single message is resolved from the list.
- `runtime`: earlier peer replaces with its message + realigned remaining; later
  peer is ignored; timestamp ties broken toward the lower peer id (both
  directions). Existing broadcast assertions extended with message + timestamp.
- `macos_helper`: `replace_break` framing carries `message`; `update_break`
  omits it; handshake/version tests updated to 7.

## Verification

- `make check` passes (fmt, clippy `-D warnings`, 275 tests) on macOS.
- `make macos-helper-build` and `make build` pass.
- Live two-machine LAN verification (identical message + countdown on a
  simultaneous break) is pending in this environment, consistent with prior sync
  steps; behavior is covered by the runtime collision tests.
