use super::run_with_backend;
use crate::backend::{Backend, BackendCommand, DisableRequest, RuntimeEvent};
use crate::config::{BreakTypeConfig, Breaks, Config, ConfigError, LockConfig};
use crate::scheduler::ScheduledBreak;
use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

#[test]
fn shutdown_exits_cleanly_after_scheduler_setup() {
    let mut backend = ScriptedBackend::new([RuntimeEvent::Shutdown]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert!(backend.commands.is_empty());
}

#[test]
fn active_time_event_starts_expected_configured_break() {
    let mut backend = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
    ]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert_eq!(
        backend.commands,
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
}

#[test]
fn break_finished_allows_next_scheduled_break_to_advance() {
    let mut backend = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
    ]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert_eq!(
        backend.commands,
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::ClearBreak,
            BackendCommand::StartBreak(scheduled_break("long", 2, 300))
        ]
    );
}

#[test]
fn autolock_break_completion_requests_local_lock() {
    let mut backend = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::Shutdown,
    ]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert_eq!(
        backend.commands,
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::ClearBreak,
            BackendCommand::StartBreak(scheduled_break("long", 2, 300)),
            BackendCommand::ClearBreak,
            BackendCommand::RequestLock
        ]
    );
}

#[test]
fn disable_clears_pending_backend_break_without_locking() {
    let mut backend = ScriptedBackend::new([
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::Disable(DisableRequest::For(Duration::from_secs(30))),
        RuntimeEvent::Shutdown,
    ]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert_eq!(
        backend.commands,
        vec![
            BackendCommand::StartBreak(scheduled_break("short", 1, 20)),
            BackendCommand::ClearBreak
        ]
    );
}

#[test]
fn finite_disable_suppresses_active_time_and_reenables_after_elapsed() {
    let mut backend = ScriptedBackend::new([
        RuntimeEvent::Disable(DisableRequest::For(Duration::from_secs(30))),
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(100)),
        RuntimeEvent::WallClockElapsed(Duration::from_secs(29)),
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::WallClockElapsed(Duration::from_secs(1)),
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
    ]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert_eq!(
        backend.commands,
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
}

#[test]
fn disable_until_restart_stays_disabled_until_explicit_enable() {
    let mut backend = ScriptedBackend::new([
        RuntimeEvent::Disable(DisableRequest::UntilRestart),
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(100)),
        RuntimeEvent::WallClockElapsed(Duration::from_secs(60 * 60)),
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(100)),
        RuntimeEvent::Enable,
        RuntimeEvent::ActiveTimeElapsed(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
    ]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert_eq!(
        backend.commands,
        vec![BackendCommand::StartBreak(scheduled_break("short", 1, 20))]
    );
}

#[test]
fn scheduler_setup_error_is_returned() {
    let mut config = test_config();
    config.breaks.types.clear();
    let mut backend = ScriptedBackend::new([RuntimeEvent::Shutdown]);

    assert_eq!(
        run_with_backend(config, &mut backend),
        Err(ConfigError::EmptyBreakTypes)
    );
}

struct ScriptedBackend {
    events: VecDeque<RuntimeEvent>,
    commands: Vec<BackendCommand>,
}

impl ScriptedBackend {
    fn new(events: impl IntoIterator<Item = RuntimeEvent>) -> Self {
        Self {
            events: events.into_iter().collect(),
            commands: Vec::new(),
        }
    }
}

impl Backend for ScriptedBackend {
    fn next_event(&mut self) -> RuntimeEvent {
        match self.events.pop_front() {
            Some(event) => event,
            None => RuntimeEvent::Shutdown,
        }
    }

    fn handle_command(&mut self, command: BackendCommand) {
        self.commands.push(command);
    }
}

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
        slot,
        duration: Duration::from_secs(duration_secs),
        messages: vec![match name {
            "long" => String::from("Take a longer break"),
            _ => String::from("Rest your eyes"),
        }],
        autolock: name == "long",
    }
}
