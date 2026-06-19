use super::{LockCommand, lock_process, spawn_lock_command};
use crate::config::LockConfig;
use std::time::Duration;

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
