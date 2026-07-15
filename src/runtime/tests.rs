use super::{
    Clock, DaemonRuntime, RuntimeInput, RuntimeSync, SyncEventBroadcaster, run_with_event_sources,
};
use crate::backend::{BackendActor, BackendCommand, DisableRequest, RuntimeEvent};
use crate::config::{
    BreakTypeConfig, Breaks, Config, ConfigError, LockConfig, StartupConfig, SyncConfig,
};
use crate::scheduler::{BreakOrigin, BreakSchedule, ScheduledBreak};
use crate::sync_protocol::{
    PeerId, SyncActiveBreak, SyncBreakOrigin, SyncEvent, SyncSchedulerPosition,
};
use crate::sync_transport::{PeerRejectionReason, SyncTransportError, SyncTransportEvent};
use crate::ui::{PreBreakNotification, RuntimeUi, StatusDisplay, UiCommand, UiNotification};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

/// Fixed wall-clock value (Unix millis) used by the deterministic test clock so
/// broadcast `started_at_ms` stamps and replacement remaining times are stable.
const TEST_NOW_MS: u64 = 1_700_000_000_000;

#[test]
fn shutdown_exits_cleanly_after_scheduler_setup() {
    let (backend, commands) = ScriptedBackend::new([RuntimeEvent::Shutdown]).into_parts();

    assert_eq!(run_config_with_backend(test_config(), backend), Ok(()));
    assert!(received_commands(&commands).is_empty());
}

#[test]
fn startup_shows_the_first_upcoming_break() -> Result<(), ConfigError> {
    let (backend, _commands) = ScriptedBackend::new([RuntimeEvent::Shutdown]).into_parts();
    let (ui, ui_commands) = recording_ui();
    let schedule = BreakSchedule::try_from(test_config().breaks)?;

    run_with_event_sources(
        schedule,
        backend,
        RuntimeSync::inactive(),
        ui,
        Clock::Fixed(TEST_NOW_MS),
    );

    assert_eq!(
        received_ui_commands(&ui_commands),
        vec![upcoming_break_status("short", Duration::from_secs(10))]
    );
    Ok(())
}

#[test]
fn active_time_event_starts_expected_configured_break() {
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(run_config_with_backend(test_config(), backend), Ok(()));
    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
}

#[test]
fn break_finished_allows_next_scheduled_break_to_advance() {
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(run_config_with_backend(test_config(), backend), Ok(()));
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::FinishBreak { lock_after: false },
            BackendCommand::StartBreak(scheduled_break("long", 2, 300))
        ]
    );
}

#[test]
fn duplicate_break_finished_finishes_overlay_exactly_once() {
    // The macOS backend queues `BreakFinished` exactly once, but a second one
    // must never produce a second `FinishBreak` (which would spuriously lock or
    // beep again). `finish_break` guards on `current_break`, so the duplicate is
    // a no-op once the overlay has been finished.
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::BreakFinished,
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(run_config_with_backend(test_config(), backend), Ok(()));
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::FinishBreak { lock_after: false },
        ]
    );
}

#[test]
fn autolock_break_completion_requests_local_lock() {
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(run_config_with_backend(test_config(), backend), Ok(()));
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::FinishBreak { lock_after: false },
            BackendCommand::StartBreak(scheduled_break("long", 2, 300)),
            BackendCommand::FinishBreak { lock_after: true }
        ]
    );
}

#[test]
fn lock_after_current_break_request_locks_after_non_autolock_break() {
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::LockAfterCurrentBreak,
        RuntimeEvent::BreakFinished,
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(run_config_with_backend(test_config(), backend), Ok(()));
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::RequestLockAfterCurrentBreak,
            BackendCommand::FinishBreak { lock_after: true }
        ]
    );
}

#[test]
fn stale_lock_after_current_break_request_before_break_is_ignored() {
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::LockAfterCurrentBreak,
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(run_config_with_backend(test_config(), backend), Ok(()));
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::FinishBreak { lock_after: false }
        ]
    );
}

#[test]
fn lock_after_current_break_request_clears_after_break_finishes() {
    let mut config = test_config();
    _ = config.breaks.types.remove("long");
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::LockAfterCurrentBreak,
        RuntimeEvent::BreakFinished,
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(run_config_with_backend(config, backend), Ok(()));
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::RequestLockAfterCurrentBreak,
            BackendCommand::FinishBreak { lock_after: true },
            BackendCommand::StartBreak(scheduled_break("short", 2, 20)),
            BackendCommand::FinishBreak { lock_after: false }
        ]
    );
}

#[test]
fn break_start_failure_skips_pending_break_without_finishing_backend_break() {
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::LockAfterCurrentBreak,
        RuntimeEvent::BreakStartFailed,
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(run_config_with_backend(test_config(), backend), Ok(()));
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::RequestLockAfterCurrentBreak,
            BackendCommand::StartBreak(scheduled_break("long", 2, 300))
        ]
    );
}

#[test]
fn disable_clears_pending_backend_break_without_locking() {
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::Disable(DisableRequest::For(Duration::from_secs(30))),
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(run_config_with_backend(test_config(), backend), Ok(()));
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::ClearBreak
        ]
    );
}

#[test]
fn disable_clears_lock_after_current_break_request() {
    let mut config = test_config();
    _ = config.breaks.types.remove("long");
    let (backend, commands) = test_backend();

    assert_eq!(
        run_config_with_steps(
            config,
            backend,
            [
                backend_step(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
                backend_step(RuntimeEvent::LockAfterCurrentBreak),
                backend_step(RuntimeEvent::Disable(DisableRequest::For(
                    Duration::from_secs(30),
                ))),
                advance_clock(Duration::from_secs(30)),
                backend_step(RuntimeEvent::WallClockElapsed(Duration::from_secs(30))),
                backend_step(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
                backend_step(RuntimeEvent::BreakFinished),
            ],
        ),
        Ok(())
    );
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::RequestLockAfterCurrentBreak,
            BackendCommand::ClearBreak,
            BackendCommand::StartBreak(scheduled_break("short", 2, 20)),
            BackendCommand::FinishBreak { lock_after: false }
        ]
    );
}

#[test]
fn finite_disable_suppresses_active_time_and_reenables_after_elapsed() {
    let (backend, commands) = test_backend();

    assert_eq!(
        run_config_with_steps(
            test_config(),
            backend,
            [
                backend_step(RuntimeEvent::Disable(DisableRequest::For(
                    Duration::from_secs(30),
                ))),
                backend_step(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(100))),
                advance_clock(Duration::from_secs(29)),
                backend_step(RuntimeEvent::WallClockElapsed(Duration::from_secs(29))),
                backend_step(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
                advance_clock(Duration::from_secs(1)),
                backend_step(RuntimeEvent::WallClockElapsed(Duration::from_secs(1))),
                backend_step(RuntimeEvent::WallClockElapsed(Duration::from_secs(10))),
                backend_step(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            ],
        ),
        Ok(())
    );
    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
}

#[test]
fn finite_disable_expires_after_sleep_even_with_small_wake_tick() {
    let (backend, commands) = test_backend();

    assert_eq!(
        run_config_with_steps(
            test_config(),
            backend,
            [
                backend_step(RuntimeEvent::Disable(DisableRequest::For(
                    Duration::from_mins(30),
                ))),
                backend_step(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(100))),
                advance_clock(Duration::from_mins(31)),
                backend_step(RuntimeEvent::WallClockElapsed(Duration::from_secs(1))),
                backend_step(RuntimeEvent::WallClockElapsed(Duration::from_secs(10))),
                backend_step(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            ],
        ),
        Ok(())
    );
    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
}

#[test]
fn disable_until_restart_ignores_wall_clock_elapsed() {
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::Disable(DisableRequest::UntilRestart),
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(100)),
        RuntimeEvent::WallClockElapsed(Duration::from_hours(1)),
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(100)),
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(run_config_with_backend(test_config(), backend), Ok(()));
    assert!(received_commands(&commands).is_empty());
}

#[test]
fn manual_break_event_starts_configured_break_without_advancing_slots() {
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(9)),
        RuntimeEvent::StartManualBreak(String::from("long")),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(1)),
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(9)),
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(run_config_with_backend(test_config(), backend), Ok(()));
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(manual_break("long", 300)),
            BackendCommand::FinishBreak { lock_after: true },
            BackendCommand::StartBreak(scheduled_break("short", 1, 20))
        ]
    );
}

