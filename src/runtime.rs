#[cfg(not(target_os = "linux"))]
use crate::backend::NoopBackend;
use crate::backend::{Backend, BackendCommand, DisableRequest, RuntimeEvent};
use crate::config::{Config, ConfigError};
use crate::scheduler::{BreakSchedule, BreakScheduler};
#[cfg(target_os = "linux")]
use crate::x11_activity::X11ActivityBackend;
use std::time::Duration;

#[cfg(target_os = "linux")]
pub(crate) fn run() -> Result<(), crate::Error> {
    let config = Config::load()?;
    let mut backend = X11ActivityBackend::connect(config.lock.clone())?;

    run_with_backend(config, &mut backend)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
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

    fn handle_event(&mut self, event: RuntimeEvent) -> bool {
        match event {
            RuntimeEvent::ActiveTimeElapsed(elapsed) => self.advance_active(elapsed),
            RuntimeEvent::WallClockElapsed(elapsed) => self.advance_wall_clock(elapsed),
            RuntimeEvent::BreakFinished => self.finish_break(),
            RuntimeEvent::Disable(DisableRequest::For(duration)) => self.disable_for(duration),
            RuntimeEvent::Disable(DisableRequest::UntilRestart) => self.disable_until_restart(),
            RuntimeEvent::Enable => self.enable(),
            RuntimeEvent::Shutdown => return false,
        }

        true
    }

    fn advance_active(&mut self, elapsed: Duration) {
        if let Some(scheduled_break) = self.scheduler.advance_active(elapsed) {
            self.handle_command(BackendCommand::StartBreak(scheduled_break));
        }
    }

    fn finish_break(&mut self) {
        if let Some(scheduled_break) = self.scheduler.finish_break() {
            self.handle_command(BackendCommand::ClearBreak);

            if scheduled_break.autolock {
                self.handle_command(BackendCommand::RequestLock);
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
        self.disable_scheduler();
        self.disable_mode = DisableMode::Timed(duration);
    }

    fn disable_until_restart(&mut self) {
        self.disable_scheduler();
        self.disable_mode = DisableMode::UntilRestart;
    }

    fn enable(&mut self) {
        self.scheduler.enable();
        self.disable_mode = DisableMode::Enabled;
    }

    fn disable_scheduler(&mut self) {
        if self.scheduler.disable().is_some() {
            self.handle_command(BackendCommand::ClearBreak);
        }
    }

    fn handle_command(&mut self, command: BackendCommand) {
        self.backend.handle_command(command);
    }
}

#[cfg(test)]
mod tests;
