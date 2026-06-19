use crate::backend::{Backend, BackendCommand, DisableRequest, RuntimeEvent};
use crate::config::Config;
#[cfg(target_os = "macos")]
use crate::macos_helper::MacOSHelperBackend;
use crate::scheduler::{BreakSchedule, BreakScheduler, ScheduledBreak};
#[cfg(target_os = "linux")]
use crate::x11_activity::X11ActivityBackend;
use std::time::Duration;

#[cfg(target_os = "linux")]
pub(crate) fn run() -> Result<(), crate::Error> {
    let Config { breaks, lock, .. } = Config::load()?;
    let schedule = BreakSchedule::try_from(breaks)?;
    let mut backend = X11ActivityBackend::connect(lock)?;

    run_with_backend(schedule, &mut backend);
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn run() -> Result<(), crate::Error> {
    let Config { breaks, lock, .. } = Config::load()?;
    let schedule = BreakSchedule::try_from(breaks)?;
    let mut backend = MacOSHelperBackend::connect(lock)?;

    run_with_backend(schedule, &mut backend);
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn run() -> Result<(), crate::Error> {
    Err(crate::Error::unsupported_platform())
}

fn run_with_backend<B>(schedule: BreakSchedule, backend: &mut B)
where
    B: Backend,
{
    let scheduler = BreakScheduler::new(schedule);
    let mut daemon = DaemonRuntime {
        scheduler,
        backend,
        disable_mode: DisableMode::Enabled,
        current_break: None,
    };

    daemon.run();
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
    current_break: Option<CurrentBreakState>,
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
            RuntimeEvent::LockAfterCurrentBreak => self.request_lock_after_current_break(),
            RuntimeEvent::StartManualBreak(name) => self.start_manual_break(&name),
            RuntimeEvent::Disable(DisableRequest::For(duration)) => self.disable_for(duration),
            RuntimeEvent::Disable(DisableRequest::UntilRestart) => self.disable_until_restart(),
            RuntimeEvent::Enable => self.enable(),
            RuntimeEvent::Shutdown => return false,
        }

        true
    }

    fn advance_active(&mut self, elapsed: Duration) {
        if let Some(scheduled_break) = self.scheduler.advance_active(elapsed) {
            self.start_break(scheduled_break);
        }
    }

    fn start_manual_break(&mut self, name: &str) {
        if let Some(scheduled_break) = self.scheduler.start_manual_break(name) {
            self.start_break(scheduled_break);
        }
    }

    fn start_break(&mut self, scheduled_break: ScheduledBreak) {
        self.current_break = Some(CurrentBreakState::for_break(&scheduled_break));
        self.handle_command(BackendCommand::StartBreak(scheduled_break));
    }

    fn finish_break(&mut self) {
        let should_lock = self
            .current_break
            .take()
            .is_some_and(CurrentBreakState::lock_after);

        if self.scheduler.finish_break() {
            self.handle_command(BackendCommand::FinishBreak {
                lock_after: should_lock,
            });
        }
    }

    fn request_lock_after_current_break(&mut self) {
        if let Some(current_break) = &mut self.current_break {
            current_break.request_lock_after();
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
        self.current_break = None;

        if self.scheduler.disable() {
            self.handle_command(BackendCommand::ClearBreak);
        }
    }

    fn handle_command(&mut self, command: BackendCommand) {
        self.backend.handle_command(command);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CurrentBreakState {
    lock_after: bool,
}

impl CurrentBreakState {
    const fn for_break(scheduled_break: &ScheduledBreak) -> Self {
        Self {
            lock_after: scheduled_break.autolock,
        }
    }

    fn request_lock_after(&mut self) {
        self.lock_after = true;
    }

    const fn lock_after(self) -> bool {
        self.lock_after
    }
}

#[cfg(test)]
mod tests;
