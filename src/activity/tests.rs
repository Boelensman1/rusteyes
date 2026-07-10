use super::{
    ActivityPoller, ActivitySample, ActivityState, BreakDeadline, NORMAL_ACTIVITY_IDLE_THRESHOLD,
};
use crate::backend::RuntimeEvent;
use std::time::{Duration, SystemTime};

fn wall_start() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
}

#[test]
fn zero_idle_time_is_active() {
    let sample = ActivitySample::new(Duration::ZERO);

    assert_eq!(sample.state_for(Duration::ZERO), ActivityState::Active);
}

#[test]
fn idle_time_equal_to_threshold_is_active() {
    let sample = ActivitySample::new(NORMAL_ACTIVITY_IDLE_THRESHOLD);

    assert_eq!(
        sample.state_for(NORMAL_ACTIVITY_IDLE_THRESHOLD),
        ActivityState::Active
    );
}

#[test]
fn idle_time_below_threshold_is_active() {
    let sample = ActivitySample::new(Duration::from_secs(2));

    assert_eq!(
        sample.state_for(NORMAL_ACTIVITY_IDLE_THRESHOLD),
        ActivityState::Active
    );
}

#[test]
fn idle_time_above_threshold_is_idle() {
    let sample = ActivitySample::new(NORMAL_ACTIVITY_IDLE_THRESHOLD + Duration::from_millis(1));

    assert_eq!(
        sample.state_for(NORMAL_ACTIVITY_IDLE_THRESHOLD),
        ActivityState::Idle
    );
}

#[test]
fn active_sample_queues_wall_clock_before_active_time() {
    let poll_interval = Duration::from_secs(1);
    let mut poller = ActivityPoller::new(poll_interval);

    assert_eq!(
        poller.queue_sample(ActivitySample::new(Duration::from_secs(2))),
        ActivityState::Active
    );
    assert_eq!(
        poller.next_event(),
        Some(RuntimeEvent::WallClockElapsed(poll_interval))
    );
    assert_eq!(
        poller.next_event(),
        Some(RuntimeEvent::ActiveTimeElapsed(poll_interval))
    );
    assert_eq!(poller.next_event(), None);
}

#[test]
fn idle_sample_queues_wall_clock_before_idle_time() {
    let poll_interval = Duration::from_secs(1);
    let mut poller = ActivityPoller::new(poll_interval);

    assert_eq!(
        poller.queue_sample(ActivitySample::new(
            NORMAL_ACTIVITY_IDLE_THRESHOLD + Duration::from_secs(1),
        )),
        ActivityState::Idle
    );
    assert_eq!(
        poller.next_event(),
        Some(RuntimeEvent::WallClockElapsed(poll_interval))
    );
    assert_eq!(
        poller.next_event(),
        Some(RuntimeEvent::IdleTimeElapsed(poll_interval))
    );
    assert_eq!(poller.next_event(), None);
}

#[test]
fn break_deadline_reports_remaining_time_before_deadline() {
    let started_at = wall_start();
    let deadline = BreakDeadline::starting_at(started_at, Duration::from_secs(1));

    assert_eq!(
        deadline.remaining_at(started_at + Duration::from_millis(400)),
        Duration::from_millis(600)
    );
    assert!(!deadline.is_finished_at(started_at + Duration::from_millis(400)));
}

#[test]
fn break_deadline_finishes_at_exact_deadline() {
    let started_at = wall_start();
    let deadline = BreakDeadline::starting_at(started_at, Duration::from_secs(1));

    assert_eq!(
        deadline.remaining_at(started_at + Duration::from_secs(1)),
        Duration::ZERO
    );
    assert!(deadline.is_finished_at(started_at + Duration::from_secs(1)));
}

#[test]
fn break_deadline_finishes_after_overshooting_deadline() {
    let started_at = wall_start();
    let deadline = BreakDeadline::starting_at(started_at, Duration::from_secs(1));

    assert_eq!(
        deadline.remaining_at(started_at + Duration::from_secs(2)),
        Duration::ZERO
    );
    assert!(deadline.is_finished_at(started_at + Duration::from_secs(2)));
}

#[test]
fn break_deadline_finishes_immediately_when_duration_overflows_clock() {
    let started_at = wall_start();
    let deadline = BreakDeadline::starting_at(started_at, Duration::MAX);

    assert_eq!(deadline.remaining_at(started_at), Duration::ZERO);
    assert!(deadline.is_finished_at(started_at));
}

#[test]
fn break_deadline_finishes_after_wall_clock_sleep_jump() {
    let started_at = wall_start();
    let deadline = BreakDeadline::starting_at(started_at, Duration::from_mins(5));

    let after_wake = started_at + Duration::from_mins(10);
    assert_eq!(deadline.remaining_at(after_wake), Duration::ZERO);
    assert!(deadline.is_finished_at(after_wake));
}

#[test]
fn break_deadline_backwards_clock_jump_clamps_remaining() {
    let started_at = wall_start();
    let duration = Duration::from_mins(5);
    let deadline = BreakDeadline::starting_at(started_at, duration);

    let jumped_back = started_at - Duration::from_hours(1);
    assert_eq!(deadline.remaining_at(jumped_back), duration);
    assert!(!deadline.is_finished_at(jumped_back));
}
