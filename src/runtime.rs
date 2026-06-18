use crate::config::{Config, ConfigError};
use crate::scheduler::{BreakSchedule, BreakScheduler, ScheduledBreak, SchedulerAction};
use std::time::Duration;

pub(crate) fn run() -> Result<(), crate::Error> {
    let config = Config::load()?;
    let mut backend = NoopBackend;

    run_with_backend(config, &mut backend)?;
    Ok(())
}

fn run_with_backend<B>(config: Config, backend: &mut B) -> Result<(), ConfigError>
where
    B: DaemonBackend,
{
    let schedule = BreakSchedule::try_from(config.breaks)?;
    let scheduler = BreakScheduler::new(schedule);
    let mut daemon = DaemonRuntime {
        scheduler,
        backend,
        disable_mode: DisableMode::Enabled,
    };

    daemon.run();
    Ok(())
}

trait DaemonBackend {
    fn next_event(&mut self) -> RuntimeEvent;

    fn start_break(&mut self, scheduled_break: ScheduledBreak);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum RuntimeEvent {
    Active(Duration),
    WallClock(Duration),
    BreakFinished,
    DisableFor(Duration),
    DisableUntilRestart,
    Enable,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisableMode {
    Enabled,
    Timed(Duration),
    UntilRestart,
}

struct DaemonRuntime<'a, B>
where
    B: DaemonBackend,
{
    scheduler: BreakScheduler,
    backend: &'a mut B,
    disable_mode: DisableMode,
}

impl<B> DaemonRuntime<'_, B>
where
    B: DaemonBackend,
{
    fn run(&mut self) {
        loop {
            let event = self.backend.next_event();
            if !self.handle_event(event) {
                break;
            }
        }
    }

    fn handle_event(&mut self, event: RuntimeEvent) -> bool {
        match event {
            RuntimeEvent::Active(elapsed) => self.advance_active(elapsed),
            RuntimeEvent::WallClock(elapsed) => self.advance_wall_clock(elapsed),
            RuntimeEvent::BreakFinished => self.scheduler.finish_break(),
            RuntimeEvent::DisableFor(duration) => self.disable_for(duration),
            RuntimeEvent::DisableUntilRestart => self.disable_until_restart(),
            RuntimeEvent::Enable => self.enable(),
            RuntimeEvent::Shutdown => return false,
        }

        true
    }

    fn advance_active(&mut self, elapsed: Duration) {
        if let SchedulerAction::StartBreak(scheduled_break) = self.scheduler.advance_active(elapsed)
        {
            self.backend.start_break(scheduled_break);
        }
    }

    fn advance_wall_clock(&mut self, elapsed: Duration) {
        match self.disable_mode {
            DisableMode::Timed(remaining) if elapsed >= remaining => self.enable(),
            DisableMode::Timed(remaining) => {
                self.disable_mode = DisableMode::Timed(remaining.saturating_sub(elapsed));
            }
            DisableMode::Enabled | DisableMode::UntilRestart => {}
        }
    }

    fn disable_for(&mut self, duration: Duration) {
        self.scheduler.disable();
        self.disable_mode = DisableMode::Timed(duration);
    }

    fn disable_until_restart(&mut self) {
        self.scheduler.disable();
        self.disable_mode = DisableMode::UntilRestart;
    }

    fn enable(&mut self) {
        self.scheduler.enable();
        self.disable_mode = DisableMode::Enabled;
    }
}

struct NoopBackend;

impl DaemonBackend for NoopBackend {
    fn next_event(&mut self) -> RuntimeEvent {
        RuntimeEvent::Shutdown
    }

    fn start_break(&mut self, _scheduled_break: ScheduledBreak) {}
}

#[cfg(test)]
mod tests;
