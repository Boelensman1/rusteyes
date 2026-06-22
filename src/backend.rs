use crate::scheduler::ScheduledBreak;
use std::fmt;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tracing::warn;

pub(crate) type BackendCommandReceiver = flume::Receiver<BackendCommand>;
pub(crate) type BackendEventSender = flume::Sender<RuntimeEvent>;

#[derive(Debug)]
pub(crate) struct BackendActor {
    command_sender: Option<flume::Sender<BackendCommand>>,
    event_receiver: flume::Receiver<RuntimeEvent>,
    thread: Option<JoinHandle<()>>,
}

impl BackendActor {
    pub(crate) fn spawn<State, Error, Connect, Run>(
        thread_name: &'static str,
        connect: Connect,
        run: Run,
    ) -> Result<Self, Error>
    where
        State: Send + 'static,
        Error: From<BackendActorSpawnError> + Send + 'static,
        Connect: FnOnce() -> Result<State, Error> + Send + 'static,
        Run: FnOnce(State, BackendCommandReceiver, BackendEventSender) + Send + 'static,
    {
        let (command_sender, command_receiver) = flume::unbounded();
        let (event_sender, event_receiver) = flume::unbounded();
        let (startup_sender, startup_receiver) = flume::bounded(1);

        let thread = thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || match connect() {
                Ok(state) => {
                    _ = startup_sender.send(Ok(()));
                    run(state, command_receiver, event_sender);
                }
                Err(error) => {
                    _ = startup_sender.send(Err(error));
                }
            })
            .map_err(|error| {
                BackendActorSpawnError::spawn_thread(thread_name, error.to_string())
            })?;

        match startup_receiver.recv() {
            Ok(Ok(())) => Ok(Self::new(command_sender, event_receiver, thread)),
            Ok(Err(error)) => {
                _ = thread.join();
                Err(error)
            }
            Err(_) => {
                _ = thread.join();
                Err(BackendActorSpawnError::startup_dropped(thread_name).into())
            }
        }
    }

    pub(crate) fn new(
        command_sender: flume::Sender<BackendCommand>,
        event_receiver: flume::Receiver<RuntimeEvent>,
        thread: JoinHandle<()>,
    ) -> Self {
        Self {
            command_sender: Some(command_sender),
            event_receiver,
            thread: Some(thread),
        }
    }

    pub(crate) fn clone_event_receiver(&self) -> flume::Receiver<RuntimeEvent> {
        self.event_receiver.clone()
    }

    pub(crate) fn send_command(&self, command: BackendCommand) -> Result<(), BackendActorError> {
        let Some(command_sender) = &self.command_sender else {
            return Err(BackendActorError::Stopped);
        };

        command_sender
            .send(command)
            .map_err(|_| BackendActorError::Stopped)
    }
}

impl Drop for BackendActor {
    fn drop(&mut self) {
        drop(self.command_sender.take());

        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            warn!("backend actor thread panicked");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendActorError {
    Stopped,
}

impl fmt::Display for BackendActorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped => formatter.write_str("backend actor stopped"),
        }
    }
}

impl std::error::Error for BackendActorError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackendActorSpawnError {
    thread_name: &'static str,
    kind: BackendActorSpawnErrorKind,
}

impl BackendActorSpawnError {
    fn spawn_thread(thread_name: &'static str, message: String) -> Self {
        Self {
            thread_name,
            kind: BackendActorSpawnErrorKind::SpawnThread { message },
        }
    }

    fn startup_dropped(thread_name: &'static str) -> Self {
        Self {
            thread_name,
            kind: BackendActorSpawnErrorKind::StartupDropped,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BackendActorSpawnErrorKind {
    SpawnThread { message: String },
    StartupDropped,
}

impl fmt::Display for BackendActorSpawnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            BackendActorSpawnErrorKind::SpawnThread { message } => {
                write!(
                    formatter,
                    "failed to spawn {} thread: {message}",
                    self.thread_name
                )
            }
            BackendActorSpawnErrorKind::StartupDropped => {
                write!(
                    formatter,
                    "{} thread exited before reporting startup",
                    self.thread_name
                )
            }
        }
    }
}

impl std::error::Error for BackendActorSpawnError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackendWait {
    Command(BackendCommand),
    Timeout,
    Disconnected,
}

pub(crate) fn wait_for_command_or_timeout(
    command_receiver: &BackendCommandReceiver,
    timeout: Duration,
) -> BackendWait {
    enum SelectedBackendWait {
        Command(Result<BackendCommand, flume::RecvError>),
    }

    match flume::Selector::new()
        .recv(command_receiver, SelectedBackendWait::Command)
        .wait_timeout(timeout)
    {
        Ok(SelectedBackendWait::Command(Ok(command))) => BackendWait::Command(command),
        Ok(SelectedBackendWait::Command(Err(_))) => BackendWait::Disconnected,
        Err(flume::select::SelectError::Timeout) => BackendWait::Timeout,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeEvent {
    ActiveTimeElapsed(Duration),
    IdleTimeElapsed(Duration),
    WallClockElapsed(Duration),
    BreakStartFailed,
    BreakFinished,
    LockAfterCurrentBreak,
    StartManualBreak(String),
    Disable(DisableRequest),
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisableRequest {
    For(Duration),
    UntilRestart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum BackendCommand {
    StartBreak(ScheduledBreak),
    FinishBreak { lock_after: bool },
    RequestLockAfterCurrentBreak,
    ClearBreak,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::{BreakOrigin, ScheduledBreak};

    #[test]
    fn wait_returns_command_before_timeout() -> Result<(), flume::SendError<BackendCommand>> {
        let (sender, receiver) = flume::unbounded();
        sender.send(BackendCommand::ClearBreak)?;

        assert_eq!(
            wait_for_command_or_timeout(&receiver, Duration::from_secs(1)),
            BackendWait::Command(BackendCommand::ClearBreak)
        );
        Ok(())
    }

    #[test]
    fn wait_returns_timeout_without_command() {
        let (_sender, receiver) = flume::unbounded();

        assert_eq!(
            wait_for_command_or_timeout(&receiver, Duration::ZERO),
            BackendWait::Timeout
        );
    }

    #[test]
    fn wait_returns_disconnected_when_sender_is_dropped() {
        let (sender, receiver) = flume::unbounded();
        drop(sender);

        assert_eq!(
            wait_for_command_or_timeout(&receiver, Duration::from_secs(1)),
            BackendWait::Disconnected
        );
    }

    #[test]
    fn backend_actor_drops_command_sender_before_joining() {
        let (command_sender, command_receiver) = flume::unbounded();
        let (_event_sender, event_receiver) = flume::unbounded();
        let thread = thread::spawn(move || {
            assert_eq!(command_receiver.recv(), Err(flume::RecvError::Disconnected));
        });

        drop(BackendActor::new(command_sender, event_receiver, thread));
    }

    #[test]
    fn send_command_reports_stopped_actor() {
        let (command_sender, command_receiver) = flume::unbounded();
        let (_event_sender, event_receiver) = flume::unbounded();
        let thread = thread::spawn(|| {});
        let actor = BackendActor::new(command_sender, event_receiver, thread);
        drop(command_receiver);

        assert_eq!(
            actor.send_command(BackendCommand::StartBreak(test_break())),
            Err(BackendActorError::Stopped)
        );
    }

    fn test_break() -> ScheduledBreak {
        ScheduledBreak {
            name: String::from("short"),
            origin: BreakOrigin::Manual,
            duration: Duration::from_secs(1),
            messages: vec![String::from("Rest your eyes")],
            autolock: false,
        }
    }
}
