use super::{RuntimeSync, SyncEventBroadcaster, run_with_event_sources};
use crate::backend::{BackendActor, BackendCommand, DisableRequest, RuntimeEvent};
use crate::config::{BreakTypeConfig, Breaks, Config, ConfigError, LockConfig, SyncConfig};
use crate::scheduler::{BreakOrigin, BreakSchedule, ScheduledBreak};
use crate::sync_protocol::{PeerId, SyncEvent};
use crate::sync_transport::{SyncTransportError, SyncTransportEvent};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
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
        RuntimeEvent::WallClockElapsed(Duration::from_secs(60 * 60)),
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
fn sync_transport_events_are_consumed_without_scheduler_behavior()
-> Result<(), Box<dyn std::error::Error>> {
    let (sync_sender, sync_receiver) = flume::unbounded();
    sync_sender.send(SyncTransportEvent::PeerAuthenticated(peer_id()?))?;
    drop(sync_sender);
    let (backend, commands) =
        ScriptedBackend::new_with_delay([RuntimeEvent::Shutdown], Duration::from_millis(25))
            .into_parts();

    run_config_with_event_sources(test_config(), backend, Some(sync_receiver.clone()))?;

    assert!(sync_receiver.is_empty());
    assert!(received_commands(&commands).is_empty());
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
fn remote_active_time_event_starts_expected_configured_break()
-> Result<(), Box<dyn std::error::Error>> {
    let (sync_sender, sync_receiver) = flume::unbounded();
    sync_sender.send(remote_active_time(Duration::from_secs(10))?)?;
    drop(sync_sender);
    let (backend, commands) =
        ScriptedBackend::new_with_delay([RuntimeEvent::Shutdown], Duration::from_millis(25))
            .into_parts();

    run_config_with_event_sources(test_config(), backend, Some(sync_receiver))?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    Ok(())
}

#[test]
fn local_and_remote_active_time_are_additive() -> Result<(), Box<dyn std::error::Error>> {
    let (sync_sender, sync_receiver) = flume::unbounded();
    sync_sender.send(remote_active_time(Duration::from_secs(4))?)?;
    drop(sync_sender);
    let (backend, commands) = ScriptedBackend::new_with_delay(
        [
            RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(6)),
            RuntimeEvent::Shutdown,
        ],
        Duration::from_millis(25),
    )
    .into_parts();

    run_config_with_event_sources(test_config(), backend, Some(sync_receiver))?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    Ok(())
}

#[test]
fn remote_active_time_is_not_rebroadcast() -> Result<(), Box<dyn std::error::Error>> {
    let (sync_sender, sync_receiver) = flume::unbounded();
    let sync_broadcaster = RecordingSyncBroadcaster::default();
    sync_sender.send(remote_active_time(Duration::from_secs(1))?)?;
    drop(sync_sender);
    let (backend, commands) =
        ScriptedBackend::new_with_delay([RuntimeEvent::Shutdown], Duration::from_millis(25))
            .into_parts();

    run_config_with_runtime_sync(
        test_config(),
        backend,
        RuntimeSync::new(Some(sync_receiver), &sync_broadcaster),
    )?;

    assert!(received_commands(&commands).is_empty());
    assert!(sync_broadcaster.events().is_empty());
    Ok(())
}

#[test]
fn disabled_scheduler_suppresses_remote_active_time() -> Result<(), Box<dyn std::error::Error>> {
    let sync_receiver = delayed_sync_event(
        remote_active_time(Duration::from_secs(10))?,
        Duration::from_millis(25),
    );
    let (backend, commands) = ScriptedBackend::new_with_event_delay(
        [
            RuntimeEvent::Disable(DisableRequest::UntilRestart),
            RuntimeEvent::Shutdown,
        ],
        Duration::from_millis(50),
    )
    .into_parts();

    run_config_with_event_sources(test_config(), backend, Some(sync_receiver))?;

    assert!(received_commands(&commands).is_empty());
    Ok(())
}

#[test]
fn pending_break_suppresses_remote_active_time() -> Result<(), Box<dyn std::error::Error>> {
    let sync_receiver = delayed_sync_event(
        remote_active_time(Duration::from_secs(10))?,
        Duration::from_millis(25),
    );
    let (backend, commands) = ScriptedBackend::new_with_event_delay(
        [
            RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
            RuntimeEvent::Shutdown,
        ],
        Duration::from_millis(50),
    )
    .into_parts();

    run_config_with_event_sources(test_config(), backend, Some(sync_receiver))?;

    assert_eq!(
        received_commands(&commands),
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
    Ok(())
}

fn run_config_with_backend(config: Config, backend: BackendActor) -> Result<(), ConfigError> {
    run_config_with_runtime_sync(config, backend, RuntimeSync::inactive())
}

fn run_config_with_event_sources(
    config: Config,
    backend: BackendActor,
    sync_event_receiver: Option<flume::Receiver<SyncTransportEvent>>,
) -> Result<(), ConfigError> {
    run_config_with_runtime_sync(
        config,
        backend,
        RuntimeSync::new(sync_event_receiver, &NOOP_SYNC_BROADCASTER_FOR_TEST),
    )
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
    run_with_event_sources(schedule, backend, sync_runtime);
    Ok(())
}

struct ScriptedBackend {
    actor: BackendActor,
    command_receiver: flume::Receiver<BackendCommand>,
}

impl ScriptedBackend {
    fn new(events: impl IntoIterator<Item = RuntimeEvent>) -> Self {
        Self::new_with_delay(events, Duration::ZERO)
    }

    fn new_with_delay(events: impl IntoIterator<Item = RuntimeEvent>, delay: Duration) -> Self {
        Self::spawn(events, delay, Duration::ZERO)
    }

    fn new_with_event_delay(
        events: impl IntoIterator<Item = RuntimeEvent>,
        event_delay: Duration,
    ) -> Self {
        Self::spawn(events, Duration::ZERO, event_delay)
    }

    fn spawn(
        events: impl IntoIterator<Item = RuntimeEvent>,
        initial_delay: Duration,
        event_delay: Duration,
    ) -> Self {
        let (command_sender, command_receiver) = flume::unbounded();
        let (event_sender, event_receiver) = flume::unbounded();
        let events = events.into_iter().collect::<VecDeque<_>>();
        let thread = thread::spawn(move || {
            if !initial_delay.is_zero() {
                thread::sleep(initial_delay);
            }

            for (index, event) in events.into_iter().enumerate() {
                if index > 0 && !event_delay.is_zero() {
                    thread::sleep(event_delay);
                }

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

fn received_commands(receiver: &flume::Receiver<BackendCommand>) -> Vec<BackendCommand> {
    receiver.try_iter().collect()
}

fn peer_id() -> Result<PeerId, Box<dyn std::error::Error>> {
    Ok(PeerId::from_str("0102030405060708090a0b0c0d0e0f10")?)
}

fn remote_active_time(elapsed: Duration) -> Result<SyncTransportEvent, Box<dyn std::error::Error>> {
    Ok(SyncTransportEvent::Domain {
        peer_id: peer_id()?,
        event: SyncEvent::ActiveTimeElapsed { elapsed },
    })
}

fn delayed_sync_event(
    event: SyncTransportEvent,
    delay: Duration,
) -> flume::Receiver<SyncTransportEvent> {
    let (sender, receiver) = flume::unbounded();
    thread::spawn(move || {
        thread::sleep(delay);
        _ = sender.send(event);
    });
    receiver
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