#[test]
fn manual_break_event_works_while_disabled_and_preserves_disable() {
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::Disable(DisableRequest::For(Duration::from_secs(30))),
        RuntimeEvent::StartManualBreak(String::from("short")),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(100)),
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(run_config_with_backend(test_config(), backend), Ok(()));
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(manual_break("short", 20)),
            BackendCommand::FinishBreak { lock_after: false }
        ]
    );
}

#[test]
fn timed_disable_can_expire_during_manual_break() {
    let (backend, commands) = test_backend();

    assert_eq!(
        run_config_with_steps(
            test_config(),
            backend,
            [
                backend_step(RuntimeEvent::Disable(DisableRequest::For(
                    Duration::from_secs(30),
                ))),
                backend_step(RuntimeEvent::StartManualBreak(String::from("short"))),
                advance_clock(Duration::from_secs(30)),
                backend_step(RuntimeEvent::WallClockElapsed(Duration::from_secs(30))),
                backend_step(RuntimeEvent::BreakFinished),
                backend_step(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            ],
        ),
        Ok(())
    );
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(manual_break("short", 20)),
            BackendCommand::FinishBreak { lock_after: false },
            BackendCommand::StartBreak(scheduled_break("short", 1, 20))
        ]
    );
}

#[test]
fn unknown_manual_break_event_is_ignored() {
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::StartManualBreak(String::from("missing")),
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(run_config_with_backend(test_config(), backend), Ok(()));
    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
}

#[test]
fn scheduler_setup_error_is_returned() {
    let mut config = test_config();
    config.breaks.types.clear();
    let (backend, _commands) = ScriptedBackend::new([RuntimeEvent::Shutdown]).into_parts();

    assert_eq!(
        run_config_with_backend(config, backend),
        Err(ConfigError::EmptyBreakTypes)
    );
}

#[test]
fn sync_transport_peer_events_do_not_trigger_scheduler_behavior()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();
    let (ui, ui_commands) = recording_ui();
    let peer_id = peer_id()?;

    run_config_with_inputs_and_ui(
        test_config(),
        backend,
        ui,
        [
            sync_input(SyncTransportEvent::PeerAuthenticated(peer_id)),
            sync_input(SyncTransportEvent::PeerDisconnected(peer_id)),
        ],
    )?;

    assert!(received_commands(&commands).is_empty());
    assert!(received_ui_commands(&ui_commands).is_empty());
    Ok(())
}

#[test]
fn rejected_sync_peer_shows_notification_once_per_peer() -> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();
    let (ui, ui_commands) = recording_ui();
    let peer_id = peer_id()?;

    run_config_with_inputs_and_ui(
        test_config(),
        backend,
        ui,
        [
            sync_input(rejected_peer(peer_id)),
            sync_input(rejected_peer(peer_id)),
        ],
    )?;

    assert!(received_commands(&commands).is_empty());
    assert_eq!(
        received_ui_commands(&ui_commands),
        vec![UiCommand::ShowNotification(UiNotification {
            summary: String::from("RustEyes sync peer rejected"),
            body: String::from(
                "Peer 01020304... was rejected because its break settings do not match."
            ),
        })]
    );
    Ok(())
}

#[test]
fn local_active_time_is_broadcast_to_sync_peers() {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(1)),
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(
        run_config_with_sync_broadcaster(test_config(), backend, &sync_broadcaster),
        Ok(())
    );
    assert!(received_commands(&commands).is_empty());
    assert_eq!(
        sync_broadcaster.events(),
        vec![SyncEvent::ActiveTimeElapsed {
            elapsed: Duration::from_secs(1),
        }]
    );
}

#[test]
fn local_scheduled_break_start_is_broadcast_to_sync_peers() {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(
        run_config_with_sync_broadcaster(test_config(), backend, &sync_broadcaster),
        Ok(())
    );
    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    assert_eq!(
        sync_broadcaster.events(),
        vec![
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break("short", 1),
        ]
    );
}

#[test]
fn local_manual_break_start_is_broadcast_to_sync_peers() {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::StartManualBreak(String::from("long")),
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(
        run_config_with_sync_broadcaster(test_config(), backend, &sync_broadcaster),
        Ok(())
    );
    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(manual_break("long", 300))]
    );
    assert_eq!(
        sync_broadcaster.events(),
        vec![broadcast_manual_break("long")]
    );
}

#[test]
fn local_manual_long_break_satisfies_long_cadence() {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::StartManualBreak(String::from("long")),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(
        run_config_with_sync_broadcaster(test_config(), backend, &sync_broadcaster),
        Ok(())
    );
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::FinishBreak { lock_after: false },
            BackendCommand::StartBreak(manual_break("long", 300)),
            BackendCommand::FinishBreak { lock_after: true },
            BackendCommand::StartBreak(scheduled_break("short", 2, 20)),
        ]
    );
    assert_eq!(
        sync_broadcaster.events(),
        vec![
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break("short", 1),
            broadcast_manual_break_at("long", 1),
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break_with_slots("short", 2, 2, 1),
        ]
    );
}

#[test]
fn local_short_manual_break_before_long_break_is_ignored_and_not_broadcast() {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::StartManualBreak(String::from("short")),
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(
        run_config_with_sync_broadcaster(test_config(), backend, &sync_broadcaster),
        Ok(())
    );
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::FinishBreak { lock_after: false },
            BackendCommand::StartBreak(scheduled_break("long", 2, 300)),
        ]
    );
    assert_eq!(
        sync_broadcaster.events(),
        vec![
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break("short", 1),
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break("long", 2),
        ]
    );
}

#[test]
fn local_disable_events_are_broadcast_to_sync_peers() {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::Disable(DisableRequest::For(Duration::from_secs(30))),
        RuntimeEvent::Disable(DisableRequest::UntilRestart),
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(
        run_config_with_sync_broadcaster(test_config(), backend, &sync_broadcaster),
        Ok(())
    );
    assert!(received_commands(&commands).is_empty());
    assert_eq!(
        sync_broadcaster.events(),
        vec![
            SyncEvent::DisableFor {
                duration: Duration::from_secs(30),
            },
            SyncEvent::DisableUntilRestart,
        ]
    );
}

#[test]
fn automatic_timed_disable_expiry_is_not_broadcast_to_sync_peers() {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    assert_eq!(
        run_config_with_steps_and_sync_broadcaster_and_ui(
            test_config(),
            backend,
            &sync_broadcaster,
            RuntimeUi::inactive(),
            [
                backend_step(RuntimeEvent::Disable(DisableRequest::For(
                    Duration::from_secs(30),
                ))),
                advance_clock(Duration::from_secs(30)),
                backend_step(RuntimeEvent::WallClockElapsed(Duration::from_secs(1))),
            ],
        ),
        Ok(())
    );
    assert!(received_commands(&commands).is_empty());
    assert_eq!(
        sync_broadcaster.events(),
        vec![SyncEvent::DisableFor {
            duration: Duration::from_secs(30),
        }]
    );
}

#[test]
fn local_lock_after_current_break_request_is_broadcast_once_to_sync_peers() {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::LockAfterCurrentBreak,
        RuntimeEvent::LockAfterCurrentBreak,
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(
        run_config_with_sync_broadcaster(test_config(), backend, &sync_broadcaster),
        Ok(())
    );
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::RequestLockAfterCurrentBreak,
        ]
    );
    assert_eq!(
        sync_broadcaster.events(),
        vec![
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break("short", 1),
            SyncEvent::LockAfterCurrentBreak,
        ]
    );
}

#[test]
fn stale_local_lock_after_current_break_request_is_not_broadcast() {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::LockAfterCurrentBreak,
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(
        run_config_with_sync_broadcaster(test_config(), backend, &sync_broadcaster),
        Ok(())
    );
    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::FinishBreak { lock_after: false },
        ]
    );
    assert_eq!(
        sync_broadcaster.events(),
        vec![
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break("short", 1),
        ]
    );
}

#[test]
fn remote_active_time_event_starts_expected_configured_break()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config(),
        backend,
        [
            backend_input(RuntimeEvent::WallClockElapsed(Duration::from_secs(10))),
            sync_input(remote_active_time(Duration::from_secs(10))?),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    Ok(())
}

