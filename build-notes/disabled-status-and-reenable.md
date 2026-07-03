# disabled-status-and-reenable

## Goal

- Surface the disabled state in the tray/menu-bar dropdown and add a manual
  re-enable control.

## Decisions

- Generalize the single status row from an active-time-only line into a
  `StatusDisplay` enum (`Active(Duration)` / `DisabledFor(Duration)` /
  `DisabledUntilRestart`) defined in `ui.rs` and shared with the runtime.
- Replace `UiCommand::UpdateActiveTime(Duration)` with
  `UiCommand::UpdateStatus(StatusDisplay)`; the runtime computes the status from
  its `DisableMode` so the UI stays a thin renderer.
- Status text: `Active time: {d}` / `Disabled for {d}` (live countdown) /
  `Permanently disabled`, all via `humantime::format_duration`.
- Add a single always-present "Enable" menu item rather than rebuilding the menu
  on state changes. It starts greyed out (app starts enabled) and is toggled via
  `MenuItem::set_enabled` from each `UpdateStatus`, clickable only while disabled.
- Route the Enable item through a new `RuntimeEvent::Enable` mapped to the
  existing `DaemonRuntime::enable`, so re-enabling also broadcasts
  `SyncEvent::Enable` to peers, mirroring how disable controls broadcast.
- Set `disable_mode` before calling `disable_scheduler` in `disable_for` /
  `disable_until_restart` so the status refresh that `disable_scheduler` emits
  already reflects the disabled state (avoids a stale `Active` flash).
- Drive the countdown from the existing 1s `WallClockElapsed` tick by calling
  `update_status_display` at the end of `advance_wall_clock`; the
  `displayed_status` dedup keeps enabled ticks from spamming the channel.
- Follow-up fix: timed disables now store an absolute Unix-millisecond deadline
  instead of a remaining duration, so time spent asleep counts once the runtime
  wakes and observes the current system clock. Automatic timed expiry still
  suppresses a sync `Enable` broadcast.

## Behavior

- While timed-disabled, the status row replaces "Active time" with a per-second
  countdown (`Disabled for 29m 59s`) and the Enable item becomes clickable.
- While disabled until restart, the row reads `Permanently disabled` and Enable
  is clickable.
- Clicking Enable (or a timed disable expiring, or a synced enable) returns the
  row to `Active time: …` and greys out the Enable item.
- If the machine sleeps past a finite disable deadline, the next wall-clock tick
  re-enables scheduling instead of leaving the original countdown frozen.

## Tests

- `ui.rs`: `status_menu_text_renders_each_state` covers all three rows;
  `menu_actions_map_to_runtime_events` covers `Enable -> RuntimeEvent::Enable`.
- `runtime/tests.rs`: existing UI-command assertions updated to
  `UpdateStatus(StatusDisplay::Active(..))`;
  `disabled_and_pending_states_suppress_pre_break_notifications` now asserts the
  disable/enable status sequence (still no pre-break notification);
  new `timed_disable_shows_countdown_status_until_reenabled` asserts the
  per-second `DisabledFor` countdown and the auto re-enable to `Active(0)`.
- Follow-up runtime tests cover sleep-style clock jumps with only a small wake
  tick and verify automatic timed expiry is not rebroadcast to sync peers.

## Commands

- Follow-up sleep-counting fix: `make test` passes (315 tests).
- Follow-up sleep-counting fix: `make check` passes (fmt-check, clippy
  `-D warnings`, 315 tests).

## Follow-up

- Manual tray verification (Linux and macOS) of the countdown and Enable item is
  pending, consistent with the other tray-UI steps.
