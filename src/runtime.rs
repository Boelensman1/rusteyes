use crate::backend::{Backend, BackendEvent, NoopBackend};
use crate::config::{Config, ConfigError};
use crate::scheduler::{BreakSchedule, BreakScheduler, SchedulerAction};
use std::time::Duration;

pub(crate) fn run() -> Result<(), crate::Error> {
    let config = Config::load()?;
    let mut backend = NoopBackend;

    run_with_backend(config, &mut backend)?;
    Ok(())
}

fn run_with_backend<B>(config: Config, backend: &mut B) -> Result<(), ConfigError>
where
    B: Backend,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisableMode {
    Enabled,
    Timed(Duration),
    UntilRestart,
}

struct DaemonRuntime<'a, B>
where
    B: Backend,
{
    scheduler: BreakScheduler,
    backend: &'a mut B,
    disable_mode: DisableMode,
}

impl<B> DaemonRuntime<'_, B>
where
    B: Backend,
{
    fn run(&mut self) {
        loop {
            let event = self.backend.next_event();
            if !self.handle_event(event) {
                break;
            }
        }
    }

    fn handle_event(&mut self, event: BackendEvent) -> bool {
        match event {
            BackendEvent::Active(elapsed) => self.advance_active(elapsed),
            BackendEvent::WallClock(elapsed) => self.advance_wall_clock(elapsed),
            BackendEvent::BreakFinished => self.finish_break(),
            BackendEvent::DisableFor(duration) => self.disable_for(duration),
            BackendEvent::DisableUntilRestart => self.disable_until_restart(),
            BackendEvent::Enable => self.enable(),
            BackendEvent::Shutdown => return false,
        }

        true
    }

    fn advance_active(&mut self, elapsed: Duration) {
        if let SchedulerAction::StartBreak(scheduled_break) = self.scheduler.advance_active(elapsed)
        {
            self.backend.start_break(scheduled_break);
        }
    }

    fn finish_break(&mut self) {
        let pending_break = self.scheduler.pending_break().cloned();
        self.scheduler.finish_break();

        if let Some(scheduled_break) = pending_break {
            self.backend.clear_break();

            if scheduled_break.autolock {
                self.backend.request_lock();
            }
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
        self.clear_pending_break();
        self.scheduler.disable();
        self.disable_mode = DisableMode::Timed(duration);
    }

    fn disable_until_restart(&mut self) {
        self.clear_pending_break();
        self.scheduler.disable();
        self.disable_mode = DisableMode::UntilRestart;
    }

    fn enable(&mut self) {
        self.scheduler.enable();
        self.disable_mode = DisableMode::Enabled;
    }

    fn clear_pending_break(&mut self) {
        if self.scheduler.pending_break().is_some() {
            self.backend.clear_break();
        }
    }
}

#[cfg(test)]
mod tests;
