use super::{
    DaemonRuntime, RuntimeInput, RuntimeSync, SyncEventBroadcaster, run_with_event_sources,
};
use crate::backend::{BackendActor, BackendCommand, DisableRequest, RuntimeEvent};
use crate::config::{BreakTypeConfig, Breaks, Config, ConfigError, LockConfig, SyncConfig};
use crate::scheduler::{BreakOrigin, BreakSchedule, ScheduledBreak};
use crate::sync_protocol::{PeerId, SyncEvent};
use crate::sync_transport::{PeerRejectionReason, SyncTransportError, SyncTransportEvent};
use crate::ui::{PreBreakNotification, RuntimeUi, UiCommand, UiNotification};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::thread;
use std::time::Duration;

#[test]
fn shutdown_exits_cleanly_after_scheduler_setup() {
    let (backend, commands) = ScriptedBackend::new([RuntimeEvent::Shutdown]).into_parts();

    assert_eq!(run_config_with_backend(test_config(), backend), Ok(()));
    assert!(received_commands(&commands).is_empty());
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
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::LockAfterCurrentBreak,
        RuntimeEvent::Disable(DisableRequest::For(Duration::from_secs(30))),
        RuntimeEvent::WallClockElapsed(Duration::from_secs(30)),
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
            BackendCommand::ClearBreak,
            BackendCommand::StartBreak(scheduled_break("short", 2, 20)),
            BackendCommand::FinishBreak { lock_after: false }
        ]
    );
}

#[test]
fn finite_disable_suppresses_active_time_and_reenables_after_elapsed() {
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::Disable(DisableRequest::For(Duration::from_secs(30))),
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(100)),
        RuntimeEvent::WallClockElapsed(Duration::from_secs(29)),
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::WallClockElapsed(Duration::from_secs(1)),
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
fn disable_until_restart_stays_disabled_until_explicit_enable() {
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::Disable(DisableRequest::UntilRestart),
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(100)),
        RuntimeEvent::WallClockElapsed(Duration::from_hours(1)),
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(100)),
        RuntimeEvent::Enable,
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
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::Disable(DisableRequest::For(Duration::from_secs(30))),
        RuntimeEvent::StartManualBreak(String::from("short")),
        RuntimeEvent::WallClockElapsed(Duration::from_secs(30)),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
    ])
    .into_parts();

    assert_eq!(run_config_with_backend(test_config(), backend), Ok(()));
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
            summary: String::from("Resteyes sync peer rejected"),
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
            SyncEvent::BreakStarted {
                name: String::from("short"),
            },
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
        vec![SyncEvent::BreakStarted {
            name: String::from("long"),
        }]
    );
}

#[test]
fn local_disable_and_enable_events_are_broadcast_to_sync_peers() {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = ScriptedBackend::new([
        RuntimeEvent::Disable(DisableRequest::For(Duration::from_secs(30))),
        RuntimeEvent::Enable,
        RuntimeEvent::Disable(DisableRequest::UntilRestart),
        RuntimeEvent::Enable,
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
            SyncEvent::Enable,
            SyncEvent::DisableUntilRestart,
            SyncEvent::Enable,
        ]
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
            SyncEvent::BreakStarted {
                name: String::from("short"),
            },
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
            SyncEvent::BreakStarted {
                name: String::from("short"),
            },
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
        [sync_input(remote_active_time(Duration::from_secs(10))?)],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    Ok(())
}

#[test]
fn local_and_remote_active_time_are_additive() -> Result<(), Box<dyn std::error::Error>> {
    let (backend, commands) = test_backend();

    run_config_with_inputs(
        test_config(),
        backend,
        [
            sync_input(remote_active_time(Duration::from_secs(4))?),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(6))),
        ],
    )?;

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
        [sync_input(remote_sync_event(SyncEvent::BreakStarted {
            name: String::from("short"),
        })?)],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(manual_break("short", 20))]
    );
    assert!(sync_broadcaster.events().is_empty());
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
        [sync_input(remote_sync_event(SyncEvent::BreakStarted {
            name: String::from("missing"),
        })?)],
    )?;

    assert!(received_commands(&commands).is_empty());
    assert!(sync_broadcaster.events().is_empty());
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
            SyncEvent::BreakStarted {
                name: String::from("short"),
            },
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
            SyncEvent::BreakStarted {
                name: String::from("short"),
            },
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
            SyncEvent::BreakStarted {
                name: String::from("short"),
            },
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
fn idle_reset_is_not_broadcast_to_sync_peers() {
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    let (backend, commands) = test_backend();

    assert_eq!(
        run_config_with_inputs_and_sync_broadcaster(
            test_config_with_reset_after_idle(Some(Duration::from_secs(5))),
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
        vec![SyncEvent::ActiveTimeElapsed {
            elapsed: Duration::from_secs(4),
        }]
    );
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
            UiCommand::UpdateActiveTime(Duration::from_secs(4)),
            UiCommand::UpdateActiveTime(Duration::from_secs(5)),
            UiCommand::ShowPreBreakNotification(PreBreakNotification {
                break_name: String::from("short"),
                starts_after: Duration::from_secs(5),
            }),
            UiCommand::UpdateActiveTime(Duration::from_secs(6)),
        ]
    );
    Ok(())
}

