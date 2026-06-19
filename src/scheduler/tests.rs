use super::{BreakSchedule, BreakScheduler, ScheduledBreak};
use crate::config::{BreakTypeConfig, Breaks, Config, ConfigError, DEFAULT_BREAK_AFTER_ACTIVE};
use std::collections::BTreeMap;
use std::time::Duration;

#[test]
fn default_config_schedules_short_and_long_break_slots() {
    let mut scheduler = scheduler(Config::default().breaks);

    let first = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(first.name, "short");
    assert_eq!(first.slot, 1);
    assert_eq!(first.duration, Duration::from_secs(20));
    assert_eq!(first.messages, vec![String::from("Rest your eyes")]);
    assert!(!first.autolock);

    assert_eq!(scheduler.finish_break(), Some(first));

    let second = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(second.name, "long");
    assert_eq!(second.slot, 2);
    assert_eq!(second.duration, Duration::from_secs(5 * 60));
    assert_eq!(second.messages, vec![String::from("Take a longer break")]);
    assert!(second.autolock);

    assert_eq!(scheduler.finish_break(), Some(second));

    let third = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(third.name, "short");
    assert_eq!(third.slot, 3);
}

#[test]
fn largest_due_interval_wins_for_the_slot() {
    let mut scheduler = scheduler(custom_breaks(
        10,
        &[("blink", 1, 1), ("short", 2, 20), ("long", 4, 300)],
    ));

    assert_eq!(
        started_break(scheduler.advance_active(Duration::from_secs(10))).name,
        "blink"
    );
    assert_eq!(
        scheduler.finish_break().map(|break_| break_.name),
        Some(String::from("blink"))
    );

    assert_eq!(
        started_break(scheduler.advance_active(Duration::from_secs(10))).name,
        "short"
    );
    assert_eq!(
        scheduler.finish_break().map(|break_| break_.name),
        Some(String::from("short"))
    );

    assert_eq!(
        started_break(scheduler.advance_active(Duration::from_secs(10))).name,
        "blink"
    );
    assert_eq!(
        scheduler.finish_break().map(|break_| break_.name),
        Some(String::from("blink"))
    );

    let fourth = started_break(scheduler.advance_active(Duration::from_secs(10)));
    assert_eq!(fourth.name, "long");
    assert_eq!(fourth.slot, 4);
    assert_eq!(fourth.duration, Duration::from_secs(300));
}

#[test]
fn empty_slots_are_skipped_until_a_break_type_is_due() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 3, 20), ("long", 5, 300)]));

    assert_eq!(scheduler.advance_active(Duration::from_secs(20)), None);
    assert_eq!(scheduler.pending_break(), None);

    let third = started_break(scheduler.advance_active(Duration::from_secs(10)));
    assert_eq!(third.name, "short");
    assert_eq!(third.slot, 3);
}

#[test]
fn large_active_delta_starts_only_the_first_due_break() {
    let mut scheduler = scheduler(Config::default().breaks);

    let first = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE * 3));
    assert_eq!(first.name, "short");
    assert_eq!(first.slot, 1);

    assert_eq!(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE), None);

    assert_eq!(scheduler.finish_break(), Some(first));

    let second = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(second.name, "long");
    assert_eq!(second.slot, 2);
}

#[test]
fn pending_break_blocks_active_time_accumulation() {
    let mut scheduler = scheduler(Config::default().breaks);

    let first = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(first.name, "short");
    assert_eq!(scheduler.pending_break(), Some(&first));

    assert_eq!(
        scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE * 10),
        None
    );
    assert_eq!(scheduler.pending_break(), Some(&first));

    assert_eq!(scheduler.finish_break(), Some(first));

    let second = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(second.name, "long");
    assert_eq!(second.slot, 2);
}

#[test]
fn finish_break_restarts_active_accumulation_from_zero() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    let first = started_break(scheduler.advance_active(Duration::from_secs(10)));
    assert_eq!(first.slot, 1);

    assert_eq!(scheduler.finish_break(), Some(first));

    assert_eq!(scheduler.advance_active(Duration::from_secs(9)), None);

    let second = started_break(scheduler.advance_active(Duration::from_secs(1)));
    assert_eq!(second.slot, 2);
}