#[test]
fn remote_active_time_triggered_scheduled_break_is_broadcast_without_active_time_echo()
-> Result<(), Box<dyn std::error::Error>> {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    run_config_with_inputs_and_sync_broadcaster(
        test_config(),
        backend,
        &sync_broadcaster,
        [
            backend_input(RuntimeEvent::WallClockElapsed(Duration::from_secs(10))),
            sync_input(remote_active_time(Duration::from_secs(10))?),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    assert_eq!(
        sync_broadcaster.events(),
        vec![broadcast_scheduled_break("short", 1)]
    );
    Ok(())
}

#[test]
fn local_and_remote_active_time_share_wall_clock_budget() -> Result<(), Box<dyn std::error::Error>>
{
    let (backend, commands) = test_backend();
    let mut inputs = Vec::new();

    for _ in 0..5 {
        inputs.push(backend_input(RuntimeEvent::WallClockElapsed(
            Duration::from_secs(1),
        )));
        inputs.push(sync_input(remote_active_time(Duration::from_secs(1))?));
        inputs.push(backend_input(RuntimeEvent::ActiveTimeElapsed(
            Duration::from_secs(1),
        )));
    }

    run_config_with_inputs(test_config(), backend, inputs)?;

    assert!(received_commands(&commands).is_empty());
    Ok(())
}

#[test]
fn overlapping_local_and_remote_active_time_start_break_after_one_interval()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();
    let mut inputs = Vec::new();

    for _ in 0..10 {
        inputs.push(backend_input(RuntimeEvent::WallClockElapsed(
            Duration::from_secs(1),
        )));
        inputs.push(backend_input(RuntimeEvent::ActiveTimeElapsed(
            Duration::from_secs(1),
        )));
        inputs.push(sync_input(remote_active_time(Duration::from_secs(1))?));
    }

    run_config_with_inputs(test_config(), backend, inputs)?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    Ok(())
}

#[test]
fn remote_only_active_time_uses_local_wall_clock_budget() -> Result<(), Box<dyn std::error::Error>>
{
    let (backend, commands) = test_backend();
    let mut inputs = Vec::new();

    for _ in 0..10 {
        inputs.push(backend_input(RuntimeEvent::WallClockElapsed(
            Duration::from_secs(1),
        )));
        inputs.push(sync_input(remote_active_time(Duration::from_secs(1))?));
    }

    run_config_with_inputs(test_config(), backend, inputs)?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    Ok(())
}

#[test]
fn remote_break_start_event_starts_configured_break_without_rebroadcast()
-> Result<(), Box<dyn std::error::Error>> {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    run_config_with_inputs_and_sync_broadcaster(
        test_config(),
        backend,
        &sync_broadcaster,
        [sync_input(incoming_break(
            "short",
            &break_message("short"),
            TEST_NOW_MS,
        )?)],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    assert!(sync_broadcaster.events().is_empty());
    Ok(())
}

#[test]
fn remote_manual_break_start_uses_manual_origin() -> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config(),
        backend,
        [
            sync_input(incoming_manual_break(
                "long",
                &break_message("long"),
                TEST_NOW_MS,
            )?),
            backend_input(RuntimeEvent::BreakFinished),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(manual_break("long", 300)),
            BackendCommand::FinishBreak { lock_after: true },
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
        ]
    );
    Ok(())
}

#[test]
fn remote_manual_long_break_satisfies_long_cadence_without_rebroadcast()
-> Result<(), Box<dyn std::error::Error>> {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    run_config_with_inputs_and_sync_broadcaster(
        test_config(),
        backend,
        &sync_broadcaster,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            backend_input(RuntimeEvent::BreakFinished),
            sync_input(incoming_manual_break_at(
                "long",
                &break_message("long"),
                TEST_NOW_MS,
                1,
            )?),
            backend_input(RuntimeEvent::BreakFinished),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::FinishBreak { lock_after: false },
            BackendCommand::StartBreak(manual_break("long", 300)),
            BackendCommand::FinishBreak { lock_after: true },
            BackendCommand::StartBreak(scheduled_break("short", 2, 20)),
        ]
    );
    assert_eq!(
        sync_broadcaster.events(),
        vec![
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break("short", 1),
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break_with_slots("short", 2, 2, 1),
        ]
    );
    Ok(())
}

#[test]
fn remote_scheduled_break_start_advances_scheduler_counter()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config(),
        backend,
        [
            sync_input(incoming_scheduled_break(
                "long",
                &break_message("long"),
                TEST_NOW_MS,
                2,
            )?),
            backend_input(RuntimeEvent::BreakFinished),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("long", 2, 300)),
            BackendCommand::FinishBreak { lock_after: true },
            BackendCommand::StartBreak(scheduled_break("short", 3, 20)),
        ]
    );
    Ok(())
}

#[test]
fn peer_authentication_sends_current_scheduler_state() -> Result<(), Box<dyn std::error::Error>> {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    run_config_with_inputs_and_sync_broadcaster(
        test_config(),
        backend,
        &sync_broadcaster,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(4))),
            sync_input(SyncTransportEvent::PeerAuthenticated(peer_id()?)),
        ],
    )?;

    assert!(received_commands(&commands).is_empty());
    assert_eq!(
        sync_broadcaster.directed_events(),
        vec![(
            peer_id()?,
            SyncEvent::SchedulerState {
                slot: 0,
                active_elapsed: Duration::from_secs(4),
                last_satisfied_slots: test_last_satisfied_slots(0, 0),
                active_break: None,
            },
        )]
    );
    Ok(())
}

#[test]
fn remote_scheduler_state_catches_up_counter_and_active_elapsed()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config(),
        backend,
        [
            sync_input(remote_sync_event(SyncEvent::SchedulerState {
                slot: 1,
                active_elapsed: Duration::from_secs(5),
                last_satisfied_slots: test_last_satisfied_slots(1, 0),
                active_break: None,
            })?),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("long", 2, 300))]
    );
    Ok(())
}

#[test]
fn remote_break_start_for_current_slot_joins_after_scheduler_state()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config(),
        backend,
        [
            sync_input(remote_sync_event(SyncEvent::SchedulerState {
                slot: 1,
                active_elapsed: Duration::ZERO,
                last_satisfied_slots: test_last_satisfied_slots(1, 0),
                active_break: None,
            })?),
            sync_input(incoming_scheduled_break(
                "short",
                "Peer message",
                TEST_NOW_MS - 5_000,
                1,
            )?),
            backend_input(RuntimeEvent::BreakFinished),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(ScheduledBreak {
                name: String::from("short"),
                origin: BreakOrigin::Scheduled { slot: 1 },
                duration: Duration::from_secs(15),
                message: String::from("Peer message"),
                autolock: false,
            }),
            BackendCommand::FinishBreak { lock_after: false },
            BackendCommand::StartBreak(scheduled_break("long", 2, 300)),
        ]
    );
    Ok(())
}

#[test]
fn remote_scheduler_state_joins_active_break_with_remaining_time()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config(),
        backend,
        [
            sync_input(remote_sync_event(SyncEvent::SchedulerState {
                slot: 2,
                active_elapsed: Duration::ZERO,
                last_satisfied_slots: test_last_satisfied_slots(2, 2),
                active_break: Some(SyncActiveBreak {
                    name: String::from("long"),
                    message: String::from("Peer message"),
                    started_at_ms: TEST_NOW_MS - 30_000,
                    origin: SyncBreakOrigin::Scheduled { slot: 2 },
                    lock_after: true,
                }),
            })?),
            backend_input(RuntimeEvent::BreakFinished),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(ScheduledBreak {
                name: String::from("long"),
                origin: BreakOrigin::Scheduled { slot: 2 },
                duration: Duration::from_secs(270),
                message: String::from("Peer message"),
                autolock: true,
            }),
            BackendCommand::FinishBreak { lock_after: true },
            BackendCommand::StartBreak(scheduled_break("short", 3, 20)),
        ]
    );
    Ok(())
}

