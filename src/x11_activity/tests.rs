use super::{ActivityPoller, ActivitySample, ActivityState, BreakTimer};
use crate::backend::RuntimeEvent;
use std::time::Duration;

#[test]
fn zero_idle_time_is_active() {
    let sample = ActivitySample::new(Duration::ZERO);

    assert_eq!(
        sample.state_for(Duration::from_secs(1)),
        ActivityState::Active
    );
}

#[test]
fn idle_time_equal_to_poll_interval_is_active() {
    let poll_interval = Duration::from_secs(1);
    let sample = ActivitySample::new(poll_interval);

    assert_eq!(sample.state_for(poll_interval), ActivityState::Active);
}

#[test]
fn idle_time_below_poll_interval_is_active() {
    let poll_interval = Duration::from_secs(1);
    let sample = ActivitySample::new(Duration::from_millis(500));

    assert_eq!(sample.state_for(poll_interval), ActivityState::Active);
}

#[test]
fn idle_time_above_poll_interval_is_idle() {
    let poll_interval = Duration::from_secs(1);
    let sample = ActivitySample::new(Duration::from_millis(1_001));

    assert_eq!(sample.state_for(poll_interval), ActivityState::Idle);
}

#[test]
fn active_sample_queues_wall_clock_before_active_time() {
    let poll_interval = Duration::from_secs(1);
    let mut poller = ActivityPoller::new(poll_interval);

    assert_eq!(
        poller.queue_sample(ActivitySample::new(Duration::from_millis(500))),
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
fn idle_sample_queues_only_wall_clock_time() {
    let poll_interval = Duration::from_secs(1);
    let mut poller = ActivityPoller::new(poll_interval);

    assert_eq!(
        poller.queue_sample(ActivitySample::new(Duration::from_secs(2))),
        ActivityState::Idle
    );
    assert_eq!(
        poller.next_event(),
        Some(RuntimeEvent::WallClockElapsed(poll_interval))
    );
    assert_eq!(poller.next_event(), None);
}

#[test]
fn break_timer_finishes_once_duration_elapses() {
    let mut timer = BreakTimer::new(Duration::from_secs(2));

    assert!(!timer.advance(Duration::from_secs(1)));
    assert!(timer.advance(Duration::from_secs(1)));
    assert!(!timer.advance(Duration::from_secs(1)));
}

#[test]
fn break_timer_finishes_when_elapsed_time_overshoots_duration() {
    let mut timer = BreakTimer::new(Duration::from_secs(2));

    assert!(timer.advance(Duration::from_secs(3)));
    assert_eq!(timer.remaining, Duration::ZERO);
}