#[test]
fn disabled_scheduler_ignores_active_time() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    assert_eq!(scheduler.disable(), None);

    assert!(scheduler.is_disabled());
    assert_eq!(scheduler.advance_active(Duration::from_secs(100)), None);

    scheduler.enable();

    assert!(!scheduler.is_disabled());
    assert_eq!(scheduler.advance_active(Duration::from_secs(9)), None);

    let first = started_break(scheduler.advance_active(Duration::from_secs(1)));
    assert_eq!(first.slot, 1);
}

#[test]
fn disable_resets_partial_active_time() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    assert_eq!(scheduler.advance_active(Duration::from_secs(9)), None);

    assert_eq!(scheduler.disable(), None);
    scheduler.enable();

    assert_eq!(scheduler.advance_active(Duration::from_secs(1)), None);

    let first = started_break(scheduler.advance_active(Duration::from_secs(9)));
    assert_eq!(first.slot, 1);
}

#[test]
fn enable_requires_fresh_active_interval_before_break() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    assert_eq!(scheduler.disable(), None);
    scheduler.enable();

    assert_eq!(scheduler.advance_active(Duration::from_secs(9)), None);

    let first = started_break(scheduler.advance_active(Duration::from_secs(1)));
    assert_eq!(first.slot, 1);
}

#[test]
fn disable_clears_pending_break_without_rewinding_slots() {
    let mut scheduler = scheduler(Config::default().breaks);

    let first = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(first.name, "short");
    assert_eq!(first.slot, 1);
    assert_eq!(scheduler.pending_break(), Some(&first));

    assert_eq!(scheduler.disable(), Some(first));

    assert!(scheduler.is_disabled());
    assert_eq!(scheduler.pending_break(), None);
    assert_eq!(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE), None);

    scheduler.enable();

    let second = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(second.name, "long");
    assert_eq!(second.slot, 2);
}

#[test]
fn disable_and_enable_are_idempotent() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    assert_eq!(scheduler.disable(), None);
    assert_eq!(scheduler.disable(), None);

    assert!(scheduler.is_disabled());
    assert_eq!(scheduler.advance_active(Duration::from_secs(10)), None);

    scheduler.enable();
    scheduler.enable();

    assert!(!scheduler.is_disabled());
    let first = started_break(scheduler.advance_active(Duration::from_secs(10)));
    assert_eq!(first.slot, 1);
}

#[test]
fn schedule_rejects_empty_break_types() {
    let mut breaks = custom_breaks(10, &[("short", 1, 20)]);
    breaks.types.clear();

    assert_eq!(
        BreakSchedule::try_from(breaks),
        Err(ConfigError::EmptyBreakTypes)
    );
}

#[test]
fn schedule_rejects_zero_break_interval() {
    let breaks = custom_breaks(10, &[("short", 0, 20)]);

    assert_eq!(
        BreakSchedule::try_from(breaks),
        Err(ConfigError::ZeroBreakInterval {
            name: String::from("short")
        })
    );
}

#[test]
fn schedule_rejects_duplicate_break_intervals() {
    let breaks = custom_breaks(10, &[("short", 1, 20), ("long", 1, 300)]);

    assert_eq!(
        BreakSchedule::try_from(breaks),
        Err(ConfigError::DuplicateBreakInterval {
            interval: 1,
            first_name: String::from("long"),
            duplicate_name: String::from("short")
        })
    );
}

fn scheduler(breaks: Breaks) -> BreakScheduler {
    match BreakSchedule::try_from(breaks) {
        Ok(schedule) => BreakScheduler::new(schedule),
        Err(error) => panic!("test breaks should be valid: {error}"),
    }
}

fn started_break(action: Option<ScheduledBreak>) -> ScheduledBreak {
    match action {
        Some(scheduled_break) => scheduled_break,
        None => panic!("expected break to start"),
    }
}

fn custom_breaks(after_active_secs: u64, types: &[(&str, usize, u64)]) -> Breaks {
    Breaks {
        after_active: Duration::from_secs(after_active_secs),
        types: types
            .iter()
            .map(|(name, interval, duration_secs)| {
                (
                    (*name).to_owned(),
                    break_type(name, *interval, *duration_secs),
                )
            })
            .collect::<BTreeMap<_, _>>(),
    }
}

fn break_type(name: &str, interval: usize, duration_secs: u64) -> BreakTypeConfig {
    BreakTypeConfig {
        interval,
        duration: Duration::from_secs(duration_secs),
        messages: vec![format!("Take a {name} break")],
        autolock: false,
    }
}