#[test]
fn expired_remote_active_break_snapshot_only_advances_counter()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config(),
        backend,
        [
            sync_input(remote_sync_event(SyncEvent::SchedulerState {
                slot: 1,
                active_elapsed: Duration::ZERO,
                last_satisfied_slots: test_last_satisfied_slots(1, 0),
                active_break: Some(SyncActiveBreak {
                    name: String::from("short"),
                    message: String::from("Peer message"),
                    started_at_ms: TEST_NOW_MS - 25_000,
                    origin: SyncBreakOrigin::Scheduled { slot: 1 },
                    lock_after: false,
                }),
            })?),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("long", 2, 300))]
    );
    Ok(())
}

#[test]
fn remote_break_start_event_ignores_unknown_break_name_without_rebroadcast()
-> Result<(), Box<dyn std::error::Error>> {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    run_config_with_inputs_and_sync_broadcaster(
        test_config(),
        backend,
        &sync_broadcaster,
        [sync_input(incoming_break(
            "missing",
            &break_message("missing"),
            TEST_NOW_MS,
        )?)],
    )?;

    assert!(received_commands(&commands).is_empty());
    assert!(sync_broadcaster.events().is_empty());
    Ok(())
}

#[test]
fn synced_earlier_break_replaces_current_break_message_and_remaining()
-> Result<(), Box<dyn std::error::Error>> {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    // The local break starts at TEST_NOW_MS; the peer's break started 5s earlier
    // so it wins and the local overlay adopts its message with 5s already gone.
    run_config_with_inputs_sync_broadcaster_ui_and_local_peer(
        test_config(),
        backend,
        &sync_broadcaster,
        RuntimeUi::inactive(),
        Some(local_peer_higher()?),
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            sync_input(incoming_break("short", "Look away", TEST_NOW_MS - 5_000)?),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::ReplaceActiveBreak {
                message: String::from("Look away"),
                remaining: Duration::from_secs(15),
                lock_after: false,
            },
        ]
    );
    // The adopted break is applied locally only; the local start is still the
    // single broadcast.
    assert_eq!(
        sync_broadcaster.events(),
        vec![
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break("short", 1),
        ]
    );
    Ok(())
}

#[test]
fn synced_later_break_does_not_replace_current_break() -> Result<(), Box<dyn std::error::Error>> {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    // The peer's break started later, so the local (earlier) break wins and is
    // left untouched.
    run_config_with_inputs_sync_broadcaster_ui_and_local_peer(
        test_config(),
        backend,
        &sync_broadcaster,
        RuntimeUi::inactive(),
        Some(local_peer_lower()?),
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            sync_input(incoming_break("short", "Look away", TEST_NOW_MS + 5_000)?),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    Ok(())
}

#[test]
fn synced_break_tie_is_broken_toward_the_lower_peer_id() -> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    // Equal start timestamps: the sending peer id is lower than ours, so the
    // peer wins the tie and the local overlay adopts its message.
    run_config_with_inputs_sync_broadcaster_ui_and_local_peer(
        test_config(),
        backend,
        &NOOP_SYNC_BROADCASTER_FOR_TEST,
        RuntimeUi::inactive(),
        Some(local_peer_higher()?),
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            sync_input(incoming_break("short", "Look away", TEST_NOW_MS)?),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::ReplaceActiveBreak {
                message: String::from("Look away"),
                remaining: Duration::from_secs(20),
                lock_after: false,
            },
        ]
    );
    Ok(())
}

#[test]
fn synced_break_tie_keeps_local_break_when_local_peer_id_is_lower()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    // Equal start timestamps: our peer id is lower, so the local break wins the
    // tie and is left untouched, and both machines converge on it.
    run_config_with_inputs_sync_broadcaster_ui_and_local_peer(
        test_config(),
        backend,
        &NOOP_SYNC_BROADCASTER_FOR_TEST,
        RuntimeUi::inactive(),
        Some(local_peer_lower()?),
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            sync_input(incoming_break("short", "Look away", TEST_NOW_MS)?),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    Ok(())
}

#[test]
fn remote_disable_event_clears_pending_break_without_rebroadcast()
-> Result<(), Box<dyn std::error::Error>> {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    run_config_with_inputs_and_sync_broadcaster(
        test_config(),
        backend,
        &sync_broadcaster,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            sync_input(remote_sync_event(SyncEvent::DisableFor {
                duration: Duration::from_secs(30),
            })?),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::ClearBreak,
        ]
    );
    assert_eq!(
        sync_broadcaster.events(),
        vec![
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break("short", 1),
        ]
    );
    Ok(())
}

#[test]
fn remote_disable_until_restart_and_enable_control_scheduler()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config(),
        backend,
        [
            sync_input(remote_sync_event(SyncEvent::DisableUntilRestart)?),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            sync_input(remote_sync_event(SyncEvent::Enable)?),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    Ok(())
}

#[test]
fn remote_lock_after_current_break_request_applies_without_rebroadcast()
-> Result<(), Box<dyn std::error::Error>> {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    run_config_with_inputs_and_sync_broadcaster(
        test_config(),
        backend,
        &sync_broadcaster,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            sync_input(remote_sync_event(SyncEvent::LockAfterCurrentBreak)?),
            sync_input(remote_sync_event(SyncEvent::LockAfterCurrentBreak)?),
            backend_input(RuntimeEvent::BreakFinished),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::RequestLockAfterCurrentBreak,
            BackendCommand::FinishBreak { lock_after: true },
        ]
    );
    assert_eq!(
        sync_broadcaster.events(),
        vec![
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break("short", 1),
        ]
    );
    Ok(())
}

#[test]
fn stale_remote_lock_after_current_break_request_is_ignored()
-> Result<(), Box<dyn std::error::Error>> {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    run_config_with_inputs_and_sync_broadcaster(
        test_config(),
        backend,
        &sync_broadcaster,
        [
            sync_input(remote_sync_event(SyncEvent::LockAfterCurrentBreak)?),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            backend_input(RuntimeEvent::BreakFinished),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::FinishBreak { lock_after: false },
        ]
    );
    assert_eq!(
        sync_broadcaster.events(),
        vec![
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break("short", 1),
        ]
    );
    Ok(())
}

#[test]
fn remote_active_time_is_not_rebroadcast() -> Result<(), Box<dyn std::error::Error>> {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    run_config_with_inputs_and_sync_broadcaster(
        test_config(),
        backend,
        &sync_broadcaster,
        [sync_input(remote_active_time(Duration::from_secs(1))?)],
    )?;

    assert!(received_commands(&commands).is_empty());
    assert!(sync_broadcaster.events().is_empty());
    Ok(())
}

#[test]
fn idle_below_reset_timeout_preserves_partial_active_time() -> Result<(), Box<dyn std::error::Error>>
{
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config_with_reset_after_idle(Some(Duration::from_secs(5))),
        backend,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(9))),
            backend_input(RuntimeEvent::IdleTimeElapsed(Duration::from_secs(4))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(1))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    Ok(())
}

#[test]
fn idle_at_reset_timeout_discards_partial_active_time() -> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config_with_reset_after_idle(Some(Duration::from_secs(5))),
        backend,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(9))),
            backend_input(RuntimeEvent::IdleTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(1))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(9))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    Ok(())
}

#[test]
fn post_sleep_idle_gap_resets_partial_active_time_before_active_sample()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config_with_reset_after_idle(Some(Duration::from_secs(5))),
        backend,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(9))),
            backend_input(RuntimeEvent::WallClockElapsed(Duration::from_secs(1))),
            backend_input(RuntimeEvent::IdleTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(1))),
        ],
    )?;

    assert!(received_commands(&commands).is_empty());
    Ok(())
}

#[test]
fn idle_reset_restarts_break_counter_after_completed_break()
-> Result<(), Box<dyn std::error::Error>> {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    run_config_with_inputs_and_sync_broadcaster(
        test_config_with_reset_count_after_idle(Some(Duration::from_secs(5))),
        backend,
        &sync_broadcaster,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            backend_input(RuntimeEvent::BreakFinished),
            backend_input(RuntimeEvent::IdleTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::FinishBreak { lock_after: false },
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
        ]
    );
    assert_eq!(
        sync_broadcaster.events(),
        vec![
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break("short", 1),
            SyncEvent::SchedulerReset,
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break("short", 1),
        ]
    );
    Ok(())
}

