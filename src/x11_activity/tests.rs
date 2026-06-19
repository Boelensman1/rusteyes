use super::DEFAULT_LOCK_COMMAND;
use crate::config::LockConfig;
use crate::lock_command::{LockCommand, lock_process};

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
