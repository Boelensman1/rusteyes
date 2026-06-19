use crate::scheduler::ScheduledBreak;
use std::time::Duration;

pub(crate) trait Backend {
    fn next_event(&mut self) -> RuntimeEvent;

    fn handle_command(&mut self, _command: BackendCommand) {}
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum RuntimeEvent {
    ActiveTimeElapsed(Duration),
    WallClockElapsed(Duration),
    BreakFinished,
    LockAfterCurrentBreak,
    StartManualBreak(String),
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
#[allow(clippy::enum_variant_names)]
pub(crate) enum BackendCommand {
    StartBreak(ScheduledBreak),
    FinishBreak { lock_after: bool },
    ClearBreak,
}
