use crate::scheduler::ScheduledBreak;
use std::time::Duration;

pub(crate) trait Backend {
    fn next_event(&mut self) -> RuntimeEvent;

    fn handle_command(&mut self, command: BackendCommand);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum RuntimeEvent {
    ActiveTimeElapsed(Duration),
    WallClockElapsed(Duration),
    BreakFinished,
    Disable(DisableRequest),
    Enable,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum DisableRequest {
    For(Duration),
    UntilRestart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackendCommand {
    StartBreak(ScheduledBreak),
    ClearBreak,
    RequestLock,
}

#[cfg(any(test, not(target_os = "linux")))]
#[allow(dead_code)]
pub(crate) struct NoopBackend;

#[cfg(any(test, not(target_os = "linux")))]
impl Backend for NoopBackend {
    fn next_event(&mut self) -> RuntimeEvent {
        RuntimeEvent::Shutdown
    }

    fn handle_command(&mut self, _command: BackendCommand) {}
}
