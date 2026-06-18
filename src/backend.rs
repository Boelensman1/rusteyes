use crate::scheduler::ScheduledBreak;
use std::time::Duration;

pub(crate) trait Backend {
    fn next_event(&mut self) -> BackendEvent;

    fn start_break(&mut self, scheduled_break: ScheduledBreak);

    fn clear_break(&mut self);

    fn request_lock(&mut self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum BackendEvent {
    Active(Duration),
    WallClock(Duration),
    BreakFinished,
    DisableFor(Duration),
    DisableUntilRestart,
    Enable,
    Shutdown,
}

pub(crate) struct NoopBackend;

impl Backend for NoopBackend {
    fn next_event(&mut self) -> BackendEvent {
        BackendEvent::Shutdown
    }

    fn start_break(&mut self, _scheduled_break: ScheduledBreak) {}

    fn clear_break(&mut self) {}

    fn request_lock(&mut self) {}
}
