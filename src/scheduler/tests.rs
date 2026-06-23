use super::{
    BreakOrigin, BreakSchedule, BreakScheduler, DEFAULT_BREAK_MESSAGE, ScheduledBreak,
    UpcomingScheduledBreak,
};
use crate::config::{BreakTypeConfig, Breaks, Config, ConfigError, DEFAULT_BREAK_AFTER_ACTIVE};
use std::collections::BTreeMap;
use std::time::Duration;

#[test]
fn default_config_schedules_short_and_long_break_slots() {
    let mut scheduler = scheduler(Config::default().breaks);

    let first = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(first.name, "short");
    assert_eq!(first.origin, BreakOrigin::Scheduled { slot: 1 });
    assert_eq!(first.duration, Duration::from_secs(20));
    assert_eq!(first.messages, vec![String::from("Rest your eyes")]);
    assert!(!first.autolock);

    assert!(scheduler.finish_break());

    let second = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(second.name, "long");
    assert_eq!(second.origin, BreakOrigin::Scheduled { slot: 2 });
    assert_eq!(second.duration, Duration::from_mins(5));
    assert_eq!(second.messages, vec![String::from("Take a longer break")]);
    assert!(second.autolock);

    assert!(scheduler.finish_break());

    let third = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(third.name, "short");
    assert_eq!(third.origin, BreakOrigin::Scheduled { slot: 3 });
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
    assert!(scheduler.finish_break());

    assert_eq!(
        started_break(scheduler.advance_active(Duration::from_secs(10))).name,
        "short"
    );
    assert!(scheduler.finish_break());

    assert_eq!(
        started_break(scheduler.advance_active(Duration::from_secs(10))).name,
        "blink"
    );
    assert!(scheduler.finish_break());

    let fourth = started_break(scheduler.advance_active(Duration::from_secs(10)));
    assert_eq!(fourth.name, "long");
    assert_eq!(fourth.origin, BreakOrigin::Scheduled { slot: 4 });
    assert_eq!(fourth.duration, Duration::from_mins(5));
}

#[test]
fn empty_slots_are_skipped_until_a_break_type_is_due() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 3, 20), ("long", 5, 300)]));

    assert_eq!(scheduler.advance_active(Duration::from_secs(20)), None);

    let third = started_break(scheduler.advance_active(Duration::from_secs(10)));
    assert_eq!(third.name, "short");
    assert_eq!(third.origin, BreakOrigin::Scheduled { slot: 3 });
}

#[test]
fn large_active_delta_starts_only_the_first_due_break() {
    let mut scheduler = scheduler(Config::default().breaks);

    let first = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE * 3));
    assert_eq!(first.name, "short");
    assert_eq!(first.origin, BreakOrigin::Scheduled { slot: 1 });

    assert_eq!(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE), None);

    assert!(scheduler.finish_break());

    let second = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(second.name, "long");
    assert_eq!(second.origin, BreakOrigin::Scheduled { slot: 2 });
}

#[test]
fn pending_break_blocks_active_time_accumulation() {
    let mut scheduler = scheduler(Config::default().breaks);

    let first = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(first.name, "short");

    assert_eq!(
        scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE * 10),
        None
    );

    assert!(scheduler.finish_break());

    let second = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(second.name, "long");
    assert_eq!(second.origin, BreakOrigin::Scheduled { slot: 2 });
}

#[test]
fn finish_break_restarts_active_accumulation_from_zero() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    let first = started_break(scheduler.advance_active(Duration::from_secs(10)));
    assert_eq!(first.origin, BreakOrigin::Scheduled { slot: 1 });

    assert!(scheduler.finish_break());

    assert_eq!(scheduler.advance_active(Duration::from_secs(9)), None);

    let second = started_break(scheduler.advance_active(Duration::from_secs(1)));
    assert_eq!(second.origin, BreakOrigin::Scheduled { slot: 2 });
}

