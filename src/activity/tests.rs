use super::{
    ActivityPoller, ActivitySample, ActivityState, BreakTimer, NORMAL_ACTIVITY_IDLE_THRESHOLD,
};
use crate::backend::RuntimeEvent;
use std::time::Duration;

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
fn break_timer_advances_without_finishing() {
    let mut timer = BreakTimer::new(Duration::from_secs(1));

    assert!(!timer.advance(Duration::from_millis(500)));
    assert_eq!(timer.remaining(), Duration::from_millis(500));
}

#[test]
fn break_timer_finishes_on_exact_remaining_elapsed() {
    let mut timer = BreakTimer::new(Duration::from_secs(1));

    assert!(timer.advance(Duration::from_secs(1)));
    assert_eq!(timer.remaining(), Duration::ZERO);
}

#[test]
fn break_timer_finishes_when_elapsed_overshoots_remaining() {
    let mut timer = BreakTimer::new(Duration::from_secs(1));

    assert!(timer.advance(Duration::from_secs(2)));
    assert_eq!(timer.remaining(), Duration::ZERO);
}

#[test]
fn break_timer_does_not_finish_again_after_reaching_zero() {
    let mut timer = BreakTimer::new(Duration::from_secs(1));

    assert!(timer.advance(Duration::from_secs(1)));
    assert!(!timer.advance(Duration::from_secs(1)));
    assert_eq!(timer.remaining(), Duration::ZERO);
}