#[test]
fn post_sleep_idle_gap_restarts_break_counter_after_completed_break()
-> Result<(), Box<dyn std::error::Error>> {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    run_config_with_inputs_and_sync_broadcaster(
        test_config_with_reset_count_after_idle(Some(Duration::from_secs(5))),
        backend,
        &sync_broadcaster,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            backend_input(RuntimeEvent::BreakFinished),
            backend_input(RuntimeEvent::WallClockElapsed(Duration::from_secs(1))),
            backend_input(RuntimeEvent::IdleTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(1))),
            backend_input(RuntimeEvent::WallClockElapsed(Duration::from_secs(9))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(9))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::FinishBreak { lock_after: false },
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
        ]
    );
    assert_eq!(
        sync_broadcaster.events(),
        vec![
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break("short", 1),
            SyncEvent::SchedulerReset,
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(1),
            },
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(9),
            },
            broadcast_scheduled_break("short", 1),
        ]
    );
    Ok(())
}

#[test]
fn active_time_idle_reset_keeps_break_counter_and_does_not_broadcast_scheduler_reset()
-> Result<(), Box<dyn std::error::Error>> {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    run_config_with_inputs_and_sync_broadcaster(
        test_config_with_reset_after_idle(Some(Duration::from_secs(5))),
        backend,
        &sync_broadcaster,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            backend_input(RuntimeEvent::BreakFinished),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(9))),
            backend_input(RuntimeEvent::IdleTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(1))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(9))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::FinishBreak { lock_after: false },
            BackendCommand::StartBreak(scheduled_break("long", 2, 300)),
        ]
    );
    assert!(
        !sync_broadcaster
            .events()
            .contains(&SyncEvent::SchedulerReset)
    );
    Ok(())
}

#[test]
fn disabled_idle_reset_preserves_current_active_time_behavior()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config_with_reset_after_idle(None),
        backend,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(9))),
            backend_input(RuntimeEvent::IdleTimeElapsed(Duration::from_secs(30))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(1))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    Ok(())
}

#[test]
fn disabled_break_count_idle_reset_preserves_completed_break_counter()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config_with_reset_count_after_idle(None),
        backend,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            backend_input(RuntimeEvent::BreakFinished),
            backend_input(RuntimeEvent::IdleTimeElapsed(Duration::from_secs(30))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::FinishBreak { lock_after: false },
            BackendCommand::StartBreak(scheduled_break("long", 2, 300)),
        ]
    );
    Ok(())
}

#[test]
fn remote_active_time_resets_combined_idle_tracking() -> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config_with_reset_after_idle(Some(Duration::from_secs(5))),
        backend,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(4))),
            backend_input(RuntimeEvent::IdleTimeElapsed(Duration::from_secs(4))),
            sync_input(remote_active_time(Duration::from_secs(1))?),
            backend_input(RuntimeEvent::IdleTimeElapsed(Duration::from_secs(4))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    Ok(())
}

#[test]
fn break_count_idle_reset_broadcasts_scheduler_reset_to_sync_peers() {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    assert_eq!(
        run_config_with_inputs_and_sync_broadcaster(
            test_config_with_reset_count_after_idle(Some(Duration::from_secs(5))),
            backend,
            &sync_broadcaster,
            [
                backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(4))),
                backend_input(RuntimeEvent::IdleTimeElapsed(Duration::from_secs(5))),
            ],
        ),
        Ok(())
    );

    assert!(received_commands(&commands).is_empty());
    assert_eq!(
        sync_broadcaster.events(),
        vec![
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(4),
            },
            SyncEvent::SchedulerReset,
        ]
    );
}

#[test]
fn inbound_scheduler_reset_restarts_counter_without_rebroadcast()
-> Result<(), Box<dyn std::error::Error>> {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    run_config_with_inputs_and_sync_broadcaster(
        test_config_with_reset_after_idle(Some(Duration::from_secs(5))),
        backend,
        &sync_broadcaster,
        [
            sync_input(remote_sync_event(SyncEvent::SchedulerState {
                slot: 1,
                active_elapsed: Duration::ZERO,
                last_satisfied_slots: test_last_satisfied_slots(1, 0),
                active_break: None,
            })?),
            sync_input(remote_sync_event(SyncEvent::SchedulerReset)?),
            backend_input(RuntimeEvent::IdleTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    assert_eq!(
        sync_broadcaster.events(),
        vec![
            SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_secs(10),
            },
            broadcast_scheduled_break("short", 1),
        ]
    );
    Ok(())
}

#[test]
fn pre_break_notification_fires_once_when_notice_window_is_reached()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();
    let (ui, ui_commands) = recording_ui();

    run_config_with_inputs_and_ui(
        test_config(),
        backend,
        ui,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(4))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(1))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(1))),
        ],
    )?;

    assert!(received_commands(&commands).is_empty());
    assert_eq!(
        received_ui_commands(&ui_commands),
        vec![
            upcoming_break_status("short", Duration::from_secs(6)),
            upcoming_break_status("short", Duration::from_secs(5)),
            UiCommand::ShowPreBreakNotification(PreBreakNotification {
                break_name: String::from("short"),
                starts_after: Duration::from_secs(5),
            }),
            upcoming_break_status("short", Duration::from_secs(4)),
        ]
    );
    Ok(())
}

#[test]
fn pre_break_notification_updates_once_for_final_warning() -> Result<(), Box<dyn std::error::Error>>
{
    let (backend, commands) = test_backend();
    let (ui, ui_commands) = recording_ui();

    run_config_with_inputs_and_ui(
        test_config_with_after_active(Duration::from_mins(1)),
        backend,
        ui,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(29))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(1))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    assert_eq!(
        received_ui_commands(&ui_commands),
        vec![
            upcoming_break_status("short", Duration::from_secs(31)),
            upcoming_break_status("short", Duration::from_secs(30)),
            UiCommand::ShowPreBreakNotification(PreBreakNotification {
                break_name: String::from("short"),
                starts_after: Duration::from_secs(30),
            }),
            upcoming_break_status("short", Duration::from_secs(25)),
            upcoming_break_status("short", Duration::from_secs(20)),
            upcoming_break_status("short", Duration::from_secs(15)),
            upcoming_break_status("short", Duration::from_secs(10)),
            upcoming_break_status("short", Duration::from_secs(5)),
            UiCommand::ShowPreBreakNotification(PreBreakNotification {
                break_name: String::from("short"),
                starts_after: Duration::from_secs(5),
            }),
            UiCommand::ClearPreBreakNotification,
        ]
    );
    Ok(())
}

#[test]
fn pre_break_notification_uses_half_interval_lead_for_short_schedules()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();
    let (ui, ui_commands) = recording_ui();

    run_config_with_inputs_and_ui(
        test_config_with_after_active(Duration::from_secs(10)),
        backend,
        ui,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    assert_eq!(
        received_ui_commands(&ui_commands),
        vec![
            upcoming_break_status("short", Duration::from_secs(5)),
            UiCommand::ShowPreBreakNotification(PreBreakNotification {
                break_name: String::from("short"),
                starts_after: Duration::from_secs(5),
            }),
            UiCommand::ClearPreBreakNotification,
        ]
    );
    Ok(())
}

#[test]
fn pre_break_notification_uses_five_second_boundary_for_twenty_second_schedule()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();
    let (ui, ui_commands) = recording_ui();

    run_config_with_inputs_and_ui(
        test_config_with_after_active(Duration::from_secs(20)),
        backend,
        ui,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    assert_eq!(
        received_ui_commands(&ui_commands),
        vec![
            upcoming_break_status("short", Duration::from_secs(10)),
            UiCommand::ShowPreBreakNotification(PreBreakNotification {
                break_name: String::from("short"),
                starts_after: Duration::from_secs(10),
            }),
            upcoming_break_status("short", Duration::from_secs(5)),
            UiCommand::ShowPreBreakNotification(PreBreakNotification {
                break_name: String::from("short"),
                starts_after: Duration::from_secs(5),
            }),
            UiCommand::ClearPreBreakNotification,
        ]
    );
    Ok(())
}