#[test]
fn disabled_scheduler_ignores_active_time() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    assert!(!scheduler.disable());

    assert!(scheduler.is_disabled());
    assert_eq!(scheduler.advance_active(Duration::from_secs(100)), None);

    scheduler.enable();

    assert!(!scheduler.is_disabled());
    assert_eq!(scheduler.advance_active(Duration::from_secs(9)), None);

    let first = started_break(scheduler.advance_active(Duration::from_secs(1)));
    assert_eq!(first.origin, BreakOrigin::Scheduled { slot: 1 });
}

#[test]
fn disable_resets_partial_active_time() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    assert_eq!(scheduler.advance_active(Duration::from_secs(9)), None);

    assert!(!scheduler.disable());
    scheduler.enable();

    assert_eq!(scheduler.advance_active(Duration::from_secs(1)), None);

    let first = started_break(scheduler.advance_active(Duration::from_secs(9)));
    assert_eq!(first.origin, BreakOrigin::Scheduled { slot: 1 });
}

#[test]
fn enable_requires_fresh_active_interval_before_break() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    assert!(!scheduler.disable());
    scheduler.enable();

    assert_eq!(scheduler.advance_active(Duration::from_secs(9)), None);

    let first = started_break(scheduler.advance_active(Duration::from_secs(1)));
    assert_eq!(first.origin, BreakOrigin::Scheduled { slot: 1 });
}

#[test]
fn disable_clears_pending_break_without_rewinding_slots() {
    let mut scheduler = scheduler(Config::default().breaks);

    let first = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(first.name, "short");
    assert_eq!(first.origin, BreakOrigin::Scheduled { slot: 1 });

    assert!(scheduler.disable());

    assert!(scheduler.is_disabled());
    assert_eq!(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE), None);

    scheduler.enable();

    let second = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
    assert_eq!(second.name, "long");
    assert_eq!(second.origin, BreakOrigin::Scheduled { slot: 2 });
}

#[test]
fn disable_and_enable_are_idempotent() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    assert!(!scheduler.disable());
    assert!(!scheduler.disable());

    assert!(scheduler.is_disabled());
    assert_eq!(scheduler.advance_active(Duration::from_secs(10)), None);

    scheduler.enable();
    scheduler.enable();

    assert!(!scheduler.is_disabled());
    let first = started_break(scheduler.advance_active(Duration::from_secs(10)));
    assert_eq!(first.origin, BreakOrigin::Scheduled { slot: 1 });
}

#[test]
fn manual_break_can_start_while_enabled() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20), ("long", 2, 300)]));

    let manual = started_break(scheduler.start_manual_break("long"));
    assert_eq!(manual.name, "long");
    assert_eq!(manual.origin, BreakOrigin::Manual);
    assert_eq!(manual.duration, Duration::from_mins(5));

    assert!(scheduler.finish_break());

    let next_break = started_break(scheduler.advance_active(Duration::from_secs(10)));
    assert_eq!(next_break.name, "short");
    assert_eq!(next_break.origin, BreakOrigin::Scheduled { slot: 1 });
}

#[test]
fn manual_break_can_start_while_disabled_and_resumes_disabled() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    assert!(!scheduler.disable());

    let manual = started_break(scheduler.start_manual_break("short"));
    assert_eq!(manual.origin, BreakOrigin::Manual);
    assert!(scheduler.is_disabled());

    assert!(scheduler.finish_break());
    assert!(scheduler.is_disabled());
    assert_eq!(scheduler.advance_active(Duration::from_secs(100)), None);

    scheduler.enable();

    let next_break = started_break(scheduler.advance_active(Duration::from_secs(10)));
    assert_eq!(next_break.origin, BreakOrigin::Scheduled { slot: 1 });
}

