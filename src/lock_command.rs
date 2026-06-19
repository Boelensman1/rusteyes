use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use tracing::{error, trace};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockCommand {
    argv: Vec<String>,
}

impl LockCommand {
    pub(crate) fn new(argv: Vec<String>) -> Self {
        Self { argv }
    }

    pub(crate) fn description(&self) -> String {
        if self.argv.is_empty() {
            String::from("<empty lock command>")
        } else {
            self.argv.join(" ")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockCommandError {
    message: String,
}

impl LockCommandError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for LockCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LockCommandError {}

pub(crate) fn start_lock_command(lock_command: &LockCommand) -> Result<(), LockCommandError> {
    let lock_command = lock_command.clone();
    let description = lock_command.description();
    let (startup_tx, startup_rx) = mpsc::channel();

    thread::Builder::new()
        .name(String::from("resteyes-lock-command"))
        .spawn(move || match spawn_lock_command(&lock_command) {
            Ok(spawned) => {
                let _ = startup_tx.send(Ok(()));
                supervise_lock_command(spawned);
            }
            Err(error) => {
                let _ = startup_tx.send(Err(error));
            }
        })
        .map_err(|error| {
            LockCommandError::new(format!(
                "failed to start supervisor for {description}: {error}"
            ))
        })?;

    startup_rx.recv().map_err(|error| {
        LockCommandError::new(format!(
            "failed to receive startup status for {description}: {error}"
        ))
    })?
}

pub(crate) fn spawn_lock_command(
    lock_command: &LockCommand,
) -> Result<SpawnedLockCommand, LockCommandError> {
    let description = lock_command.description();
    let mut command = lock_process(lock_command)?;
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        LockCommandError::new(format!("failed to start {description}: {error}"))
    })?;

    Ok(SpawnedLockCommand {
        description,
        stdout: child.stdout.take(),
        stderr: child.stderr.take(),
        child,
    })
}

pub(crate) fn lock_process(lock_command: &LockCommand) -> Result<Command, LockCommandError> {
    let Some((program, args)) = lock_command.argv.split_first() else {
        return Err(LockCommandError::new("lock command must not be empty"));
    };

    let mut command = Command::new(program);
    command.args(args);
    Ok(command)
}

pub(crate) struct SpawnedLockCommand {
    pub(crate) description: String,
    pub(crate) child: Child,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
}

fn supervise_lock_command(spawned: SpawnedLockCommand) {
    let SpawnedLockCommand {
        description,
        mut child,
        stdout,
        stderr,
    } = spawned;

    thread::scope(|scope| {
        if let Some(stdout) = stdout {
            let description = description.clone();
            scope.spawn(move || trace_lock_stdout(&description, stdout));
        }

        if let Some(stderr) = stderr {
            let description = description.clone();
            scope.spawn(move || mirror_lock_stderr(&description, stderr));
        }

        match child.wait() {
            Ok(status) => {
                trace!(
                    command = %description,
                    %status,
                    success = status.success(),
                    "lock command exited"
                );
            }
            Err(error) => {
                error!(command = %description, %error, "failed to wait for lock command");
            }
        }
    });
}

fn trace_lock_stdout<R>(description: &str, output: R)
where
    R: Read,
{
    for line in BufReader::new(output).lines() {
        match line {
            Ok(line) => trace!(command = %description, %line, "lock command stdout"),
            Err(error) => {
                trace!(command = %description, %error, "failed to read lock command stdout");
                break;
            }
        }
    }
}

fn mirror_lock_stderr<R>(description: &str, output: R)
where
    R: Read,
{
    let mut stderr = io::stderr().lock();

    for line in BufReader::new(output).lines() {
        match line {
            Ok(line) => {
                let _ = writeln!(
                    stderr,
                    "resteyes: lock command stderr ({description}): {line}"
                );
                trace!(command = %description, %line, "lock command stderr");
            }
            Err(error) => {
                let _ = writeln!(
                    stderr,
                    "resteyes: failed to read lock command stderr ({description}): {error}"
                );
                trace!(command = %description, %error, "failed to read lock command stderr");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests;