#[test]
fn idle_reset_clears_pre_break_notification_and_resets_upcoming_break_display()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();
    let (ui, ui_commands) = recording_ui();

    run_config_with_inputs_and_ui(
        test_config_with_reset_after_idle(Some(Duration::from_secs(5))),
        backend,
        ui,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::IdleTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
        ],
    )?;

    assert!(received_commands(&commands).is_empty());
    assert_eq!(
        received_ui_commands(&ui_commands),
        vec![
            upcoming_break_status("short", Duration::from_secs(5)),
            UiCommand::ShowPreBreakNotification(PreBreakNotification {
                break_name: String::from("short"),
                starts_after: Duration::from_secs(5),
            }),
            UiCommand::ClearPreBreakNotification,
            upcoming_break_status("short", Duration::from_secs(10)),
            upcoming_break_status("short", Duration::from_secs(5)),
            UiCommand::ShowPreBreakNotification(PreBreakNotification {
                break_name: String::from("short"),
                starts_after: Duration::from_secs(5),
            }),
        ]
    );
    Ok(())
}

#[test]
fn pre_break_notification_is_skipped_when_active_time_immediately_starts_break()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();
    let (ui, ui_commands) = recording_ui();

    run_config_with_inputs_and_ui(
        test_config(),
        backend,
        ui,
        [backend_input(RuntimeEvent::ActiveTimeElapsed(
            Duration::from_secs(10),
        ))],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    assert!(received_ui_commands(&ui_commands).is_empty());
    Ok(())
}

#[test]
fn pre_break_notification_resets_after_break_finishes() -> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();
    let (ui, ui_commands) = recording_ui();

    run_config_with_inputs_and_ui(
        test_config(),
        backend,
        ui,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
            backend_input(RuntimeEvent::BreakFinished),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::FinishBreak { lock_after: false },
        ]
    );
    assert_eq!(
        received_ui_commands(&ui_commands),
        vec![
            upcoming_break_status("short", Duration::from_secs(5)),
            UiCommand::ShowPreBreakNotification(PreBreakNotification {
                break_name: String::from("short"),
                starts_after: Duration::from_secs(5),
            }),
            UiCommand::ClearPreBreakNotification,
            upcoming_break_status("long", Duration::from_secs(10)),
            UiCommand::UpdateManualBreakAvailability(manual_break_availability(&[
                ("long", true),
                ("short", false),
            ])),
            upcoming_break_status("long", Duration::from_secs(5)),
            UiCommand::ShowPreBreakNotification(PreBreakNotification {
                break_name: String::from("long"),
                starts_after: Duration::from_secs(5),
            }),
        ]
    );
    Ok(())
}

#[test]
fn manual_break_availability_updates_when_long_break_is_next()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();
    let (ui, ui_commands) = recording_ui();

    run_config_with_inputs_and_ui(
        test_config(),
        backend,
        ui,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            backend_input(RuntimeEvent::BreakFinished),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            backend_input(RuntimeEvent::BreakFinished),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::FinishBreak { lock_after: false },
            BackendCommand::StartBreak(scheduled_break("long", 2, 300)),
            BackendCommand::FinishBreak { lock_after: true },
        ]
    );
    assert_eq!(
        received_ui_commands(&ui_commands),
        vec![
            upcoming_break_status("long", Duration::from_secs(10)),
            UiCommand::UpdateManualBreakAvailability(manual_break_availability(&[
                ("long", true),
                ("short", false),
            ])),
            UiCommand::UpdateManualBreakAvailability(manual_break_availability(&[
                ("long", true),
                ("short", true),
            ])),
            upcoming_break_status("short", Duration::from_secs(10)),
        ]
    );
    Ok(())
}

#[test]
fn disabled_and_pending_states_suppress_pre_break_notifications()
-> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();
    let (ui, ui_commands) = recording_ui();

    run_config_with_inputs_and_ui(
        test_config(),
        backend,
        ui,
        [
            backend_input(RuntimeEvent::Disable(DisableRequest::UntilRestart)),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(100))),
            sync_input(remote_sync_event(SyncEvent::Enable)?),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    // The status line reflects the disable and the later re-enable, but no
    // pre-break notification is shown while disabled or pending.
    assert_eq!(
        received_ui_commands(&ui_commands),
        vec![
            UiCommand::UpdateStatus(StatusDisplay::DisabledUntilRestart),
            upcoming_break_status("short", Duration::from_secs(10)),
        ]
    );
    Ok(())
}

#[test]
fn timed_disable_shows_countdown_status_until_reenabled() -> Result<(), Box<dyn std::error::Error>>
{
    let (backend, _commands) = test_backend();
    let (ui, ui_commands) = recording_ui();

    run_config_with_steps_and_ui(
        test_config(),
        backend,
        ui,
        [
            backend_step(RuntimeEvent::Disable(DisableRequest::For(
                Duration::from_secs(3),
            ))),
            advance_clock(Duration::from_secs(1)),
            backend_step(RuntimeEvent::WallClockElapsed(Duration::from_secs(1))),
            advance_clock(Duration::from_secs(1)),
            backend_step(RuntimeEvent::WallClockElapsed(Duration::from_secs(1))),
            advance_clock(Duration::from_secs(1)),
            backend_step(RuntimeEvent::WallClockElapsed(Duration::from_secs(1))),
        ],
    )?;

    assert_eq!(
        received_ui_commands(&ui_commands),
        vec![
            UiCommand::UpdateStatus(StatusDisplay::DisabledFor(Duration::from_secs(3))),
            UiCommand::UpdateStatus(StatusDisplay::DisabledFor(Duration::from_secs(2))),
            UiCommand::UpdateStatus(StatusDisplay::DisabledFor(Duration::from_secs(1))),
            upcoming_break_status("short", Duration::from_secs(10)),
        ]
    );
    Ok(())
}

#[test]
fn synced_active_time_can_trigger_pre_break_notification() -> Result<(), Box<dyn std::error::Error>>
{
    let (backend, commands) = test_backend();
    let (ui, ui_commands) = recording_ui();

    run_config_with_inputs_and_ui(
        test_config(),
        backend,
        ui,
        [sync_input(remote_active_time(Duration::from_secs(5))?)],
    )?;

    assert!(received_commands(&commands).is_empty());
    assert_eq!(
        received_ui_commands(&ui_commands),
        vec![
            upcoming_break_status("short", Duration::from_secs(5)),
            UiCommand::ShowPreBreakNotification(PreBreakNotification {
                break_name: String::from("short"),
                starts_after: Duration::from_secs(5),
            }),
        ]
    );
    Ok(())
}

#[test]
fn ui_events_use_local_runtime_control_path() {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    assert_eq!(
        run_config_with_inputs_and_sync_broadcaster(
            test_config(),
            backend,
            &sync_broadcaster,
            [ui_input(RuntimeEvent::StartManualBreak(String::from(
                "long"
            )))],
        ),
        Ok(())
    );

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(manual_break("long", 300))]
    );
    assert_eq!(
        sync_broadcaster.events(),
        vec![broadcast_manual_break("long")]
    );
}

#[test]
fn disabled_scheduler_suppresses_remote_active_time() -> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config(),
        backend,
        [
            backend_input(RuntimeEvent::Disable(DisableRequest::UntilRestart)),
            sync_input(remote_active_time(Duration::from_secs(10))?),
        ],
    )?;

    assert!(received_commands(&commands).is_empty());
    Ok(())
}

#[test]
fn pending_break_suppresses_remote_active_time() -> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config(),
        backend,
        [
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            sync_input(remote_active_time(Duration::from_secs(10))?),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    Ok(())
}

fn run_config_with_backend(config: Config, backend: BackendActor) -> Result<(), ConfigError> {
    run_config_with_runtime_sync(config, backend, RuntimeSync::inactive())
}

fn run_config_with_sync_broadcaster(
    config: Config,
    backend: BackendActor,
    sync_broadcaster: &dyn SyncEventBroadcaster,
) -> Result<(), ConfigError> {
    run_config_with_runtime_sync(
        config,
        backend,
        RuntimeSync::new(None, None, sync_broadcaster),
    )
}

fn run_config_with_runtime_sync(
    config: Config,
    backend: BackendActor,
    sync_runtime: RuntimeSync<'_>,
) -> Result<(), ConfigError> {
    let schedule = BreakSchedule::try_from(config.breaks)?;
    run_with_event_sources(
        schedule,
        backend,
        sync_runtime,
        RuntimeUi::inactive(),
        Clock::Fixed(TEST_NOW_MS),
    );
    Ok(())
}