#[test]
fn enable_during_disabled_manual_break_resumes_active_after_finish() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    assert!(!scheduler.disable());
    let manual = started_break(scheduler.start_manual_break("short"));
    assert_eq!(manual.origin, BreakOrigin::Manual);

    scheduler.enable();

    assert!(!scheduler.is_disabled());
    assert!(scheduler.finish_break());

    let next_break = started_break(scheduler.advance_active(Duration::from_secs(10)));
    assert_eq!(next_break.origin, BreakOrigin::Scheduled { slot: 1 });
}

#[test]
fn unknown_manual_break_name_is_ignored() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    assert_eq!(scheduler.advance_active(Duration::from_secs(9)), None);
    assert_eq!(scheduler.start_manual_break("missing"), None);

    let next_break = started_break(scheduler.advance_active(Duration::from_secs(1)));
    assert_eq!(next_break.origin, BreakOrigin::Scheduled { slot: 1 });
}

#[test]
fn manual_break_resets_active_accumulation_without_advancing_slots() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20), ("long", 2, 300)]));

    assert_eq!(scheduler.advance_active(Duration::from_secs(9)), None);
    let manual = started_break(scheduler.start_manual_break("long"));

    assert_eq!(manual.name, "long");
    assert_eq!(manual.origin, BreakOrigin::Manual);
    assert!(scheduler.finish_break());
    assert_eq!(scheduler.advance_active(Duration::from_secs(1)), None);

    let next_break = started_break(scheduler.advance_active(Duration::from_secs(9)));
    assert_eq!(next_break.name, "short");
    assert_eq!(next_break.origin, BreakOrigin::Scheduled { slot: 1 });
}

#[test]
fn upcoming_scheduled_break_reports_remaining_active_time() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20), ("long", 2, 300)]));

    let upcoming = upcoming_break(scheduler.upcoming_scheduled_break());
    assert_eq!(upcoming.scheduled_break.name, "short");
    assert_eq!(
        upcoming.scheduled_break.origin,
        BreakOrigin::Scheduled { slot: 1 }
    );
    assert_eq!(upcoming.starts_after, Duration::from_secs(10));

    assert_eq!(scheduler.advance_active(Duration::from_secs(4)), None);

    let upcoming = upcoming_break(scheduler.upcoming_scheduled_break());
    assert_eq!(upcoming.scheduled_break.name, "short");
    assert_eq!(upcoming.starts_after, Duration::from_secs(6));
}

#[test]
fn active_elapsed_reports_current_slot_accumulation() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    assert_eq!(scheduler.active_elapsed(), Duration::ZERO);
    assert_eq!(scheduler.advance_active(Duration::from_secs(4)), None);
    assert_eq!(scheduler.active_elapsed(), Duration::from_secs(4));

    let scheduled_break = started_break(scheduler.advance_active(Duration::from_secs(6)));
    assert_eq!(scheduled_break.origin, BreakOrigin::Scheduled { slot: 1 });
    assert_eq!(scheduler.active_elapsed(), Duration::ZERO);
}

#[test]
fn upcoming_scheduled_break_skips_empty_slots() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 3, 20), ("long", 5, 300)]));

    let upcoming = upcoming_break(scheduler.upcoming_scheduled_break());
    assert_eq!(upcoming.scheduled_break.name, "short");
    assert_eq!(
        upcoming.scheduled_break.origin,
        BreakOrigin::Scheduled { slot: 3 }
    );
    assert_eq!(upcoming.starts_after, Duration::from_secs(30));

    assert_eq!(scheduler.advance_active(Duration::from_secs(25)), None);

    let upcoming = upcoming_break(scheduler.upcoming_scheduled_break());
    assert_eq!(upcoming.scheduled_break.name, "short");
    assert_eq!(upcoming.starts_after, Duration::from_secs(5));
}

#[test]
fn upcoming_scheduled_break_is_absent_while_disabled_or_pending() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    assert!(!scheduler.disable());
    assert_eq!(scheduler.upcoming_scheduled_break(), None);

    scheduler.enable();
    let scheduled_break = started_break(scheduler.advance_active(Duration::from_secs(10)));
    assert_eq!(scheduled_break.origin, BreakOrigin::Scheduled { slot: 1 });
    assert_eq!(scheduler.upcoming_scheduled_break(), None);
}

