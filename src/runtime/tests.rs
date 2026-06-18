use super::run_with_backend;
use crate::backend::{Backend, BackendEvent};
use crate::config::{BreakTypeConfig, Breaks, Config, ConfigError};
use crate::scheduler::ScheduledBreak;
use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

#[test]
fn shutdown_exits_cleanly_after_scheduler_setup() {
    let mut backend = ScriptedBackend::new([BackendEvent::Shutdown]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert!(backend.started_breaks.is_empty());
}

#[test]
fn active_time_event_starts_expected_configured_break() {
    let mut backend = ScriptedBackend::new([
        BackendEvent::Active(Duration::from_secs(10)),
        BackendEvent::Shutdown,
    ]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert_eq!(
        backend.started_breaks,
        vec![scheduled_break("short", 1, 20)]
    );
}

#[test]
fn break_finished_allows_next_scheduled_break_to_advance() {
    let mut backend = ScriptedBackend::new([
        BackendEvent::Active(Duration::from_secs(10)),
        BackendEvent::BreakFinished,
        BackendEvent::Active(Duration::from_secs(10)),
        BackendEvent::Shutdown,
    ]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert_eq!(
        backend.started_breaks,
        vec![
            scheduled_break("short", 1, 20),
            scheduled_break("long", 2, 300)
        ]
    );
    assert_eq!(backend.cleared_breaks, 1);
    assert_eq!(backend.lock_requests, 0);
}

#[test]
fn autolock_break_completion_requests_local_lock() {
    let mut backend = ScriptedBackend::new([
        BackendEvent::Active(Duration::from_secs(10)),
        BackendEvent::BreakFinished,
        BackendEvent::Active(Duration::from_secs(10)),
        BackendEvent::BreakFinished,
        BackendEvent::Shutdown,
    ]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert_eq!(
        backend.started_breaks,
        vec![
            scheduled_break("short", 1, 20),
            scheduled_break("long", 2, 300)
        ]
    );
    assert_eq!(backend.cleared_breaks, 2);
    assert_eq!(backend.lock_requests, 1);
}

#[test]
fn disable_clears_pending_backend_break_without_locking() {
    let mut backend = ScriptedBackend::new([
        BackendEvent::Active(Duration::from_secs(10)),
        BackendEvent::DisableFor(Duration::from_secs(30)),
        BackendEvent::Shutdown,
    ]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert_eq!(
        backend.started_breaks,
        vec![scheduled_break("short", 1, 20)]
    );
    assert_eq!(backend.cleared_breaks, 1);
    assert_eq!(backend.lock_requests, 0);
}

#[test]
fn finite_disable_suppresses_active_time_and_reenables_after_elapsed() {
    let mut backend = ScriptedBackend::new([
        BackendEvent::DisableFor(Duration::from_secs(30)),
        BackendEvent::Active(Duration::from_secs(100)),
        BackendEvent::WallClock(Duration::from_secs(29)),
        BackendEvent::Active(Duration::from_secs(10)),
        BackendEvent::WallClock(Duration::from_secs(1)),
        BackendEvent::Active(Duration::from_secs(10)),
        BackendEvent::Shutdown,
    ]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert_eq!(
        backend.started_breaks,
        vec![scheduled_break("short", 1, 20)]
    );
}

#[test]
fn disable_until_restart_stays_disabled_until_explicit_enable() {
    let mut backend = ScriptedBackend::new([
        BackendEvent::DisableUntilRestart,
        BackendEvent::Active(Duration::from_secs(100)),
        BackendEvent::WallClock(Duration::from_secs(60 * 60)),
        BackendEvent::Active(Duration::from_secs(100)),
        BackendEvent::Enable,
        BackendEvent::Active(Duration::from_secs(10)),
        BackendEvent::Shutdown,
    ]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert_eq!(
        backend.started_breaks,
        vec![scheduled_break("short", 1, 20)]
    );
}

#[test]
fn scheduler_setup_error_is_returned() {
    let mut config = test_config();
    config.breaks.types.clear();
    let mut backend = ScriptedBackend::new([BackendEvent::Shutdown]);

    assert_eq!(
        run_with_backend(config, &mut backend),
        Err(ConfigError::EmptyBreakTypes)
    );
}

struct ScriptedBackend {
    events: VecDeque<BackendEvent>,
    started_breaks: Vec<ScheduledBreak>,
    cleared_breaks: usize,
    lock_requests: usize,
}

impl ScriptedBackend {
    fn new(events: impl IntoIterator<Item = BackendEvent>) -> Self {
        Self {
            events: events.into_iter().collect(),
            started_breaks: Vec::new(),
            cleared_breaks: 0,
            lock_requests: 0,
        }
    }
}

impl Backend for ScriptedBackend {
    fn next_event(&mut self) -> BackendEvent {
        match self.events.pop_front() {
            Some(event) => event,
            None => BackendEvent::Shutdown,
        }
    }

    fn start_break(&mut self, scheduled_break: ScheduledBreak) {
        self.started_breaks.push(scheduled_break);
    }

    fn clear_break(&mut self) {
        self.cleared_breaks += 1;
    }

    fn request_lock(&mut self) {
        self.lock_requests += 1;
    }
}

fn test_config() -> Config {
    Config {
        breaks: Breaks {
            after_active: Duration::from_secs(10),
            types: [
                (
                    String::from("short"),
                    BreakTypeConfig {
                        interval: 1,
                        duration: Duration::from_secs(20),
                        messages: vec![String::from("Rest your eyes")],
                        autolock: false,
                    },
                ),
                (
                    String::from("long"),
                    BreakTypeConfig {
                        interval: 2,
                        duration: Duration::from_secs(300),
                        messages: vec![String::from("Take a longer break")],
                        autolock: true,
                    },
                ),
            ]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        },
        disable_presets: vec![Duration::from_secs(30)],
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
