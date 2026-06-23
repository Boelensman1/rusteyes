use super::{BREAK_START_RETRY_TIMEOUT, DEFAULT_LOCK_COMMAND, PendingBreakStart};
use crate::config::LockConfig;
use crate::lock_command::{LockCommand, lock_process};
use crate::scheduler::{BreakOrigin, ScheduledBreak};
use std::time::Duration;

#[test]
fn default_lock_command_uses_loginctl() {
    let lock_command = LockCommand::from(LockConfig { command: None });
    let command = match lock_process(&lock_command) {
        Ok(command) => command,
        Err(error) => panic!("expected default lock command process: {error}"),
    };

    assert_eq!(
        command.get_program().to_str(),
        Some(DEFAULT_LOCK_COMMAND[0])
    );
    assert_eq!(
        command
            .get_args()
            .map(std::ffi::OsStr::to_str)
            .collect::<Vec<_>>(),
        vec![Some(DEFAULT_LOCK_COMMAND[1])]
    );
}

#[test]
fn explicit_lock_command_overrides_default() {
    let lock_command = LockCommand::from(LockConfig {
        command: Some(vec![
            String::from("locker"),
            String::from("--now"),
            String::from("--quiet"),
        ]),
    });
    let command = match lock_process(&lock_command) {
        Ok(command) => command,
        Err(error) => panic!("expected explicit lock command process: {error}"),
    };

    assert_eq!(command.get_program().to_str(), Some("locker"));
    assert_eq!(
        command
            .get_args()
            .map(std::ffi::OsStr::to_str)
            .collect::<Vec<_>>(),
        vec![Some("--now"), Some("--quiet")]
    );
}

#[test]
fn pending_break_start_times_out_after_retry_timeout() {
    let mut pending_break = PendingBreakStart::new(test_break(false));

    pending_break.advance_wait(
        BREAK_START_RETRY_TIMEOUT
            .checked_sub(Duration::from_millis(1))
            .unwrap_or_default(),
    );
    assert!(!pending_break.retry_timed_out());

    pending_break.advance_wait(Duration::from_millis(1));
    assert!(pending_break.retry_timed_out());
}

#[test]
fn pending_break_start_remembers_lock_after_request() {
    let mut pending_break = PendingBreakStart::new(test_break(false));

    pending_break.request_lock_after_current_break();

    assert!(pending_break.scheduled_break.autolock);
}

fn test_break(autolock: bool) -> ScheduledBreak {
    ScheduledBreak {
        name: String::from("short"),
        origin: BreakOrigin::Scheduled { slot: 1 },
        duration: Duration::from_secs(20),
        message: String::from("Rest your eyes"),
        autolock,
    }
}