#[test]
fn reset_active_time_discards_partial_active_time() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    assert_eq!(scheduler.advance_active(Duration::from_secs(9)), None);
    scheduler.reset_active_time();

    assert_eq!(scheduler.advance_active(Duration::from_secs(1)), None);
    let first = started_break(scheduler.advance_active(Duration::from_secs(9)));
    assert_eq!(first.origin, BreakOrigin::Scheduled { slot: 1 });
}

#[test]
fn reset_active_time_preserves_completed_slots() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20), ("long", 2, 300)]));

    let first = started_break(scheduler.advance_active(Duration::from_secs(10)));
    assert_eq!(first.origin, BreakOrigin::Scheduled { slot: 1 });
    assert!(scheduler.finish_break());

    assert_eq!(scheduler.advance_active(Duration::from_secs(9)), None);
    scheduler.reset_active_time();

    let second = started_break(scheduler.advance_active(Duration::from_secs(10)));
    assert_eq!(second.name, "long");
    assert_eq!(second.origin, BreakOrigin::Scheduled { slot: 2 });
}

#[test]
fn reset_active_time_preserves_pending_and_disabled_state() {
    let mut scheduler = scheduler(custom_breaks(10, &[("short", 1, 20)]));

    let first = started_break(scheduler.advance_active(Duration::from_secs(10)));
    assert_eq!(first.origin, BreakOrigin::Scheduled { slot: 1 });
    scheduler.reset_active_time();
    assert_eq!(scheduler.advance_active(Duration::from_secs(10)), None);
    assert!(scheduler.finish_break());

    assert!(!scheduler.disable());
    scheduler.reset_active_time();
    assert!(scheduler.is_disabled());
    assert_eq!(scheduler.advance_active(Duration::from_secs(10)), None);
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

#[test]
fn message_at_returns_the_message_for_the_given_index() {
    let scheduled_break = scheduled_break(&["Rest your eyes", "Look away", "Blink"]);

    assert_eq!(scheduled_break.message_at(0), "Rest your eyes");
    assert_eq!(scheduled_break.message_at(1), "Look away");
    assert_eq!(scheduled_break.message_at(2), "Blink");
}

#[test]
fn message_at_wraps_indices_past_the_end() {
    let scheduled_break = scheduled_break(&["Rest your eyes", "Look away"]);

    assert_eq!(scheduled_break.message_at(2), "Rest your eyes");
    assert_eq!(scheduled_break.message_at(3), "Look away");
}

#[test]
fn message_at_falls_back_to_the_default_when_no_messages_are_configured() {
    let scheduled_break = scheduled_break(&[]);

    assert_eq!(scheduled_break.message_at(0), DEFAULT_BREAK_MESSAGE);
}

#[test]
fn random_message_returns_a_configured_message() {
    let messages = ["Rest your eyes", "Look away", "Blink"];
    let scheduled_break = scheduled_break(&messages);

    for _ in 0..32 {
        assert!(messages.contains(&scheduled_break.random_message()));
    }
}

fn scheduled_break(messages: &[&str]) -> ScheduledBreak {
    ScheduledBreak {
        name: String::from("short"),
        origin: BreakOrigin::Manual,
        duration: Duration::from_secs(20),
        messages: messages
            .iter()
            .map(|message| (*message).to_owned())
            .collect(),
        autolock: false,
    }
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

fn upcoming_break(action: Option<UpcomingScheduledBreak>) -> UpcomingScheduledBreak {
    match action {
        Some(upcoming_break) => upcoming_break,
        None => panic!("expected upcoming break"),
    }
}

fn custom_breaks(after_active_secs: u64, types: &[(&str, usize, u64)]) -> Breaks {
    Breaks {
        after_active: Duration::from_secs(after_active_secs),
        reset_after_idle: Some(Duration::from_mins(5)),
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
