use super::{
    ActivityPoller, ActivitySample, ActivityState, BreakTimer, LockCommand,
    break_elapsed_for_sample, lock_process, spawn_lock_command,
};
use crate::backend::RuntimeEvent;
use crate::config::LockConfig;
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

#[test]
fn active_overlay_sample_does_not_count_down_break_time() {
    let poll_interval = Duration::from_millis(500);
    let elapsed = break_elapsed_for_sample(ActivitySample::new(Duration::ZERO), poll_interval);

    assert_eq!(elapsed, Duration::ZERO);
}

#[test]
fn idle_overlay_sample_counts_down_break_time() {
    let poll_interval = Duration::from_millis(500);
    let elapsed = break_elapsed_for_sample(
        ActivitySample::new(Duration::from_millis(501)),
        poll_interval,
    );

    assert_eq!(elapsed, poll_interval);
}

#[test]
fn lock_command_splits_program_and_args() {
    let lock_command = LockCommand::from(LockConfig {
        command: vec![
            String::from("locker"),
            String::from("--now"),
            String::from("--quiet"),
        ],
    });

    assert_eq!(
        lock_command,
        LockCommand {
            argv: vec![
                String::from("locker"),
                String::from("--now"),
                String::from("--quiet")
            ]
        }
    );
}

#[test]
fn lock_process_uses_configured_argv_without_shell() {
    let lock_command = LockCommand {
        argv: vec![
            String::from("locker"),
            String::from("--message"),
            String::from("lock now"),
        ],
    };
    let command = match lock_process(&lock_command) {
        Ok(command) => command,
        Err(error) => panic!("expected lock command process: {error}"),
    };

    assert_eq!(command.get_program().to_str(), Some("locker"));
    assert_eq!(
        command
            .get_args()
            .map(std::ffi::OsStr::to_str)
            .collect::<Vec<_>>(),
        vec![Some("--message"), Some("lock now")]
    );
}

#[test]
fn empty_lock_command_returns_context() {
    let lock_command = LockCommand { argv: Vec::new() };
    let Err(error) = lock_process(&lock_command) else {
        panic!("expected empty lock command error");
    };

    assert_eq!(
        error.to_string(),
        "failed to request local lock: lock command must not be empty"
    );
}

#[test]
fn missing_lock_command_binary_returns_start_error() {
    let lock_command = LockCommand {
        argv: vec![String::from(
            "/definitely/missing/resteyes-lock-command-test-binary",
        )],
    };
    let Err(error) = spawn_lock_command(&lock_command) else {
        panic!("expected missing binary error");
    };

    let message = error.to_string();
    assert!(message.contains("failed to request local lock: failed to start"));
    assert!(message.contains("/definitely/missing/resteyes-lock-command-test-binary"));
}

#[test]
fn lock_command_spawn_returns_before_child_exits() {
    let lock_command = LockCommand {
        argv: vec![
            String::from("sh"),
            String::from("-c"),
            String::from("sleep 5"),
        ],
    };
    let mut spawned = match spawn_lock_command(&lock_command) {
        Ok(spawned) => spawned,
        Err(error) => panic!("expected sleeping lock command to start: {error}"),
    };

    match spawned.child.try_wait() {
        Ok(None) => {}
        Ok(Some(status)) => panic!("expected child to still run, exited with {status}"),
        Err(error) => panic!("failed to poll child: {error}"),
    }

    if let Err(error) = spawned.child.kill() {
        panic!("failed to kill test child: {error}");
    }
    if let Err(error) = spawned.child.wait() {
        panic!("failed to wait for test child: {error}");
    }
}