#[test]
fn idle_reset_clears_pre_break_notification_and_active_time_display()
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
            UiCommand::UpdateActiveTime(Duration::from_secs(5)),
            UiCommand::ShowPreBreakNotification(PreBreakNotification {
                break_name: String::from("short"),
                starts_after: Duration::from_secs(5),
            }),
            UiCommand::UpdateActiveTime(Duration::ZERO),
            UiCommand::UpdateActiveTime(Duration::from_secs(5)),
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
            UiCommand::UpdateActiveTime(Duration::from_secs(5)),
            UiCommand::ShowPreBreakNotification(PreBreakNotification {
                break_name: String::from("short"),
                starts_after: Duration::from_secs(5),
            }),
            UiCommand::UpdateActiveTime(Duration::ZERO),
            UiCommand::UpdateActiveTime(Duration::from_secs(5)),
            UiCommand::ShowPreBreakNotification(PreBreakNotification {
                break_name: String::from("long"),
                starts_after: Duration::from_secs(5),
            }),
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
            backend_input(RuntimeEvent::Enable),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10))),
            backend_input(RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(5))),
        ],
    )?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    assert!(received_ui_commands(&ui_commands).is_empty());
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
            UiCommand::UpdateActiveTime(Duration::from_secs(5)),
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
        vec![SyncEvent::BreakStarted {
            name: String::from("long"),
        }]
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
    run_config_with_runtime_sync(config, backend, RuntimeSync::new(None, sync_broadcaster))
}

fn run_config_with_runtime_sync(
    config: Config,
    backend: BackendActor,
    sync_runtime: RuntimeSync<'_>,
) -> Result<(), ConfigError> {
    let schedule = BreakSchedule::try_from(config.breaks)?;
    run_with_event_sources(schedule, backend, sync_runtime, RuntimeUi::inactive());
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
    let schedule = BreakSchedule::try_from(config.breaks)?;
    let sync_runtime = RuntimeSync::new(None, sync_broadcaster);
    let mut daemon = DaemonRuntime::new(schedule, backend, sync_runtime, ui);

    for input in inputs {
        if !daemon.handle_input(input) {
            break;
        }
    }

    Ok(())
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

fn peer_id() -> Result<PeerId, Box<dyn std::error::Error>> {
    Ok(PeerId::from_str("0102030405060708090a0b0c0d0e0f10")?)
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
}

impl RecordingSyncBroadcaster {
    fn events(&self) -> Vec<SyncEvent> {
        self.events.borrow().clone()
    }
}

impl SyncEventBroadcaster for RecordingSyncBroadcaster {
    fn broadcast_sync_event(&self, event: SyncEvent) -> Result<usize, SyncTransportError> {
        self.events.borrow_mut().push(event);
        Ok(1)
    }
}

struct NoopSyncBroadcasterForTest;

impl SyncEventBroadcaster for NoopSyncBroadcasterForTest {
    fn broadcast_sync_event(&self, _event: SyncEvent) -> Result<usize, SyncTransportError> {
        Ok(0)
    }
}

static NOOP_SYNC_BROADCASTER_FOR_TEST: NoopSyncBroadcasterForTest = NoopSyncBroadcasterForTest;

fn test_config() -> Config {
    Config {
        breaks: Breaks {
            after_active: Duration::from_secs(10),
            reset_after_idle: Some(Duration::from_mins(5)),
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
        sync: SyncConfig::default(),
    }
}

fn test_config_with_reset_after_idle(reset_after_idle: Option<Duration>) -> Config {
    let mut config = test_config();
    config.breaks.reset_after_idle = reset_after_idle;
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

fn scheduled_break(name: &str, slot: usize, duration_secs: u64) -> ScheduledBreak {
    ScheduledBreak {
        name: name.to_owned(),
        origin: BreakOrigin::Scheduled { slot },
        duration: Duration::from_secs(duration_secs),
        messages: vec![match name {
            "long" => String::from("Take a longer break"),
            _ => String::from("Rest your eyes"),
        }],
        autolock: name == "long",
    }
}

fn manual_break(name: &str, duration_secs: u64) -> ScheduledBreak {
    ScheduledBreak {
        name: name.to_owned(),
        origin: BreakOrigin::Manual,
        duration: Duration::from_secs(duration_secs),
        messages: vec![match name {
            "long" => String::from("Take a longer break"),
            _ => String::from("Rest your eyes"),
        }],
        autolock: name == "long",
    }
}
