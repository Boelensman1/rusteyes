use super::{DaemonBackend, RuntimeEvent, run_with_backend};
use crate::config::{BreakTypeConfig, Breaks, Config, ConfigError};
use crate::scheduler::ScheduledBreak;
use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

#[test]
fn shutdown_exits_cleanly_after_scheduler_setup() {
    let mut backend = ScriptedBackend::new([RuntimeEvent::Shutdown]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert!(backend.started_breaks.is_empty());
}

#[test]
fn active_time_event_starts_expected_configured_break() {
    let mut backend = ScriptedBackend::new([
        RuntimeEvent::Active(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
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
        RuntimeEvent::Active(Duration::from_secs(10)),
        RuntimeEvent::BreakFinished,
        RuntimeEvent::Active(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
    ]);

    assert_eq!(run_with_backend(test_config(), &mut backend), Ok(()));
    assert_eq!(
        backend.started_breaks,
        vec![
            scheduled_break("short", 1, 20),
            scheduled_break("long", 2, 300)
        ]
    );
}

#[test]
fn finite_disable_suppresses_active_time_and_reenables_after_elapsed() {
    let mut backend = ScriptedBackend::new([
        RuntimeEvent::DisableFor(Duration::from_secs(30)),
        RuntimeEvent::Active(Duration::from_secs(100)),
        RuntimeEvent::WallClock(Duration::from_secs(29)),
        RuntimeEvent::Active(Duration::from_secs(10)),
        RuntimeEvent::WallClock(Duration::from_secs(1)),
        RuntimeEvent::Active(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
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
        RuntimeEvent::DisableUntilRestart,
        RuntimeEvent::Active(Duration::from_secs(100)),
        RuntimeEvent::WallClock(Duration::from_secs(60 * 60)),
        RuntimeEvent::Active(Duration::from_secs(100)),
        RuntimeEvent::Enable,
        RuntimeEvent::Active(Duration::from_secs(10)),
        RuntimeEvent::Shutdown,
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
    let mut backend = ScriptedBackend::new([RuntimeEvent::Shutdown]);

    assert_eq!(
        run_with_backend(config, &mut backend),
        Err(ConfigError::EmptyBreakTypes)
    );
}

struct ScriptedBackend {
    events: VecDeque<RuntimeEvent>,
    started_breaks: Vec<ScheduledBreak>,
}

impl ScriptedBackend {
    fn new(events: impl IntoIterator<Item = RuntimeEvent>) -> Self {
        Self {
            events: events.into_iter().collect(),
            started_breaks: Vec::new(),
        }
    }
}

impl DaemonBackend for ScriptedBackend {
    fn next_event(&mut self) -> RuntimeEvent {
        match self.events.pop_front() {
            Some(event) => event,
            None => RuntimeEvent::Shutdown,
        }
    }

    fn start_break(&mut self, scheduled_break: ScheduledBreak) {
        self.started_breaks.push(scheduled_break);
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