fn run_config_with_inputs(
    config: Config,
    backend: BackendActor,
    inputs: impl IntoIterator<Item = RuntimeInput>,
) -> Result<(), ConfigError> {
    run_config_with_inputs_and_sync_broadcaster(
        config,
        backend,
        &NOOP_SYNC_BROADCASTER_FOR_TEST,
        inputs,
    )
}

fn run_config_with_inputs_and_sync_broadcaster(
    config: Config,
    backend: BackendActor,
    sync_broadcaster: &dyn SyncEventBroadcaster,
    inputs: impl IntoIterator<Item = RuntimeInput>,
) -> Result<(), ConfigError> {
    run_config_with_inputs_and_sync_broadcaster_and_ui(
        config,
        backend,
        sync_broadcaster,
        RuntimeUi::inactive(),
        inputs,
    )
}

fn run_config_with_inputs_and_ui(
    config: Config,
    backend: BackendActor,
    ui: RuntimeUi,
    inputs: impl IntoIterator<Item = RuntimeInput>,
) -> Result<(), ConfigError> {
    run_config_with_inputs_and_sync_broadcaster_and_ui(
        config,
        backend,
        &NOOP_SYNC_BROADCASTER_FOR_TEST,
        ui,
        inputs,
    )
}

fn run_config_with_inputs_and_sync_broadcaster_and_ui(
    config: Config,
    backend: BackendActor,
    sync_broadcaster: &dyn SyncEventBroadcaster,
    ui: RuntimeUi,
    inputs: impl IntoIterator<Item = RuntimeInput>,
) -> Result<(), ConfigError> {
    run_config_with_inputs_sync_broadcaster_ui_and_local_peer(
        config,
        backend,
        sync_broadcaster,
        ui,
        None,
        inputs,
    )
}

fn run_config_with_inputs_sync_broadcaster_ui_and_local_peer(
    config: Config,
    backend: BackendActor,
    sync_broadcaster: &dyn SyncEventBroadcaster,
    ui: RuntimeUi,
    local_peer_id: Option<PeerId>,
    inputs: impl IntoIterator<Item = RuntimeInput>,
) -> Result<(), ConfigError> {
    let schedule = BreakSchedule::try_from(config.breaks)?;
    let sync_runtime = RuntimeSync::new(None, local_peer_id, sync_broadcaster);
    let mut daemon = DaemonRuntime::new(
        schedule,
        backend,
        sync_runtime,
        ui,
        Clock::Fixed(TEST_NOW_MS),
    );

    for input in inputs {
        if !daemon.handle_input(input) {
            break;
        }
    }

    Ok(())
}

fn run_config_with_steps_and_sync_broadcaster_and_ui(
    config: Config,
    backend: BackendActor,
    sync_broadcaster: &dyn SyncEventBroadcaster,
    ui: RuntimeUi,
    steps: impl IntoIterator<Item = TestRuntimeStep>,
) -> Result<(), ConfigError> {
    let schedule = BreakSchedule::try_from(config.breaks)?;
    let sync_runtime = RuntimeSync::new(None, None, sync_broadcaster);
    let now_ms = Arc::new(AtomicU64::new(TEST_NOW_MS));
    let mut daemon = DaemonRuntime::new(
        schedule,
        backend,
        sync_runtime,
        ui,
        Clock::Shared(Arc::clone(&now_ms)),
    );

    for step in steps {
        match step {
            TestRuntimeStep::Input(input) => {
                if !daemon.handle_input(input) {
                    break;
                }
            }
            TestRuntimeStep::AdvanceClock(duration) => {
                advance_test_clock(&now_ms, duration);
            }
        }
    }

    Ok(())
}

fn run_config_with_steps(
    config: Config,
    backend: BackendActor,
    steps: impl IntoIterator<Item = TestRuntimeStep>,
) -> Result<(), ConfigError> {
    run_config_with_steps_and_sync_broadcaster_and_ui(
        config,
        backend,
        &NOOP_SYNC_BROADCASTER_FOR_TEST,
        RuntimeUi::inactive(),
        steps,
    )
}

fn run_config_with_steps_and_ui(
    config: Config,
    backend: BackendActor,
    ui: RuntimeUi,
    steps: impl IntoIterator<Item = TestRuntimeStep>,
) -> Result<(), ConfigError> {
    run_config_with_steps_and_sync_broadcaster_and_ui(
        config,
        backend,
        &NOOP_SYNC_BROADCASTER_FOR_TEST,
        ui,
        steps,
    )
}

enum TestRuntimeStep {
    Input(RuntimeInput),
    AdvanceClock(Duration),
}

fn step_input(event: RuntimeInput) -> TestRuntimeStep {
    TestRuntimeStep::Input(event)
}

fn backend_step(event: RuntimeEvent) -> TestRuntimeStep {
    step_input(backend_input(event))
}

fn advance_clock(duration: Duration) -> TestRuntimeStep {
    TestRuntimeStep::AdvanceClock(duration)
}

fn advance_test_clock(now_ms: &AtomicU64, duration: Duration) {
    let delta_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    let now = now_ms.load(Ordering::Relaxed);

    now_ms.store(now.saturating_add(delta_ms), Ordering::Relaxed);
}

struct ScriptedBackend {
    actor: BackendActor,
    command_receiver: flume::Receiver<BackendCommand>,
}

impl ScriptedBackend {
    fn new(events: impl IntoIterator<Item = RuntimeEvent>) -> Self {
        let (command_sender, command_receiver) = flume::unbounded();
        let (event_sender, event_receiver) = flume::unbounded();
        let events = events.into_iter().collect::<Vec<_>>();
        let thread = thread::spawn(move || {
            for event in events {
                if event_sender.send(event).is_err() {
                    break;
                }
            }
        });

        Self {
            actor: BackendActor::new(command_sender, event_receiver, thread),
            command_receiver,
        }
    }

    fn into_parts(self) -> (BackendActor, flume::Receiver<BackendCommand>) {
        (self.actor, self.command_receiver)
    }
}

fn test_backend() -> (BackendActor, flume::Receiver<BackendCommand>) {
    let (command_sender, command_receiver) = flume::unbounded();
    let (_event_sender, event_receiver) = flume::unbounded();
    let thread = thread::spawn(|| {});

    (
        BackendActor::new(command_sender, event_receiver, thread),
        command_receiver,
    )
}

fn received_commands(receiver: &flume::Receiver<BackendCommand>) -> Vec<BackendCommand> {
    receiver.try_iter().collect()
}

fn recording_ui() -> (RuntimeUi, flume::Receiver<UiCommand>) {
    let (sender, receiver) = flume::unbounded();

    (RuntimeUi::with_command_sender(sender), receiver)
}

fn received_ui_commands(receiver: &flume::Receiver<UiCommand>) -> Vec<UiCommand> {
    receiver.try_iter().collect()
}

fn upcoming_break_status(break_name: &str, starts_after: Duration) -> UiCommand {
    UiCommand::UpdateStatus(StatusDisplay::UpcomingBreak {
        break_name: break_name.to_owned(),
        starts_after,
    })
}

fn peer_id() -> Result<PeerId, Box<dyn std::error::Error>> {
    Ok(PeerId::from_str("0102030405060708090a0b0c0d0e0f10")?)
}

/// A local peer id ordered above [`peer_id`], so an inbound peer wins a start
/// timestamp tie against it.
fn local_peer_higher() -> Result<PeerId, Box<dyn std::error::Error>> {
    Ok(PeerId::from_str("ff112233445566778899aabbccddeeff")?)
}

/// A local peer id ordered below [`peer_id`], so the local break wins a start
/// timestamp tie against an inbound peer.
fn local_peer_lower() -> Result<PeerId, Box<dyn std::error::Error>> {
    Ok(PeerId::from_str("00112233445566778899aabbccddeeff")?)
}

fn remote_active_time(elapsed: Duration) -> Result<SyncTransportEvent, Box<dyn std::error::Error>> {
    remote_sync_event(SyncEvent::ActiveTimeElapsed { elapsed })
}

fn remote_sync_event(event: SyncEvent) -> Result<SyncTransportEvent, Box<dyn std::error::Error>> {
    Ok(SyncTransportEvent::Domain {
        peer_id: peer_id()?,
        event,
    })
}

fn rejected_peer(peer_id: PeerId) -> SyncTransportEvent {
    SyncTransportEvent::PeerRejected {
        peer_id,
        reason: PeerRejectionReason::IncompatibleConfiguration,
    }
}

fn backend_input(event: RuntimeEvent) -> RuntimeInput {
    RuntimeInput::Backend(event)
}

fn ui_input(event: RuntimeEvent) -> RuntimeInput {
    RuntimeInput::Ui(event)
}

fn sync_input(event: SyncTransportEvent) -> RuntimeInput {
    RuntimeInput::SyncTransport(event)
}

#[derive(Default)]
struct RecordingSyncBroadcaster {
    events: RefCell<Vec<SyncEvent>>,
    directed_events: RefCell<Vec<(PeerId, SyncEvent)>>,
}

impl RecordingSyncBroadcaster {
    fn events(&self) -> Vec<SyncEvent> {
        self.events.borrow().clone()
    }

    fn directed_events(&self) -> Vec<(PeerId, SyncEvent)> {
        self.directed_events.borrow().clone()
    }
}

impl SyncEventBroadcaster for RecordingSyncBroadcaster {
    fn broadcast_sync_event(&self, event: SyncEvent) -> Result<usize, SyncTransportError> {
        self.events.borrow_mut().push(event);
        Ok(1)
    }

    fn send_sync_event(
        &self,
        peer_id: PeerId,
        event: SyncEvent,
    ) -> Result<bool, SyncTransportError> {
        self.directed_events.borrow_mut().push((peer_id, event));
        Ok(true)
    }
}

struct NoopSyncBroadcasterForTest;

impl SyncEventBroadcaster for NoopSyncBroadcasterForTest {
    fn broadcast_sync_event(&self, _event: SyncEvent) -> Result<usize, SyncTransportError> {
        Ok(0)
    }

    fn send_sync_event(
        &self,
        _peer_id: PeerId,
        _event: SyncEvent,
    ) -> Result<bool, SyncTransportError> {
        Ok(false)
    }
}

static NOOP_SYNC_BROADCASTER_FOR_TEST: NoopSyncBroadcasterForTest = NoopSyncBroadcasterForTest;

fn test_config() -> Config {
    Config {
        breaks: Breaks {
            after_active: Duration::from_secs(10),
            reset_after_idle: Some(Duration::from_mins(5)),
            reset_count_after_idle: Some(Duration::from_hours(1)),
            types: [
                (
                    String::from("short"),
                    break_type(1, 20, "Rest your eyes", false),
                ),
                (
                    String::from("long"),
                    break_type(2, 300, "Take a longer break", true),
                ),
            ]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        },
        disable_presets: vec![Duration::from_secs(30)],
        lock: LockConfig::default(),
        startup: StartupConfig::default(),
        sync: SyncConfig::default(),
    }
}

fn test_config_with_reset_after_idle(reset_after_idle: Option<Duration>) -> Config {
    let mut config = test_config();
    config.breaks.reset_after_idle = reset_after_idle;
    config
}

fn test_config_with_reset_count_after_idle(reset_count_after_idle: Option<Duration>) -> Config {
    let mut config = test_config();
    config.breaks.reset_count_after_idle = reset_count_after_idle;
    config
}

fn test_config_with_after_active(after_active: Duration) -> Config {
    let mut config = test_config();
    config.breaks.after_active = after_active;
    config
}

fn break_type(
    interval: usize,
    duration_secs: u64,
    message: &str,
    autolock: bool,
) -> BreakTypeConfig {
    BreakTypeConfig {
        interval,
        duration: Duration::from_secs(duration_secs),
        messages: vec![message.to_owned()],
        autolock,
    }
}

fn break_message(name: &str) -> String {
    match name {
        "long" => String::from("Take a longer break"),
        _ => String::from("Rest your eyes"),
    }
}

fn broadcast_scheduled_break(name: &str, slot: usize) -> SyncEvent {
    let long_slot = if name == "long" { slot } else { 0 };

    broadcast_scheduled_break_with_slots(name, slot, slot, long_slot)
}

fn broadcast_scheduled_break_with_slots(
    name: &str,
    slot: usize,
    short_slot: usize,
    long_slot: usize,
) -> SyncEvent {
    SyncEvent::BreakStarted {
        name: name.to_owned(),
        message: break_message(name),
        started_at_ms: TEST_NOW_MS,
        origin: SyncBreakOrigin::Scheduled { slot },
        position: sync_position(slot, Duration::ZERO, short_slot, long_slot),
    }
}

fn broadcast_manual_break(name: &str) -> SyncEvent {
    broadcast_manual_break_at(name, 0)
}

fn broadcast_manual_break_at(name: &str, slot: usize) -> SyncEvent {
    SyncEvent::BreakStarted {
        name: name.to_owned(),
        message: break_message(name),
        started_at_ms: TEST_NOW_MS,
        origin: SyncBreakOrigin::Manual,
        position: manual_sync_position(name, slot),
    }
}

fn incoming_break(
    name: &str,
    message: &str,
    started_at_ms: u64,
) -> Result<SyncTransportEvent, Box<dyn std::error::Error>> {
    incoming_scheduled_break(name, message, started_at_ms, 1)
}

fn incoming_scheduled_break(
    name: &str,
    message: &str,
    started_at_ms: u64,
    slot: usize,
) -> Result<SyncTransportEvent, Box<dyn std::error::Error>> {
    remote_sync_event(SyncEvent::BreakStarted {
        name: name.to_owned(),
        message: message.to_owned(),
        started_at_ms,
        origin: SyncBreakOrigin::Scheduled { slot },
        position: scheduled_sync_position(name, slot),
    })
}

fn incoming_manual_break(
    name: &str,
    message: &str,
    started_at_ms: u64,
) -> Result<SyncTransportEvent, Box<dyn std::error::Error>> {
    incoming_manual_break_at(name, message, started_at_ms, 0)
}

fn incoming_manual_break_at(
    name: &str,
    message: &str,
    started_at_ms: u64,
    slot: usize,
) -> Result<SyncTransportEvent, Box<dyn std::error::Error>> {
    remote_sync_event(SyncEvent::BreakStarted {
        name: name.to_owned(),
        message: message.to_owned(),
        started_at_ms,
        origin: SyncBreakOrigin::Manual,
        position: manual_sync_position(name, slot),
    })
}

fn scheduled_sync_position(name: &str, slot: usize) -> SyncSchedulerPosition {
    let long_slot = if name == "long" { slot } else { 0 };

    sync_position(slot, Duration::ZERO, slot, long_slot)
}

fn manual_sync_position(name: &str, slot: usize) -> SyncSchedulerPosition {
    let (short_slot, long_slot) = match name {
        "long" => (slot, slot),
        _ => (slot, 0),
    };

    sync_position(slot, Duration::ZERO, short_slot, long_slot)
}

fn sync_position(
    slot: usize,
    active_elapsed: Duration,
    short_slot: usize,
    long_slot: usize,
) -> SyncSchedulerPosition {
    SyncSchedulerPosition {
        slot,
        active_elapsed,
        last_satisfied_slots: test_last_satisfied_slots(short_slot, long_slot),
    }
}

fn test_last_satisfied_slots(short_slot: usize, long_slot: usize) -> BTreeMap<String, usize> {
    [
        (String::from("short"), short_slot),
        (String::from("long"), long_slot),
    ]
    .into_iter()
    .collect()
}

fn manual_break_availability(items: &[(&str, bool)]) -> BTreeMap<String, bool> {
    items
        .iter()
        .map(|(name, available)| ((*name).to_owned(), *available))
        .collect()
}

fn scheduled_break(name: &str, slot: usize, duration_secs: u64) -> ScheduledBreak {
    ScheduledBreak {
        name: name.to_owned(),
        origin: BreakOrigin::Scheduled { slot },
        duration: Duration::from_secs(duration_secs),
        message: break_message(name),
        autolock: name == "long",
    }
}

fn manual_break(name: &str, duration_secs: u64) -> ScheduledBreak {
    ScheduledBreak {
        name: name.to_owned(),
        origin: BreakOrigin::Manual,
        duration: Duration::from_secs(duration_secs),
        message: break_message(name),
        autolock: name == "long",
    }
}
